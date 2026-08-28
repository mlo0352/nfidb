use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use apple_cf::cv::CVPixelBuffer;
use apple_cf::iosurface::{IOSurface, IOSurfaceLockOptions};
use nfidb_core::{
    AppConfig, AutoBenchmarkObservation, BrowserVideoCapabilities, EncodedVideoFrame, EncoderBackend,
    EncoderCapability, EncoderMode, KeyframeRequest, Metrics, PipelineMemoryMode, VideoCodec, VideoConfig,
    VideoRuntimeStatus, VideoSettingsRuntime, score_auto_candidate,
};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use parking_lot::{Condvar, Mutex};
use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::prelude::{
    PixelFormat, SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutputType,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::hardware::functional_probe;
use crate::videotoolbox_encoder::{EncodedSurface, VideoToolboxEncoder, VideoToolboxEncoderConfig};

#[derive(Debug, Clone)]
pub struct CaptureStatus {
    pub running: bool,
    pub source: String,
    pub encoder: String,
    pub error: Option<String>,
}

impl Default for CaptureStatus {
    fn default() -> Self {
        Self {
            running: false,
            source: "None".to_owned(),
            encoder: "No encoder active".to_owned(),
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
enum CaptureSource {
    Monitor(usize),
    TestPattern(u32, u32),
}

enum CapturedFrame {
    PixelBuffer(CVPixelBuffer),
    Bgra { width: u32, height: u32, bytes: Vec<u8> },
}

impl CapturedFrame {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::PixelBuffer(buffer) => (buffer.width() as u32, buffer.height() as u32),
            Self::Bgra { width, height, .. } => (*width, *height),
        }
    }
}

#[derive(Default)]
struct LatestFrame {
    frame: Mutex<Option<CapturedFrame>>,
    ready: Condvar,
    stopped: AtomicBool,
}

impl LatestFrame {
    fn submit(&self, frame: CapturedFrame, metrics: &Metrics) {
        let mut current = self.frame.lock();
        if current.replace(frame).is_some() {
            metrics.dropped_frame();
        }
        self.ready.notify_one();
    }

    fn take(&self) -> Option<CapturedFrame> {
        let mut current = self.frame.lock();
        while current.is_none() && !self.stopped.load(Ordering::Acquire) {
            self.ready.wait_for(&mut current, Duration::from_millis(250));
        }
        current.take()
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.ready.notify_all();
    }
}

#[derive(Clone)]
struct PipelineSelection {
    generation: u64,
    video: VideoConfig,
    active_mode: EncoderMode,
}

pub struct CaptureManager {
    stream: Mutex<Option<SCStream>>,
    producer_thread: Mutex<Option<JoinHandle<()>>>,
    encoder_thread: Mutex<Option<JoinHandle<()>>>,
    slot: Mutex<Arc<LatestFrame>>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    keyframe_request: KeyframeRequest,
    metrics: Arc<Metrics>,
    video: Mutex<VideoConfig>,
    browser: Mutex<BrowserVideoCapabilities>,
    capabilities: Vec<EncoderCapability>,
    learned: Mutex<Vec<AutoBenchmarkObservation>>,
    runtime: Mutex<VideoRuntimeStatus>,
    pipeline: Arc<Mutex<PipelineSelection>>,
    source: Mutex<Option<CaptureSource>>,
    status: Arc<Mutex<CaptureStatus>>,
}

impl CaptureManager {
    #[must_use]
    pub fn new(
        video_tx: broadcast::Sender<EncodedVideoFrame>,
        metrics: Arc<Metrics>,
        video: VideoConfig,
        capabilities: Vec<EncoderCapability>,
    ) -> Self {
        let learned = load_learned();
        let (active_mode, reason, encoder_name) =
            select_encoder(&video, &BrowserVideoCapabilities::default(), &capabilities, &learned).unwrap_or((
                EncoderMode::H264Software,
                "VideoToolbox discovery produced no functional hardware path; using compatibility fallback".to_owned(),
                "OpenH264 software encoder".to_owned(),
            ));
        let preset = video.active_preset();
        let codec = active_mode.codec().unwrap_or(VideoCodec::H264);
        Self {
            stream: Mutex::new(None),
            producer_thread: Mutex::new(None),
            encoder_thread: Mutex::new(None),
            slot: Mutex::new(Arc::new(LatestFrame::default())),
            video_tx,
            keyframe_request: KeyframeRequest::default(),
            metrics,
            video: Mutex::new(video.clone()),
            browser: Mutex::new(BrowserVideoCapabilities::default()),
            capabilities,
            learned: Mutex::new(learned),
            runtime: Mutex::new(VideoRuntimeStatus {
                requested_mode: video.encoder,
                active_mode,
                codec,
                backend: backend_for_mode(active_mode),
                encoder_name,
                hardware: active_mode.requires_hardware(),
                pipeline_memory_mode: memory_mode_for_mode(active_mode),
                output_width: 0,
                output_height: 0,
                target_fps: preset.max_fps,
                target_bitrate_bps: preset.bitrate_bps(codec),
                restart_count: 0,
                switching: false,
                auto_selection_reason: reason,
                last_error: None,
            }),
            pipeline: Arc::new(Mutex::new(PipelineSelection {
                generation: 0,
                video,
                active_mode,
            })),
            source: Mutex::new(None),
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        }
    }

    pub fn start_monitor(&self, index: usize) -> Result<(), String> {
        self.stop();
        let content = match SCShareableContent::get() {
            Ok(content) => content,
            Err(error) => {
                let message = permission_error(&error.to_string());
                self.status.lock().error = Some(message.clone());
                return Err(message);
            }
        };
        let display = content
            .displays()
            .into_iter()
            .nth(index)
            .ok_or_else(|| format!("ScreenCaptureKit display index {index} is unavailable"))?;
        let source_width = display.width();
        let source_height = display.height();
        let video = self.video.lock().clone();
        let preset = video.active_preset();
        let (width, height) = output_dimensions(source_width, source_height, preset.max_width);
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();
        let config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(video.cursor)
            .with_queue_depth(2)
            .with_fps(preset.max_fps);
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);
        self.start_encoder_thread(Arc::clone(&slot))?;

        let handler_slot = Arc::clone(&slot);
        let metrics = Arc::clone(&self.metrics);
        let status = Arc::clone(&self.status);
        let mut stream = SCStream::new(&filter, &config);
        let handler = stream.add_output_handler(
            move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Screen {
                    return;
                }
                if sample
                    .frame_status()
                    .is_some_and(|frame_status| frame_status != SCFrameStatus::Complete)
                {
                    return;
                }
                let Some(buffer) = sample.image_buffer() else {
                    status.lock().error = Some("ScreenCaptureKit returned a frame without a CVPixelBuffer".to_owned());
                    return;
                };
                let width = buffer.width() as u32;
                let height = buffer.height() as u32;
                metrics.captured(width, height);
                handler_slot.submit(CapturedFrame::PixelBuffer(buffer), &metrics);
            },
            SCStreamOutputType::Screen,
        );
        if handler.is_none() {
            slot.stop();
            self.join_encoder_thread();
            return Err("ScreenCaptureKit rejected NFiDB's screen output handler".to_owned());
        }
        if let Err(error) = stream.start_capture() {
            slot.stop();
            self.join_encoder_thread();
            return Err(permission_error(&error.to_string()));
        }
        *self.stream.lock() = Some(stream);
        *self.source.lock() = Some(CaptureSource::Monitor(index));
        *self.status.lock() = CaptureStatus {
            running: true,
            source: format!("Display {} ({}×{})", index + 1, source_width, source_height),
            encoder: self.runtime.lock().encoder_name.clone(),
            error: None,
        };
        Ok(())
    }

    pub fn start_test_pattern(&self) -> Result<(), String> {
        self.start_test_pattern_size(1280, 720)
    }

    pub fn start_test_pattern_size(&self, width: u32, height: u32) -> Result<(), String> {
        self.stop();
        let width = width.clamp(320, 3840) & !1;
        let height = height.clamp(180, 2160) & !1;
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);
        self.start_encoder_thread(Arc::clone(&slot))?;
        let producer_slot = Arc::clone(&slot);
        let metrics = Arc::clone(&self.metrics);
        let pipeline = Arc::clone(&self.pipeline);
        let producer = thread::Builder::new()
            .name("nfidb-test-pattern".to_owned())
            .spawn(move || test_pattern_loop(producer_slot, metrics, pipeline, width, height))
            .map_err(|error| error.to_string())?;
        *self.producer_thread.lock() = Some(producer);
        *self.source.lock() = Some(CaptureSource::TestPattern(width, height));
        *self.status.lock() = CaptureStatus {
            running: true,
            source: format!("Generated integrity test pattern ({width}×{height})"),
            encoder: self.runtime.lock().encoder_name.clone(),
            error: None,
        };
        Ok(())
    }

    fn start_encoder_thread(&self, slot: Arc<LatestFrame>) -> Result<(), String> {
        let video_tx = self.video_tx.clone();
        let metrics = Arc::clone(&self.metrics);
        let keyframe = self.keyframe_request.clone();
        let pipeline = Arc::clone(&self.pipeline);
        let status = Arc::clone(&self.status);
        let thread = thread::Builder::new()
            .name("nfidb-videotoolbox".to_owned())
            .spawn(move || encode_loop(slot, video_tx, metrics, keyframe, pipeline, status))
            .map_err(|error| error.to_string())?;
        *self.encoder_thread.lock() = Some(thread);
        Ok(())
    }

    fn join_encoder_thread(&self) {
        if let Some(thread) = self.encoder_thread.lock().take() {
            let _ = thread.join();
        }
    }

    pub fn stop(&self) {
        if let Some(stream) = self.stream.lock().take()
            && let Err(error) = stream.stop_capture()
        {
            tracing::warn!(%error, "ScreenCaptureKit shutdown reported an error");
        }
        self.slot.lock().stop();
        if let Some(thread) = self.producer_thread.lock().take() {
            let _ = thread.join();
        }
        self.join_encoder_thread();
        self.status.lock().running = false;
    }

    #[must_use]
    pub fn status(&self) -> CaptureStatus {
        self.status.lock().clone()
    }

    #[must_use]
    pub fn keyframe_request(&self) -> KeyframeRequest {
        self.keyframe_request.clone()
    }
}

