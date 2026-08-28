use nfidb_core::{CapabilityState, EncoderBackend, EncoderCapability, VideoCodec};
use videotoolbox::compression::{CompressionSession, ProfileLevel};
use videotoolbox::encoder_list::available_video_encoder_details;
use videotoolbox::ffi;
use videotoolbox::session::Codec;

const H264_FOURCC: u32 = u32::from_be_bytes(*b"avc1");
const HEVC_FOURCC: u32 = u32::from_be_bytes(*b"hvc1");
const AV1_FOURCC: u32 = u32::from_be_bytes(*b"av01");

#[must_use]
pub fn discover_video_encoders() -> Vec<EncoderCapability> {
    let details = match available_video_encoder_details() {
        Ok(details) => details,
        Err(status) => {
            return vec![
                unavailable(
                    VideoCodec::H264,
                    format!("VTCopyVideoEncoderList failed with OSStatus {status}"),
                ),
                unavailable(
                    VideoCodec::Hevc,
                    format!("VTCopyVideoEncoderList failed with OSStatus {status}"),
                ),
                unavailable(
                    VideoCodec::Av1,
                    format!("VTCopyVideoEncoderList failed with OSStatus {status}"),
                ),
                software_fallback(),
            ];
        }
    };

    let mut capabilities = Vec::new();
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
        let fourcc = fourcc(codec);
        let candidates: Vec<_> = details
            .iter()
            .filter(|encoder| encoder.base.codec_type == fourcc && encoder.is_hardware_accelerated == Some(true))
            .collect();
        if candidates.is_empty() {
            capabilities.push(unavailable(
                codec,
                format!("VideoToolbox exposes no hardware {} encoder on this Mac", codec.label()),
            ));
            continue;
        }
        for candidate in candidates {
            let probe = functional_probe(codec, 1920, 1080, 60, 8_000_000);
            let (state, failure_reason) = match probe {
                Ok(()) => (CapabilityState::Functional, None),
                Err(error) => (CapabilityState::Failed, Some(error)),
            };
            capabilities.push(EncoderCapability {
                id: candidate.base.encoder_id.clone(),
                codec,
                backend: EncoderBackend::VideoToolboxHardware,
                hardware: true,
                encoder_name: candidate.base.encoder_name.clone(),
                adapter_name: candidate.gpu_registry_id.map(|id| format!("Metal registry {id:#x}")),
                adapter_luid: candidate.gpu_registry_id.map(|id| format!("{id:#x}")),
                vendor: Some("Apple VideoToolbox".to_owned()),
                driver_version: None,
                input_formats: vec!["420v / NV12 IOSurface".to_owned(), "BGRA IOSurface".to_owned()],
                profiles: profiles(codec),
                low_latency: Some(true),
                rate_control: vec!["average bitrate".to_owned(), "real-time".to_owned()],
                maximum_tested_width: (state == CapabilityState::Functional).then_some(1920),
                maximum_tested_height: (state == CapabilityState::Functional).then_some(1080),
                maximum_tested_fps: (state == CapabilityState::Functional).then_some(60),
                state,
                failure_reason,
            });
        }
    }
    capabilities.push(software_fallback());
    capabilities
}

pub(crate) fn functional_probe(
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
) -> Result<(), String> {
    let codec = vt_codec(codec)
        .ok_or_else(|| "VideoToolbox AV1 hardware encoding is not exposed by the current macOS SDK path".to_owned())?;
    let mut builder = CompressionSession::builder(width as i32, height as i32, codec)
        .with_real_time(true)
        .with_allow_frame_reordering(false)
        .with_average_bit_rate(bitrate_bps.min(i32::MAX as u32) as i32)
        .with_expected_frame_rate(f64::from(fps))
        .with_max_keyframe_interval((fps.saturating_mul(2)).min(i32::MAX as u32) as i32);
    builder = match codec {
        Codec::H264 => builder.with_profile_level(ProfileLevel::H264ConstrainedHighAutoLevel),
        Codec::HEVC => builder.with_profile_level(ProfileLevel::HEVCMainAutoLevel),
        _ => builder,
    };
    let session = builder.build().map_err(|error| error.to_string())?;
    configure_interactive_latency(&session);
    require_hardware_session(&session)
}

