use std::ffi::c_void;

use apple_cf::iosurface::IOSurface;
use nfidb_core::VideoCodec;
use videotoolbox::compression::{CompressionSession, ProfileLevel};

use crate::hardware::{configure_interactive_latency, require_hardware_session, vt_codec};

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetFormatDescription(sample_buffer: *const c_void) -> *const c_void;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        video_description: *const c_void,
        parameter_set_index: usize,
        parameter_set_pointer_out: *mut *const u8,
        parameter_set_size_out: *mut usize,
        parameter_set_count_out: *mut usize,
        nal_unit_header_length_out: *mut i32,
    ) -> i32;
    fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        video_description: *const c_void,
        parameter_set_index: usize,
        parameter_set_pointer_out: *mut *const u8,
        parameter_set_size_out: *mut usize,
        parameter_set_count_out: *mut usize,
        nal_unit_header_length_out: *mut i32,
    ) -> i32;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoToolboxEncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
}

pub(crate) struct VideoToolboxEncoder {
    session: CompressionSession,
    config: VideoToolboxEncoderConfig,
    frame_number: i64,
    restart_for_keyframe: bool,
}

pub(crate) struct EncodedSurface {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

impl VideoToolboxEncoder {
    pub fn new(config: VideoToolboxEncoderConfig) -> Result<Self, String> {
        Ok(Self {
            session: build_session(config)?,
            config,
            frame_number: 0,
            restart_for_keyframe: false,
        })
    }

    pub fn request_keyframe(&mut self) {
        self.restart_for_keyframe = true;
    }

    pub fn encode(&mut self, surface: &IOSurface) -> Result<EncodedSurface, String> {
        if self.restart_for_keyframe {
            self.session = build_session(self.config)?;
            self.frame_number = 0;
            self.restart_for_keyframe = false;
        }
        let encoded = self
            .session
            .encode(surface, (self.frame_number, self.config.max_fps.max(1) as i32))
            .map_err(|error| error.to_string())?;
        self.frame_number = self.frame_number.saturating_add(1);
        let (mut data, keyframe) = length_prefixed_to_annex_b(self.config.codec, &encoded.data)?;
        if keyframe {
            let parameter_sets = unsafe { parameter_sets(encoded.cm_sample_buffer_ptr().cast(), self.config.codec) }?;
            if !parameter_sets.is_empty() {
                let mut with_headers = Vec::with_capacity(parameter_sets.len() + data.len());
                with_headers.extend_from_slice(&parameter_sets);
                with_headers.append(&mut data);
                data = with_headers;
            }
        }
        Ok(EncodedSurface { data, keyframe })
    }
}

fn build_session(config: VideoToolboxEncoderConfig) -> Result<CompressionSession, String> {
    let codec = vt_codec(config.codec).ok_or_else(|| "VideoToolbox AV1 encoding is unavailable".to_owned())?;
    let mut builder = CompressionSession::builder(config.width as i32, config.height as i32, codec)
        .with_real_time(true)
        .with_allow_frame_reordering(false)
        .with_average_bit_rate(config.bitrate_bps.min(i32::MAX as u32) as i32)
        .with_expected_frame_rate(f64::from(config.max_fps))
        .with_max_keyframe_interval((config.max_fps.saturating_mul(2)).min(i32::MAX as u32) as i32);
    builder = match config.codec {
        VideoCodec::H264 => builder.with_profile_level(ProfileLevel::H264BaselineAutoLevel),
        VideoCodec::Hevc => builder.with_profile_level(ProfileLevel::HEVCMainAutoLevel),
        VideoCodec::Av1 => builder,
    };
    let session = builder.build().map_err(|error| error.to_string())?;
    configure_interactive_latency(&session);
    require_hardware_session(&session)?;
    Ok(session)
}

fn length_prefixed_to_annex_b(codec: VideoCodec, bytes: &[u8]) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::with_capacity(bytes.len() + 32);
    let mut cursor = 0;
    let mut keyframe = false;
    while cursor < bytes.len() {
        let length_bytes: [u8; 4] = bytes
            .get(cursor..cursor + 4)
            .ok_or_else(|| "VideoToolbox returned a truncated NAL length".to_owned())?
            .try_into()
            .map_err(|_| "VideoToolbox returned an invalid NAL length".to_owned())?;
        cursor += 4;
        let length = u32::from_be_bytes(length_bytes) as usize;
        let nal = bytes
            .get(cursor..cursor.saturating_add(length))
            .ok_or_else(|| "VideoToolbox returned a truncated NAL unit".to_owned())?;
        if nal.is_empty() {
            return Err("VideoToolbox returned an empty NAL unit".to_owned());
        }
        keyframe |= match codec {
            VideoCodec::H264 => nal[0] & 0x1f == 5,
            VideoCodec::Hevc => matches!((nal[0] >> 1) & 0x3f, 16..=21),
            VideoCodec::Av1 => false,
        };
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        cursor += length;
    }
    Ok((output, keyframe))
}