impl VideoSettingsRuntime for CaptureManager {
    fn apply_video_settings(
        &self,
        settings: &VideoConfig,
        browser: &BrowserVideoCapabilities,
    ) -> Result<VideoRuntimeStatus, String> {
        settings.validate()?;
        let (active_mode, reason, encoder_name) =
            select_encoder(settings, browser, &self.capabilities, &self.learned.lock())?;
        if *self.video.lock() == *settings && self.runtime.lock().active_mode == active_mode {
            *self.browser.lock() = browser.clone();
            let mut runtime = self.runtime.lock();
            runtime.auto_selection_reason = reason;
            return Ok(runtime.clone());
        }
        preflight_encoder(settings, active_mode)?;
        let source = self.source.lock().clone();
        let preset = settings.active_preset();
        let codec = active_mode.codec().unwrap_or(VideoCodec::H264);
        let restart_count = self.runtime.lock().restart_count.saturating_add(1);
        *self.video.lock() = settings.clone();
        *self.browser.lock() = browser.clone();
        {
            let mut pipeline = self.pipeline.lock();
            pipeline.generation = pipeline.generation.wrapping_add(1);
            pipeline.video = settings.clone();
            pipeline.active_mode = active_mode;
        }
        *self.runtime.lock() = VideoRuntimeStatus {
            requested_mode: settings.encoder,
            active_mode,
            codec,
            backend: backend_for_mode(active_mode),
            encoder_name,
            hardware: active_mode.requires_hardware(),
            pipeline_memory_mode: memory_mode_for_mode(active_mode),
            output_width: 0,
            output_height: 0,
            target_fps: preset.max_fps,
            target_bitrate_bps: preset.bitrate_bps(codec),
            restart_count,
            switching: source.is_some(),
            auto_selection_reason: reason,
            last_error: None,
        };
        if let Some(source) = source {
            self.status.lock().encoder = format!("Switching to {}", active_mode.label());
            let result = match source {
                CaptureSource::Monitor(index) => self.start_monitor(index),
                CaptureSource::TestPattern(width, height) => self.start_test_pattern_size(width, height),
            };
            if let Err(error) = result {
                let mut runtime = self.runtime.lock();
                runtime.switching = false;
                runtime.last_error = Some(error.clone());
                return Err(error);
            }
        }
        self.keyframe_request.request();
        let mut runtime = self.runtime.lock();
        runtime.switching = false;
        Ok(runtime.clone())
    }

