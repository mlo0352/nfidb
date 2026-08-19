use std::collections::HashMap;
use std::sync::OnceLock;

use nfidb_core::{CapabilityState, EncoderBackend, EncoderCapability, VideoCodec};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVLowLatencyMode, ICodecAPI,
    IMFActivate, IMFAttributes, IMFTransform, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_PROFILE,
    MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateMediaType, MFMediaType_Video, MFSTARTUP_FULL, MFStartup,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_ADAPTER_LUID, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_ENUM_HARDWARE_URL_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFT_REGISTER_TYPE_INFO, MFT_SET_TYPE_TEST_ONLY, MFVideoFormat_AV1, MFVideoFormat_H264,
    MFVideoFormat_HEVC, MFVideoFormat_I420, MFVideoFormat_IYUV, MFVideoFormat_NV12, MFVideoFormat_YV12,
    MFVideoInterlace_Progressive, eAVEncH264VProfile_ConstrainedBase, eAVEncH265VProfile_Main_420_8,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::{GUID, Interface};

static MEDIA_FOUNDATION: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Debug, Clone)]
struct AdapterIdentity {
    name: String,
    vendor: String,
}

/// Enumerates actual Media Foundation hardware transforms and performs a
/// non-destructive media-type initialization probe. A candidate is deliberately
/// not promoted to `Functional` here: that state is reserved for a transform
/// that has returned encoded bytes from the runtime/benchmark probe.
pub fn discover_video_encoders() -> Vec<EncoderCapability> {
    let adapters = enumerate_adapters();
    let mut capabilities = vec![software_h264_capability()];
    let Some(startup_error) = initialize_media_foundation().err() else {
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            match enumerate_codec(codec, &adapters) {
                Ok(mut discovered) => capabilities.append(&mut discovered),
                Err(error) => capabilities.push(unavailable_hardware(codec, error)),
            }
        }
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            match crate::mf_encoder::functional_probe(codec) {
                Ok((encoder_name, _, _)) => {
                    if let Some(capability) = capabilities.iter_mut().find(|item| {
                        item.codec == codec
                            && item.backend == EncoderBackend::MediaFoundationHardware
                            && item.encoder_name == encoder_name
                    }) {
                        capability.state = CapabilityState::Functional;
                        capability.maximum_tested_width = Some(1280);
                        capability.maximum_tested_height = Some(720);
                        capability.maximum_tested_fps = Some(60);
                        capability.failure_reason = None;
                    }
                }
                Err(error) => {
                    if let Some(capability) = capabilities
                        .iter_mut()
                        .find(|item| item.codec == codec && item.backend == EncoderBackend::MediaFoundationHardware)
                    {
                        capability.state = CapabilityState::Failed;
                        capability.failure_reason = Some(format!("encoded-frame probe failed: {error}"));
                    }
                }
            }
        }
        return capabilities;
    };
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
        capabilities.push(unavailable_hardware(codec, startup_error.clone()));
    }
    capabilities
}

pub(crate) fn initialize_media_foundation() -> Result<(), String> {
    MEDIA_FOUNDATION
        .get_or_init(|| unsafe {
            // RPC_E_CHANGED_MODE is harmless here: Media Foundation only needs
            // COM to be initialized on the calling thread in either apartment.
            let com = CoInitializeEx(None, COINIT_MULTITHREADED);
            if com.is_err() && com.0 != 0x8001_0106_u32 as i32 {
                return Err(format!("COM initialization failed: 0x{:08x}", com.0 as u32));
            }
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|error| format!("Media Foundation startup failed: {error}"))
        })
        .clone()
}

fn enumerate_codec(
    codec: VideoCodec,
    adapters: &HashMap<u64, AdapterIdentity>,
) -> Result<Vec<EncoderCapability>, String> {
    let output_subtype = codec_subtype(codec);
    let output_info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: output_subtype,
    };
    let flags = MFT_ENUM_FLAG(
        MFT_ENUM_FLAG_SYNCMFT.0
            | MFT_ENUM_FLAG_ASYNCMFT.0
            | MFT_ENUM_FLAG_HARDWARE.0
            | MFT_ENUM_FLAG_LOCALMFT.0
            | MFT_ENUM_FLAG_SORTANDFILTER.0,
    );
    let mut raw = std::ptr::null_mut();
    let mut count = 0_u32;
    unsafe {
        windows::Win32::Media::MediaFoundation::MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&output_info),
            &mut raw,
            &mut count,
        )
        .map_err(|error| format!("{} hardware enumeration failed: {error}", codec.label()))?;
    }
    if raw.is_null() || count == 0 {
        return Ok(vec![unavailable_hardware(
            codec,
            format!("no hardware {} encoder MFT was returned by Windows", codec.label()),
        )]);
    }

    let mut result = Vec::with_capacity(count as usize);
    unsafe {
        let activations = std::slice::from_raw_parts_mut(raw, count as usize);
        for activation in activations {
            if let Some(activation) = activation.take() {
                result.push(inspect_activation(codec, &activation, adapters));
            }
        }
        CoTaskMemFree(Some(raw.cast()));
    }
    if result.is_empty() {
        result.push(unavailable_hardware(
            codec,
            "Windows returned an empty activation list".to_owned(),
        ));
    }
    Ok(result)
}

