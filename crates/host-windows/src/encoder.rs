use nfidb_core::{EncoderBackend, EncoderMode, PipelineMemoryMode, VideoCodec};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVBuffer, YUVSource};

use crate::MediaFoundationEncoder;
use crate::gpu_preprocess::{GpuSurface, Nv12Readback};

#[derive(Debug, Clone, Copy)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodec,
    pub mode: EncoderMode,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
}

pub enum VideoFrameData<'a> {
    I420(&'a YUVBuffer),
    D3D11Nv12(&'a GpuSurface),
}

pub struct VideoFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub data: VideoFrameData<'a>,
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
    fn pipeline_memory_mode(&self) -> PipelineMemoryMode;
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

    fn pipeline_memory_mode(&self) -> PipelineMemoryMode {
        PipelineMemoryMode::CpuPreprocessing
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
        let VideoFrameData::I420(yuv) = frame.data else {
            return Err("OpenH264 requires an I420 system-memory frame".to_owned());
        };
        let bitstream = self.encoder.encode(yuv).map_err(|error| error.to_string())?;
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
    d3d_device_identity: Option<usize>,
    memory_mode: PipelineMemoryMode,
    nv12_readback: Nv12Readback,
    direct_gpu_disabled: Option<usize>,
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
            d3d_device_identity: None,
            memory_mode: PipelineMemoryMode::CpuPreprocessing,
            nv12_readback: Nv12Readback::default(),
            direct_gpu_disabled: None,
        })
    }

    fn rebuild(&mut self, width: u32, height: u32, gpu: Option<&GpuSurface>) -> Result<(), String> {
        self.config.width = width;
        self.config.height = height;
        let encoder = if let Some(gpu) = gpu {
            MediaFoundationEncoder::new_with_d3d11(
                self.config.codec,
                width,
                height,
                self.config.max_fps,
                self.config.bitrate_bps,
                &gpu.device,
            )?
        } else {
            MediaFoundationEncoder::new(
                self.config.codec,
                width,
                height,
                self.config.max_fps,
                self.config.bitrate_bps,
            )?
        };
        self.name = encoder.encoder_name.clone();
        self.encoder = Some(encoder);
        self.force_rebuild = false;
        self.d3d_device_identity = gpu.map(GpuSurface::device_identity);
        self.memory_mode = if gpu.is_some() {
            PipelineMemoryMode::GpuZeroCopy
        } else {
            PipelineMemoryMode::CpuPreprocessing
        };
        Ok(())
    }

    fn encode_gpu_assisted(&mut self, frame: &GpuSurface) -> Result<crate::HardwareEncodedFrame, String> {
        let nv12 = self.nv12_readback.read(frame)?;
        if self.force_rebuild
            || self.d3d_device_identity.is_some()
            || self.encoder.is_none()
            || (frame.width, frame.height) != (self.config.width, self.config.height)
        {
            self.rebuild(frame.width, frame.height, None)?;
        }
        self.memory_mode = PipelineMemoryMode::GpuAssisted;
        self.encoder
            .as_mut()
            .ok_or_else(|| "hardware encoder is shut down".to_owned())?
            .encode_nv12(&nv12)
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

    fn pipeline_memory_mode(&self) -> PipelineMemoryMode {
        self.memory_mode
    }

    fn configure(&mut self, config: VideoEncoderConfig) -> Result<(), String> {
        if !config.mode.requires_hardware() || config.mode.codec() != Some(config.codec) {
            return Err("Media Foundation hardware mode and codec do not match".to_owned());
        }
        self.config = config;
        self.direct_gpu_disabled = None;
        self.rebuild(config.width, config.height, None)
    }

    fn encode(&mut self, frame: VideoFrame<'_>) -> Result<Option<EncodedPacket>, String> {
        let packet = match frame.data {
            VideoFrameData::I420(yuv) => {
                if self.force_rebuild
                    || (frame.width, frame.height) != (self.config.width, self.config.height)
                    || self.d3d_device_identity.is_some()
                {
                    self.rebuild(frame.width, frame.height, None)?;
                }
                self.memory_mode = PipelineMemoryMode::CpuPreprocessing;
                let nv12 = i420_to_nv12(yuv);
                self.encoder
                    .as_mut()
                    .ok_or_else(|| "hardware encoder is shut down".to_owned())?
                    .encode_nv12(&nv12)?
            }
            VideoFrameData::D3D11Nv12(gpu) => {
                if self.direct_gpu_disabled == Some(gpu.device_identity()) {
                    let packet = self.encode_gpu_assisted(gpu)?;
                    return Ok(Some(EncodedPacket {
                        data: packet.data,
                        keyframe: packet.keyframe,
                    }));
                }
                let needs_gpu_rebuild = self.force_rebuild
                    || (frame.width, frame.height) != (self.config.width, self.config.height)
                    || self.d3d_device_identity != Some(gpu.device_identity());
                if needs_gpu_rebuild && let Err(error) = self.rebuild(frame.width, frame.height, Some(gpu)) {
                    self.direct_gpu_disabled = Some(gpu.device_identity());
                    tracing::warn!(%error, "direct Media Foundation GPU input is unavailable; using GPU-assisted readback");
                    return self.encode_gpu_assisted(gpu).map(|packet| {
                        Some(EncodedPacket {
                            data: packet.data,
                            keyframe: packet.keyframe,
                        })
                    });
                }
                self.memory_mode = PipelineMemoryMode::GpuZeroCopy;
                match self
                    .encoder
                    .as_mut()
                    .ok_or_else(|| "hardware encoder is shut down".to_owned())?
                    .encode_nv12_surface(gpu)
                {
                    Ok(packet) => packet,
                    Err(error) => {
                        self.direct_gpu_disabled = Some(gpu.device_identity());
                        tracing::warn!(%error, "hardware encoder rejected direct GPU input; using GPU-assisted readback");
                        self.encode_gpu_assisted(gpu)?
                    }
                }
            }
        };
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
        self.d3d_device_identity = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_preprocess::GpuBenchmarkPipeline;

    #[test]
    fn available_hardware_encoders_accept_the_gpu_pipeline() {
        let Ok(mut gpu) = GpuBenchmarkPipeline::new(640, 360, 640, 360, 30) else {
            println!("GPU pipeline unavailable on this test machine; skipping hardware-only assertion");
            return;
        };
        let bgra = vec![96_u8; 640 * 360 * 4];
        let surface = gpu
            .process_bgra(&bgra)
            .expect("GPU BGRA-to-NV12 preprocessing should work");
        let mut tested = 0;
        for (codec, mode) in [
            (VideoCodec::H264, EncoderMode::H264Hardware),
            (VideoCodec::Hevc, EncoderMode::HevcHardware),
            (VideoCodec::Av1, EncoderMode::Av1Hardware),
        ] {
            let Ok(mut encoder) = create_video_encoder(VideoEncoderConfig {
                codec,
                mode,
                width: 640,
                height: 360,
                max_fps: 30,
                bitrate_bps: 2_000_000,
            }) else {
                println!("{} hardware encoder unavailable", codec.label());
                continue;
            };
            let packet = encoder
                .encode(VideoFrame {
                    width: 640,
                    height: 360,
                    data: VideoFrameData::D3D11Nv12(&surface),
                })
                .expect("available hardware encoder should consume GPU-preprocessed NV12")
                .expect("hardware encoder should return a packet");
            assert!(!packet.data.is_empty());
            assert!(matches!(
                encoder.pipeline_memory_mode(),
                PipelineMemoryMode::GpuZeroCopy | PipelineMemoryMode::GpuAssisted
            ));
            encoder.shutdown();
            tested += 1;
        }
        println!("GPU pipeline tested with {tested} available hardware encoder(s)");
    }

    #[test]
    fn i420_to_nv12_preserves_plane_order() {
        let yuv = YUVBuffer::from_vec(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 4, 2);
        assert_eq!(i420_to_nv12(&yuv), vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 10, 12]);
    }
}