    fn video_runtime_status(&self) -> VideoRuntimeStatus {
        let mut runtime = self.runtime.lock().clone();
        let metrics = self.metrics.snapshot();
        runtime.output_width = metrics.output_width;
        runtime.output_height = metrics.output_height;
        runtime
    }

    fn encoder_capabilities(&self) -> Vec<EncoderCapability> {
        self.capabilities.clone()
    }

    fn request_video_keyframe(&self) {
        self.keyframe_request.request();
    }

    fn record_auto_benchmark(&self, mut observation: AutoBenchmarkObservation) -> Result<(), String> {
        if observation.mode == EncoderMode::Auto {
            return Err("benchmark observations must identify an actual encoder mode".to_owned());
        }
        let browser = self.browser.lock();
        if browser.user_agent.is_empty() || observation.receiver_runtime != browser.user_agent {
            return Err("benchmark receiver identity does not match the paired browser".to_owned());
        }
        let codec = observation
            .mode
            .codec()
            .ok_or_else(|| "benchmark mode has no codec".to_owned())?;
        if !browser.get(codec).presented || !observation.end_to_end_verified {
            return Err("an end-to-end benchmark requires verified decoded presentation".to_owned());
        }
        let candidate = self
            .capabilities
            .iter()
            .find(|candidate| candidate.id == observation.encoder_id && candidate.mode() == observation.mode)
            .ok_or_else(|| "benchmark encoder identity is not present on this Mac".to_owned())?;
        if !candidate.state.is_usable() {
            return Err("benchmark encoder is not functional".to_owned());
        }
        observation.schema_version = 1;
        observation.nfidb_version = env!("CARGO_PKG_VERSION").to_owned();
        observation.score = score_auto_candidate(observation.mode, &observation.metrics);
        let mut learned = self.learned.lock();
        learned.retain(|current| {
            !(current.receiver_runtime == observation.receiver_runtime
                && current.encoder_id == observation.encoder_id
                && current.profile == observation.profile
                && current.max_width == observation.max_width
                && current.requested_fps == observation.requested_fps)
        });
        learned.push(observation);
        if learned.len() > 128 {
            let remove = learned.len() - 128;
            learned.drain(..remove);
        }
        save_learned(&learned)
    }