unsafe fn parameter_sets(sample_buffer: *const c_void, codec: VideoCodec) -> Result<Vec<u8>, String> {
    if sample_buffer.is_null() {
        return Ok(Vec::new());
    }
    let description = unsafe { CMSampleBufferGetFormatDescription(sample_buffer) };
    if description.is_null() {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    let mut count = 0_usize;
    let mut header_length = 0_i32;
    let getter: unsafe extern "C" fn(*const c_void, usize, *mut *const u8, *mut usize, *mut usize, *mut i32) -> i32 =
        match codec {
            VideoCodec::H264 => CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
            VideoCodec::Hevc => CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
            VideoCodec::Av1 => return Ok(output),
        };
    let status = unsafe {
        getter(
            description,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut count,
            &mut header_length,
        )
    };
    if status != 0 {
        return Err(format!("CoreMedia parameter-set query failed with OSStatus {status}"));
    }
    for index in 0..count {
        let mut pointer = std::ptr::null();
        let mut size = 0_usize;
        let status = unsafe {
            getter(
                description,
                index,
                &mut pointer,
                &mut size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 || pointer.is_null() {
            return Err(format!("CoreMedia parameter set {index} failed with OSStatus {status}"));
        }
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(pointer, size) });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_h264_avcc_and_detects_idr() {
        let bytes = [0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 2, 0x41, 0x99];
        let (annex_b, keyframe) = length_prefixed_to_annex_b(VideoCodec::H264, &bytes).unwrap();
        assert!(keyframe);
        assert_eq!(&annex_b[..6], &[0, 0, 0, 1, 0x65, 0x88]);
    }

    #[test]
    fn rejects_truncated_video_toolbox_output() {
        assert!(length_prefixed_to_annex_b(VideoCodec::H264, &[0, 0, 0, 8, 1]).is_err());
    }

    #[test]
    fn hardware_h264_keyframe_contains_baseline_decoder_configuration() {
        let width = 1920;
        let height = 1080;
        let pixels = vec![0x80_u8; width as usize * height as usize * 4];
        let surface = crate::capture::bgra_iosurface(width, height, &pixels).unwrap();
        let mut encoder = VideoToolboxEncoder::new(VideoToolboxEncoderConfig {
            codec: VideoCodec::H264,
            width,
            height,
            max_fps: 60,
            bitrate_bps: 4_000_000,
        })
        .unwrap();

        encoder.request_keyframe();
        let encoded = encoder.encode(&surface).unwrap();
        assert!(encoded.keyframe);

        let nals = annex_b_nals(&encoded.data);
        let sps = nals
            .iter()
            .find(|nal| !nal.is_empty() && nal[0] & 0x1f == 7)
            .expect("keyframe must carry an SPS");
        assert!(
            nals.iter().any(|nal| !nal.is_empty() && nal[0] & 0x1f == 8),
            "keyframe must carry a PPS"
        );
        assert!(
            nals.iter().any(|nal| !nal.is_empty() && nal[0] & 0x1f == 5),
            "keyframe must carry an IDR slice"
        );
        assert_eq!(
            sps.get(1).copied(),
            Some(66),
            "SPS must advertise H.264 Baseline profile"
        );
        assert_eq!(
            sps.get(2).copied(),
            Some(0),
            "SPS compatibility flags must match the 4200 WebRTC profile"
        );
    }

    fn annex_b_nals(bytes: &[u8]) -> Vec<&[u8]> {
        let mut starts = Vec::new();
        let mut cursor = 0;
        while cursor + 3 < bytes.len() {
            if bytes[cursor..].starts_with(&[0, 0, 0, 1]) {
                starts.push(cursor + 4);
                cursor += 4;
            } else if bytes[cursor..].starts_with(&[0, 0, 1]) {
                starts.push(cursor + 3);
                cursor += 3;
            } else {
                cursor += 1;
            }
        }
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| {
                let end = starts
                    .get(index + 1)
                    .map(|next| next.saturating_sub(4))
                    .unwrap_or(bytes.len());
                &bytes[*start..end]
            })
            .collect()
    }
}