fn inspect_activation(
    codec: VideoCodec,
    activation: &IMFActivate,
    adapters: &HashMap<u64, AdapterIdentity>,
) -> EncoderCapability {
    let attributes: &IMFAttributes = activation;
    let encoder_name = attribute_string(attributes, &MFT_FRIENDLY_NAME_Attribute)
        .unwrap_or_else(|| format!("{} Media Foundation hardware encoder", codec.label()));
    let hardware_url = attribute_string(attributes, &MFT_ENUM_HARDWARE_URL_Attribute);
    let luid_value = unsafe { attributes.GetUINT64(&MFT_ENUM_ADAPTER_LUID).ok() };
    let adapter = luid_value.and_then(|luid| adapters.get(&luid));
    let adapter = adapter.or_else(|| {
        let upper_name = encoder_name.to_ascii_uppercase();
        adapters
            .values()
            .find(|item| upper_name.contains(&item.vendor.to_ascii_uppercase()))
    });
    let id = format!(
        "mf:{}:{}",
        codec.label().to_ascii_lowercase().replace(['.', ' '], ""),
        luid_value.map_or_else(|| encoder_name.clone(), |value| format!("{value:016x}"))
    );

    let mut capability = EncoderCapability {
        id,
        codec,
        backend: EncoderBackend::MediaFoundationHardware,
        hardware: true,
        encoder_name,
        adapter_name: adapter.map(|item| item.name.clone()),
        adapter_luid: luid_value.map(|value| format!("{value:016x}")),
        vendor: adapter.map(|item| item.vendor.clone()),
        driver_version: None,
        input_formats: Vec::new(),
        profiles: Vec::new(),
        low_latency: None,
        rate_control: Vec::new(),
        maximum_tested_width: None,
        maximum_tested_height: None,
        maximum_tested_fps: None,
        state: CapabilityState::Detected,
        failure_reason: hardware_url.map(|url| format!("hardware endpoint: {url}")),
    };

    match unsafe { activation.ActivateObject::<IMFTransform>() } {
        Ok(transform) => {
            capability.input_formats = available_input_formats(&transform);
            if let Ok(codec_api) = transform.cast::<ICodecAPI>() {
                capability.low_latency = Some(unsafe { codec_api.IsSupported(&CODECAPI_AVLowLatencyMode).is_ok() });
                for (guid, label) in [
                    (&CODECAPI_AVEncCommonMeanBitRate, "mean-bitrate"),
                    (&CODECAPI_AVEncCommonRateControlMode, "rate-control"),
                ] {
                    if unsafe { codec_api.IsSupported(guid).is_ok() } {
                        capability.rate_control.push(label.to_owned());
                    }
                }
            }
            match configure_probe(&transform, codec, 1280, 720, 60, 5_000_000) {
                Ok(()) => {
                    if !capability.input_formats.iter().any(|format| format == "NV12") {
                        capability.input_formats.push("NV12".to_owned());
                    }
                    capability.profiles.push(
                        match codec {
                            VideoCodec::H264 => "Constrained Baseline",
                            VideoCodec::Hevc => "Main 4:2:0 8-bit",
                            VideoCodec::Av1 => "Main / profile 0",
                        }
                        .to_owned(),
                    );
                    capability.state = CapabilityState::Initializeable;
                    capability.failure_reason =
                        Some("media types initialized; an encoded-frame probe is still required before use".to_owned());
                }
                Err(error) => {
                    capability.state = CapabilityState::Failed;
                    capability.failure_reason = Some(format!("1280×720@60 initialization failed: {error}"));
                }
            }
            let _ = unsafe { activation.ShutdownObject() };
        }
        Err(error) => {
            capability.state = CapabilityState::Failed;
            capability.failure_reason = Some(format!("encoder activation failed: {error}"));
        }
    }
    capability
}