    fn clear_auto_benchmarks(&self) -> Result<(), String> {
        self.learned.lock().clear();
        save_learned(&[])
    }

    fn auto_benchmark_results(&self) -> Vec<AutoBenchmarkObservation> {
        self.learned.lock().clone()
    }
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
    }
}

enum ActiveEncoder {
    Hardware(VideoToolboxEncoder),
    Software(Encoder),
}

impl ActiveEncoder {
    fn new(selection: &PipelineSelection, width: u32, height: u32) -> Result<Self, String> {
        let codec = selection.active_mode.codec().unwrap_or(VideoCodec::H264);
        let preset = selection.video.active_preset();
        if selection.active_mode == EncoderMode::H264Software {
            let config = EncoderConfig::new()
                .bitrate(BitRate::from_bps(preset.bitrate_bps(codec)))
                .max_frame_rate(FrameRate::from_hz(preset.max_fps as f32))
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
            let encoder =
                Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|error| error.to_string())?;
            Ok(Self::Software(encoder))
        } else {
            Ok(Self::Hardware(VideoToolboxEncoder::new(VideoToolboxEncoderConfig {
                codec,
                width,
                height,
                max_fps: preset.max_fps,
                bitrate_bps: preset.bitrate_bps(codec),
            })?))
        }
    }

    fn request_keyframe(&mut self) {
        match self {
            Self::Hardware(encoder) => encoder.request_keyframe(),
            Self::Software(encoder) => encoder.force_intra_frame(),
        }
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<Option<EncodedSurface>, String> {
        match self {
            Self::Hardware(encoder) => {
                let temporary;
                let surface = match frame {
                    CapturedFrame::PixelBuffer(buffer) => buffer
                        .io_surface()
                        .ok_or_else(|| "ScreenCaptureKit frame is not IOSurface-backed".to_owned())?,
                    CapturedFrame::Bgra { width, height, bytes } => {
                        temporary = bgra_iosurface(*width, *height, bytes)?;
                        temporary
                    }
                };
                encoder.encode(&surface).map(Some)
            }
            Self::Software(encoder) => {
                let (width, height) = frame.dimensions();
                let bytes = bgra_bytes(frame)?;
                let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&bytes, (width as usize, height as usize)));
                let bitstream = encoder.encode(&yuv).map_err(|error| error.to_string())?;
                if bitstream.frame_type() == FrameType::Skip {
                    return Ok(None);
                }
                Ok(Some(EncodedSurface {
                    data: bitstream.to_vec(),
                    keyframe: matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I),
                }))
            }
        }
    }
}

