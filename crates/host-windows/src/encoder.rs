use nfidb_core::{EncoderBackend, EncoderMode, VideoCodec};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVBuffer, YUVSource};

use crate::MediaFoundationEncoder;

#[derive(Debug, Clone, Copy)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodec,
    pub mode: EncoderMode,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
}

pub struct VideoFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub yuv: &'a YUVBuffer,
}

#[derive(Debug)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

/// Codec-neutral interface used by live capture and deterministic benchmarks.
/// Implementations may rebuild only their encoder when a vendor does not
/// support an in-place setting or keyframe change.
pub trait VideoEncoder {
    fn codec(&self) -> VideoCodec;
    fn backend(&self) -> EncoderBackend;
    fn name(&self) -> &str;
    fn configure(&mut self, config: VideoEncoderConfig) -> Result<(), String>;
    fn encode(&mut self, frame: VideoFrame<'_>) -> Result<Option<EncodedPacket>, String>;
    fn request_keyframe(&mut self) -> Result<(), String>;
    fn shutdown(&mut self);
}

pub fn create_video_encoder(config: VideoEncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
    if config.mode == EncoderMode::H264Software {
        Ok(Box::new(OpenH264VideoEncoder::new(config)?))
    } else {
        Ok(Box::new(MediaFoundationVideoEncoder::new(config)?))
    }
}

struct OpenH264VideoEncoder {
    encoder: Encoder,
    config: VideoEncoderConfig,
}

impl OpenH264VideoEncoder {
    fn new(config: VideoEncoderConfig) -> Result<Self, String> {
        if config.codec != VideoCodec::H264 || config.mode != EncoderMode::H264Software {
            return Err("OpenH264 only implements the H.264 software mode".to_owned());
        }
        let encoder = make_openh264(config)?;
        Ok(Self { encoder, config })
    }
}

impl VideoEncoder for OpenH264VideoEncoder {
    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }

    fn backend(&self) -> EncoderBackend {
        EncoderBackend::OpenH264Software
    }

    fn name(&self) -> &str {
        "OpenH264 software encoder"
    }

    fn configure(&mut self, config: VideoEncoderConfig) -> Result<(), String> {
        if config.codec != VideoCodec::H264 || config.mode != EncoderMode::H264Software {
            return Err("OpenH264 cannot be configured for this mode".to_owned());
        }
        self.encoder = make_openh264(config)?;
        self.config = config;
        Ok(())
    }

    fn encode(&mut self, frame: VideoFrame<'_>) -> Result<Option<EncodedPacket>, String> {
        let bitstream = self.encoder.encode(frame.yuv).map_err(|error| error.to_string())?;
        if bitstream.frame_type() == FrameType::Skip {
            return Ok(None);
        }
        Ok(Some(EncodedPacket {
            keyframe: matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I),
            data: bitstream.to_vec(),
        }))
    }

    fn request_keyframe(&mut self) -> Result<(), String> {
        self.encoder.force_intra_frame();
        Ok(())
    }

    fn shutdown(&mut self) {}
}

fn make_openh264(config: VideoEncoderConfig) -> Result<Encoder, String> {
    let encoder = EncoderConfig::new()
        .bitrate(BitRate::from_bps(config.bitrate_bps))
        .max_frame_rate(FrameRate::from_hz(config.max_fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .usage_type(UsageType::ScreenContentRealTime)
        .profile(Profile::Baseline)
        .level(Level::Level_4_1)
        .complexity(Complexity::Low)
        .skip_frames(true)
        .scene_change_detect(true)
        .adaptive_quantization(false)
        .background_detection(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(0))
        .vui(VuiConfig::srgb());
    Encoder::with_api_config(OpenH264API::from_source(), encoder).map_err(|error| error.to_string())
}

struct MediaFoundationVideoEncoder {
    encoder: Option<MediaFoundationEncoder>,
    config: VideoEncoderConfig,
    name: String,
    force_rebuild: bool,
}

impl MediaFoundationVideoEncoder {
    fn new(config: VideoEncoderConfig) -> Result<Self, String> {
        if !config.mode.requires_hardware() || config.mode.codec() != Some(config.codec) {
            return Err("Media Foundation hardware mode and codec do not match".to_owned());
        }
        let encoder = MediaFoundationEncoder::new(
            config.codec,
            config.width,
            config.height,
            config.max_fps,
            config.bitrate_bps,
        )?;
        let name = encoder.encoder_name.clone();
        Ok(Self {
            encoder: Some(encoder),
            config,
            name,
            force_rebuild: false,
        })
    }

    fn rebuild(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.config.width = width;
        self.config.height = height;
        let encoder = MediaFoundationEncoder::new(
            self.config.codec,
            width,
            height,
            self.config.max_fps,
            self.config.bitrate_bps,
        )?;
        self.name = encoder.encoder_name.clone();
        self.encoder = Some(encoder);
        self.force_rebuild = false;
        Ok(())
    }
}

impl VideoEncoder for MediaFoundationVideoEncoder {
    fn codec(&self) -> VideoCodec {
        self.config.codec
    }

    fn backend(&self) -> EncoderBackend {
        EncoderBackend::MediaFoundationHardware
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn configure(&mut self, config: VideoEncoderConfig) -> Result<(), String> {
        if !config.mode.requires_hardware() || config.mode.codec() != Some(config.codec) {
            return Err("Media Foundation hardware mode and codec do not match".to_owned());
        }
        self.config = config;
        self.rebuild(config.width, config.height)
    }

    fn encode(&mut self, frame: VideoFrame<'_>) -> Result<Option<EncodedPacket>, String> {
        if self.force_rebuild || (frame.width, frame.height) != (self.config.width, self.config.height) {
            self.rebuild(frame.width, frame.height)?;
        }
        let nv12 = i420_to_nv12(frame.yuv);
        let packet = self
            .encoder
            .as_mut()
            .ok_or_else(|| "hardware encoder is shut down".to_owned())?
            .encode_nv12(&nv12)?;
        Ok(Some(EncodedPacket {
            data: packet.data,
            keyframe: packet.keyframe,
        }))
    }

    fn request_keyframe(&mut self) -> Result<(), String> {
        // Rebuilding is the reliable cross-vendor path for fresh parameter sets
        // and an IDR/IRAP. It leaves capture, pairing, and input transport alive.
        self.force_rebuild = true;
        Ok(())
    }

    fn shutdown(&mut self) {
        self.encoder = None;
    }
}

pub(crate) fn i420_to_nv12(source: &YUVBuffer) -> Vec<u8> {
    let (width, height) = source.dimensions();
    let y_len = width * height;
    let chroma_len = y_len / 4;
    let mut nv12 = Vec::with_capacity(y_len + chroma_len * 2);
    nv12.extend_from_slice(&source.y()[..y_len]);
    for index in 0..chroma_len {
        nv12.push(source.u()[index]);
        nv12.push(source.v()[index]);
    }
    nv12
}