pub(crate) fn configure_interactive_latency(session: &CompressionSession) {
    let max_delayed_frames = 1_i32;
    let value = unsafe {
        ffi::CFNumberCreate(
            ffi::kCFAllocatorDefault,
            ffi::kCFNumberSInt32Type,
            (&raw const max_delayed_frames).cast(),
        )
    };
    if !value.is_null() {
        if let Err(error) =
            unsafe { session.set_property(ffi::kVTCompressionPropertyKey_MaxFrameDelayCount, value.cast()) }
        {
            tracing::debug!(%error, "VideoToolbox does not accept MaxFrameDelayCount for this encoder");
        }
        unsafe { ffi::CFRelease(value.cast()) };
    }
    if let Err(error) = unsafe {
        session.set_property(
            ffi::kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
            ffi::kCFBooleanTrue.cast(),
        )
    } {
        tracing::debug!(%error, "VideoToolbox does not accept PrioritizeEncodingSpeedOverQuality for this encoder");
    }
}

pub(crate) fn require_hardware_session(session: &CompressionSession) -> Result<(), String> {
    let value = unsafe { session.copy_property(ffi::kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder) }
        .map_err(|error| format!("could not verify VideoToolbox hardware acceleration: {error}"))?
        .ok_or_else(|| "VideoToolbox did not report whether the active encoder uses hardware".to_owned())?;
    if value.as_ptr().cast_const() == unsafe { ffi::kCFBooleanTrue.cast() } {
        Ok(())
    } else {
        Err("VideoToolbox created a software encoder while hardware acceleration was required".to_owned())
    }
}

pub(crate) const fn vt_codec(codec: VideoCodec) -> Option<Codec> {
    match codec {
        VideoCodec::H264 => Some(Codec::H264),
        VideoCodec::Hevc => Some(Codec::HEVC),
        VideoCodec::Av1 => None,
    }
}

const fn fourcc(codec: VideoCodec) -> u32 {
    match codec {
        VideoCodec::H264 => H264_FOURCC,
        VideoCodec::Hevc => HEVC_FOURCC,
        VideoCodec::Av1 => AV1_FOURCC,
    }
}

fn profiles(codec: VideoCodec) -> Vec<String> {
    match codec {
        VideoCodec::H264 => vec!["Constrained High Auto Level".to_owned()],
        VideoCodec::Hevc => vec!["Main Auto Level".to_owned()],
        VideoCodec::Av1 => Vec::new(),
    }
}

fn unavailable(codec: VideoCodec, reason: String) -> EncoderCapability {
    EncoderCapability {
        id: format!("videotoolbox-{}-unavailable", codec.label().to_ascii_lowercase()),
        codec,
        backend: EncoderBackend::VideoToolboxHardware,
        hardware: true,
        encoder_name: format!("{} Hardware", codec.label()),
        adapter_name: None,
        adapter_luid: None,
        vendor: Some("Apple VideoToolbox".to_owned()),
        driver_version: None,
        input_formats: Vec::new(),
        profiles: Vec::new(),
        low_latency: None,
        rate_control: Vec::new(),
        maximum_tested_width: None,
        maximum_tested_height: None,
        maximum_tested_fps: None,
        state: CapabilityState::Unavailable,
        failure_reason: Some(reason),
    }
}

fn software_fallback() -> EncoderCapability {
    EncoderCapability {
        id: "openh264-software".to_owned(),
        codec: VideoCodec::H264,
        backend: EncoderBackend::OpenH264Software,
        hardware: false,
        encoder_name: "OpenH264 software encoder".to_owned(),
        adapter_name: None,
        adapter_luid: None,
        vendor: Some("Cisco OpenH264".to_owned()),
        driver_version: None,
        input_formats: vec!["I420".to_owned()],
        profiles: vec!["Constrained Baseline".to_owned()],
        low_latency: Some(true),
        rate_control: vec!["bitrate".to_owned()],
        maximum_tested_width: None,
        maximum_tested_height: None,
        maximum_tested_fps: None,
        state: CapabilityState::Functional,
        failure_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_always_keeps_the_software_fallback() {
        let capabilities = discover_video_encoders();
        assert!(
            capabilities
                .iter()
                .any(|item| !item.hardware && item.codec == VideoCodec::H264)
        );
    }
}