fn encode_loop(
    slot: Arc<LatestFrame>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    metrics: Arc<Metrics>,
    keyframe_request: KeyframeRequest,
    pipeline: Arc<Mutex<PipelineSelection>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    let mut encoder: Option<ActiveEncoder> = None;
    let mut generation = u64::MAX;
    let mut dimensions = (0, 0);
    let mut last_keyframe_at = Instant::now() - Duration::from_secs(10);
    while let Some(frame) = slot.take() {
        let selection = pipeline.lock().clone();
        let frame_dimensions = frame.dimensions();
        if selection.generation != generation || frame_dimensions != dimensions {
            generation = selection.generation;
            dimensions = frame_dimensions;
            match ActiveEncoder::new(&selection, dimensions.0, dimensions.1) {
                Ok(active) => encoder = Some(active),
                Err(error) => {
                    status.lock().error = Some(error);
                    encoder = None;
                    continue;
                }
            }
        }
        let Some(active) = encoder.as_mut() else { continue };
        if keyframe_request.take() || last_keyframe_at.elapsed() >= Duration::from_secs(5) {
            active.request_keyframe();
        }
        let started = Instant::now();
        match active.encode(&frame) {
            Ok(Some(packet)) => {
                let elapsed = started.elapsed();
                metrics.preprocessed(0);
                metrics.encoded(
                    packet.data.len(),
                    elapsed.as_micros() as u64,
                    dimensions.0,
                    dimensions.1,
                );
                if packet.keyframe {
                    metrics.encoded_keyframe();
                    last_keyframe_at = Instant::now();
                }
                let codec = selection.active_mode.codec().unwrap_or(VideoCodec::H264);
                let encoded = EncodedVideoFrame {
                    data: Arc::from(packet.data),
                    codec,
                    duration: Duration::from_secs_f64(1.0 / f64::from(selection.video.active_preset().max_fps.max(1))),
                    width: dimensions.0,
                    height: dimensions.1,
                    keyframe: packet.keyframe,
                };
                if video_tx.send(encoded).is_err() {
                    // No receiver is normal before the iPad connects.
                }
                status.lock().encoder = selection.active_mode.label().to_owned();
            }
            Ok(None) => metrics.dropped_frame(),
            Err(error) => status.lock().error = Some(error),
        }
    }
}

fn bgra_bytes(frame: &CapturedFrame) -> Result<Vec<u8>, String> {
    match frame {
        CapturedFrame::Bgra { bytes, .. } => Ok(bytes.clone()),
        CapturedFrame::PixelBuffer(buffer) => {
            let guard = buffer
                .lock(apple_cf::cv::CVPixelBufferLockFlags::READ_ONLY)
                .map_err(|status| format!("CVPixelBufferLockBaseAddress failed: {status}"))?;
            let width = guard.width();
            let height = guard.height();
            let row_bytes = width.saturating_mul(4);
            let stride = guard.bytes_per_row();
            let source = guard.as_slice();
            if stride == row_bytes {
                return Ok(source[..row_bytes.saturating_mul(height)].to_vec());
            }
            let mut packed = Vec::with_capacity(row_bytes.saturating_mul(height));
            for row in 0..height {
                let start = row.saturating_mul(stride);
                packed.extend_from_slice(&source[start..start + row_bytes]);
            }
            Ok(packed)
        }
    }
}

pub(crate) fn bgra_iosurface(width: u32, height: u32, bytes: &[u8]) -> Result<IOSurface, String> {
    let surface = IOSurface::create(width as usize, height as usize, u32::from_be_bytes(*b"BGRA"), 4)
        .ok_or_else(|| "IOSurfaceCreate failed for generated test frame".to_owned())?;
    {
        let mut guard = surface
            .lock(IOSurfaceLockOptions::NONE)
            .map_err(|status| format!("IOSurfaceLock failed: {status}"))?;
        let destination = guard
            .as_slice_mut()
            .ok_or_else(|| "IOSurface read-write lock did not expose writable memory".to_owned())?;
        let row_bytes = width as usize * 4;
        let stride = surface.bytes_per_row();
        for row in 0..height as usize {
            destination[row * stride..row * stride + row_bytes]
                .copy_from_slice(&bytes[row * row_bytes..(row + 1) * row_bytes]);
        }
    }
    Ok(surface)
}