pub(crate) fn configure_probe(
    transform: &IMFTransform,
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) -> Result<(), String> {
    unsafe {
        if let Ok(attributes) = transform.GetAttributes() {
            let _ = attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
        }
        let output = MFCreateMediaType().map_err(|error| format!("create output media type: {error}"))?;
        set_video_type(&output, codec_subtype(codec), width, height, fps, Some(bitrate))
            .map_err(|error| format!("describe output media type: {error}"))?;
        let profile = match codec {
            VideoCodec::H264 => eAVEncH264VProfile_ConstrainedBase.0 as u32,
            VideoCodec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
            VideoCodec::Av1 => 0,
        };
        output
            .SetUINT32(&MF_MT_VIDEO_PROFILE, profile)
            .map_err(|error| format!("set codec profile: {error}"))?;
        let input = MFCreateMediaType().map_err(|error| format!("create input media type: {error}"))?;
        set_video_type(&input, MFVideoFormat_NV12, width, height, fps, None)
            .map_err(|error| format!("describe NV12 input media type: {error}"))?;
        transform
            .SetOutputType(0, &output, 0)
            .map_err(|error| format!("set encoded output type: {error}"))?;
        transform
            .SetInputType(0, &input, MFT_SET_TYPE_TEST_ONLY.0 as u32)
            .map_err(|error| format!("test NV12 input type: {error}"))?;
        transform
            .SetInputType(0, &input, 0)
            .map_err(|error| format!("set NV12 input type: {error}"))?;
    }
    Ok(())
}

fn set_video_type(
    media_type: &windows::Win32::Media::MediaFoundation::IMFMediaType,
    subtype: GUID,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: Option<u32>,
) -> windows::core::Result<()> {
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(width) << 32) | u64::from(height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1_u64 << 32) | 1)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        if let Some(bitrate) = bitrate {
            media_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        }
    }
    Ok(())
}

fn available_input_formats(transform: &IMFTransform) -> Vec<String> {
    let mut formats = Vec::new();
    for index in 0..64 {
        let Ok(media_type) = (unsafe { transform.GetInputAvailableType(0, index) }) else {
            break;
        };
        let Ok(subtype) = (unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }) else {
            continue;
        };
        let label = if subtype == MFVideoFormat_NV12 {
            "NV12"
        } else if subtype == MFVideoFormat_I420 {
            "I420"
        } else if subtype == MFVideoFormat_IYUV {
            "IYUV"
        } else if subtype == MFVideoFormat_YV12 {
            "YV12"
        } else {
            continue;
        };
        if !formats.iter().any(|item| item == label) {
            formats.push(label.to_owned());
        }
    }
    formats
}

fn enumerate_adapters() -> HashMap<u64, AdapterIdentity> {
    let mut result = HashMap::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return result;
    };
    for index in 0..32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        let end = desc
            .Description
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end]);
        let luid = (u64::from(desc.AdapterLuid.HighPart as u32) << 32) | u64::from(desc.AdapterLuid.LowPart);
        result.insert(
            luid,
            AdapterIdentity {
                name,
                vendor: vendor_name(desc.VendorId).to_owned(),
            },
        );
    }
    result
}

fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10de => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x1414 => "Microsoft",
        _ => "Unknown",
    }
}

pub(crate) fn attribute_string(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    let length = unsafe { attributes.GetStringLength(key).ok()? };
    let mut buffer = vec![0_u16; length as usize + 1];
    unsafe { attributes.GetString(key, &mut buffer, None).ok()? };
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

pub(crate) const fn codec_subtype(codec: VideoCodec) -> GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

fn unavailable_hardware(codec: VideoCodec, reason: String) -> EncoderCapability {
    EncoderCapability {
        id: format!("mf-{}-unavailable", codec.label().to_ascii_lowercase()),
        codec,
        backend: EncoderBackend::MediaFoundationHardware,
        hardware: true,
        encoder_name: format!("{} Hardware", codec.label()),
        adapter_name: None,
        adapter_luid: None,
        vendor: None,
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

fn software_h264_capability() -> EncoderCapability {
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
        profiles: vec!["Baseline".to_owned()],
        low_latency: Some(true),
        rate_control: vec!["bitrate".to_owned()],
        maximum_tested_width: Some(2560),
        maximum_tested_height: Some(1440),
        maximum_tested_fps: Some(60),
        state: CapabilityState::Functional,
        failure_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_always_keeps_software_fallback() {
        let capabilities = discover_video_encoders();
        println!("{capabilities:#?}");
        assert!(capabilities.iter().any(|item| {
            item.mode() == nfidb_core::EncoderMode::H264Software && item.state == CapabilityState::Functional
        }));
    }
}