fn test_pattern_loop(
    slot: Arc<LatestFrame>,
    metrics: Arc<Metrics>,
    pipeline: Arc<Mutex<PipelineSelection>>,
    source_width: u32,
    source_height: u32,
) {
    let mut frame_number = 0_u32;
    while !slot.stopped.load(Ordering::Acquire) {
        let selection = pipeline.lock().clone();
        let preset = selection.video.active_preset();
        let (width, height) = output_dimensions(source_width, source_height, preset.max_width);
        let started = Instant::now();
        let bytes = render_test_pattern(width, height, frame_number);
        metrics.captured(source_width, source_height);
        slot.submit(CapturedFrame::Bgra { width, height, bytes }, &metrics);
        frame_number = frame_number.wrapping_add(1);
        let interval = Duration::from_secs_f64(1.0 / f64::from(preset.max_fps.max(1)));
        thread::sleep(interval.saturating_sub(started.elapsed()));
    }
}

fn render_test_pattern(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; width as usize * height as usize * 4];
    let moving_x = frame.wrapping_mul(13) % width.max(1);
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            let grid = if x % 64 == 0 || y % 64 == 0 { 40 } else { 0 };
            let stroke = (x.abs_diff(moving_x) < 5).then_some(210).unwrap_or(0);
            bytes[offset] = ((x * 160 / width.max(1)) as u8)
                .saturating_add(grid)
                .saturating_add(stroke);
            bytes[offset + 1] = ((y * 150 / height.max(1)) as u8).saturating_add(grid);
            bytes[offset + 2] = 28_u8.saturating_add(grid);
            bytes[offset + 3] = 255;
        }
    }
    bytes
}

fn preflight_encoder(settings: &VideoConfig, mode: EncoderMode) -> Result<(), String> {
    let codec = mode
        .codec()
        .ok_or_else(|| "Auto is not a concrete encoder".to_owned())?;
    let preset = settings.active_preset();
    if mode == EncoderMode::H264Software {
        ActiveEncoder::new(
            &PipelineSelection {
                generation: 0,
                video: settings.clone(),
                active_mode: mode,
            },
            preset.max_width.min(1920),
            1080,
        )
        .map(|_| ())
    } else {
        functional_probe(
            codec,
            preset.max_width.min(1920),
            1080,
            preset.max_fps,
            preset.bitrate_bps(codec),
        )
    }
}

fn select_encoder(
    settings: &VideoConfig,
    browser: &BrowserVideoCapabilities,
    capabilities: &[EncoderCapability],
    learned: &[AutoBenchmarkObservation],
) -> Result<(EncoderMode, String, String), String> {
    let functional = |mode| {
        capabilities
            .iter()
            .find(|item| item.mode() == mode && item.state.is_usable())
    };
    if settings.encoder != EncoderMode::Auto {
        let codec = settings.encoder.codec().expect("manual mode has codec");
        if !browser.user_agent.is_empty() && !browser.get(codec).reported {
            return Err(format!(
                "the paired browser did not report {} receive support",
                codec.label()
            ));
        }
        let selected = functional(settings.encoder).ok_or_else(|| {
            capabilities
                .iter()
                .find(|item| item.mode() == settings.encoder)
                .and_then(|item| item.failure_reason.clone())
                .unwrap_or_else(|| format!("{} is unavailable on this Mac", settings.encoder.label()))
        })?;
        return Ok((
            settings.encoder,
            "The encoder was selected manually and is mutually supported".to_owned(),
            selected.encoder_name.clone(),
        ));
    }
    let measured = (!browser.user_agent.is_empty())
        .then(|| {
            capabilities
                .iter()
                .filter(|item| item.state.is_usable() && browser.get(item.codec).reported)
                .filter_map(|item| {
                    learned
                        .iter()
                        .filter(|result| {
                            result.nfidb_version == env!("CARGO_PKG_VERSION")
                                && result.receiver_runtime == browser.user_agent
                                && result.encoder_id == item.id
                                && result.profile == settings.profile
                                && result.end_to_end_verified
                                && result.score.passed_gates
                        })
                        .max_by(|left, right| {
                            left.score
                                .score
                                .unwrap_or_default()
                                .total_cmp(&right.score.score.unwrap_or_default())
                        })
                        .map(|result| (item, result))
                })
                .max_by(|(_, left), (_, right)| {
                    left.score
                        .score
                        .unwrap_or_default()
                        .total_cmp(&right.score.score.unwrap_or_default())
                })
        })
        .flatten();
    if let Some((selected, result)) = measured {
        return Ok((
            selected.mode(),
            format!(
                "{} won the verified end-to-end Auto benchmark with score {:.1}",
                selected.mode().label(),
                result.score.score.unwrap_or_default()
            ),
            selected.encoder_name.clone(),
        ));
    }
    let selected = if browser.hevc.reported {
        functional(EncoderMode::HevcHardware)
            .or_else(|| functional(EncoderMode::H264Hardware))
            .or_else(|| functional(EncoderMode::H264Software))
    } else {
        functional(EncoderMode::H264Hardware).or_else(|| functional(EncoderMode::H264Software))
    }
    .ok_or_else(|| "no functional video encoder is available".to_owned())?;
    let reason = if selected.mode() == EncoderMode::HevcHardware {
        "VideoToolbox HEVC is functional and the receiver reports HEVC; Auto will verify presentation before learning it".to_owned()
    } else if selected.mode() == EncoderMode::H264Hardware {
        "VideoToolbox H.264 is the safe accelerated path before an end-to-end benchmark".to_owned()
    } else {
        "Hardware encoding is unavailable; OpenH264 is the compatibility fallback".to_owned()
    };
    Ok((selected.mode(), reason, selected.encoder_name.clone()))
}

fn backend_for_mode(mode: EncoderMode) -> EncoderBackend {
    if mode == EncoderMode::H264Software {
        EncoderBackend::OpenH264Software
    } else {
        EncoderBackend::VideoToolboxHardware
    }
}

fn memory_mode_for_mode(mode: EncoderMode) -> PipelineMemoryMode {
    if mode == EncoderMode::H264Software {
        PipelineMemoryMode::CpuPreprocessing
    } else {
        PipelineMemoryMode::GpuZeroCopy
    }
}

fn output_dimensions(source_width: u32, source_height: u32, max_width: u32) -> (u32, u32) {
    let source_width = source_width.max(2) & !1;
    let source_height = source_height.max(2) & !1;
    if source_width <= max_width {
        return (source_width, source_height);
    }
    let width = max_width.max(2) & !1;
    let height = (((u64::from(source_height) * u64::from(width)) / u64::from(source_width)) as u32).max(2) & !1;
    (width, height)
}

fn permission_error(error: &str) -> String {
    format!(
        "{error}. Allow NFiDB in System Settings > Privacy & Security > Screen & System Audio Recording, then try again."
    )
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LearnedFile {
    schema_version: u32,
    results: Vec<AutoBenchmarkObservation>,
}

fn learned_path() -> Option<PathBuf> {
    AppConfig::path()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("codec-benchmarks.json")))
}

fn load_learned() -> Vec<AutoBenchmarkObservation> {
    learned_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<LearnedFile>(&bytes).ok())
        .map(|file| file.results)
        .unwrap_or_default()
}

fn save_learned(results: &[AutoBenchmarkObservation]) -> Result<(), String> {
    let path = learned_path().ok_or_else(|| "NFiDB configuration directory is unavailable".to_owned())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&LearnedFile {
        schema_version: 1,
        results: results.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_dimensions_preserve_aspect_ratio_and_evenness() {
        assert_eq!(output_dimensions(3840, 2160, 1920), (1920, 1080));
        let (width, height) = output_dimensions(3457, 2161, 1280);
        assert_eq!(width % 2, 0);
        assert_eq!(height % 2, 0);
    }

    #[test]
    fn latest_frame_replaces_stale_work() {
        let slot = LatestFrame::default();
        let metrics = Metrics::default();
        slot.submit(
            CapturedFrame::Bgra {
                width: 2,
                height: 2,
                bytes: vec![0; 16],
            },
            &metrics,
        );
        slot.submit(
            CapturedFrame::Bgra {
                width: 4,
                height: 2,
                bytes: vec![0; 32],
            },
            &metrics,
        );
        assert_eq!(slot.take().unwrap().dimensions(), (4, 2));
    }
}
