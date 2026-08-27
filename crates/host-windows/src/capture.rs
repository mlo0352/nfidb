use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use nfidb_core::{
    AutoBenchmarkObservation, BrowserVideoCapabilities, EncodedVideoFrame, EncoderBackend, EncoderCapability,
    EncoderMode, KeyframeRequest, Metrics, PipelineMemoryMode, VideoCodec, VideoConfig, VideoRuntimeStatus,
    VideoSettingsRuntime, score_auto_candidate,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use parking_lot::{Condvar, Mutex};
use tokio::sync::broadcast;
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings,
    SecondaryWindowSettings, Settings,
};

use crate::gpu_preprocess::{GpuSurface, GpuSurfacePool, GpuVideoProcessor, copy_capture_surface, read_bgra};
use crate::{VideoEncoderConfig, VideoFrame, VideoFrameData, create_video_encoder};

#[derive(Debug, Clone)]
pub struct CaptureStatus {
    pub running: bool,
    pub source: String,
    pub encoder: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
enum CaptureSource {
    Monitor(usize),
    TestPattern,
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

#[derive(Debug)]
struct RawFrame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

enum CapturedFrame {
    Cpu(RawFrame),
    Gpu(Arc<GpuSurface>),
}

enum PreparedFrameData {
    I420(YUVBuffer),
    D3D11Nv12(Arc<GpuSurface>),
}

struct PreparedFrame {
    width: u32,
    height: u32,
    data: PreparedFrameData,
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

#[derive(Default)]
struct LatestPreparedFrame {
    frame: Mutex<Option<PreparedFrame>>,
    ready: Condvar,
    stopped: AtomicBool,
}

impl LatestPreparedFrame {
    fn submit(&self, frame: PreparedFrame, metrics: &Metrics) {
        let mut current = self.frame.lock();
        if current.replace(frame).is_some() {
            metrics.dropped_frame();
        }
        self.ready.notify_one();
    }

    fn take(&self) -> Option<PreparedFrame> {
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
struct CaptureFlags {
    slot: Arc<LatestFrame>,
    metrics: Arc<Metrics>,
    max_fps: Arc<AtomicU32>,
    hardware_active: Arc<AtomicBool>,
    status: Arc<Mutex<CaptureStatus>>,
}

struct EncodeLoopContext {
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    metrics: Arc<Metrics>,
    keyframe_request: KeyframeRequest,
    pipeline: Arc<Mutex<PipelineSelection>>,
    max_width: Arc<AtomicU32>,
    memory_mode: Arc<AtomicU32>,
    status: Arc<Mutex<CaptureStatus>>,
}

struct ScreenCapture {
    flags: CaptureFlags,
    scratch: Vec<u8>,
    gpu_pool: GpuSurfacePool,
    gpu_capture_unavailable: bool,
    last_frame: Instant,
}

impl GraphicsCaptureApiHandler for ScreenCapture {
    type Flags = CaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            flags: ctx.flags,
            scratch: Vec::new(),
            gpu_pool: GpuSurfacePool::default(),
            gpu_capture_unavailable: false,
            last_frame: Instant::now() - Duration::from_secs(1),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let min_interval = Duration::from_secs_f64(1.0 / f64::from(self.flags.max_fps.load(Ordering::Relaxed).max(1)));
        if self.last_frame.elapsed() < min_interval {
            return Ok(());
        }
        self.last_frame = Instant::now();
        let source_width = frame.width();
        let width = source_width & !1;
        let height = frame.height() & !1;
        if width < 2 || height < 2 {
            return Ok(());
        }
        if self.flags.hardware_active.load(Ordering::Relaxed) && !self.gpu_capture_unavailable {
            match copy_capture_surface(
                &mut self.gpu_pool,
                frame.device(),
                frame.device_context(),
                frame.as_raw_texture(),
                width,
                height,
                frame.desc().Format,
            ) {
                Ok(Some(surface)) => {
                    self.flags.metrics.captured(width, height);
                    self.flags.slot.submit(CapturedFrame::Gpu(surface), &self.flags.metrics);
                    return Ok(());
                }
                Ok(None) => {
                    self.flags.metrics.dropped_frame();
                    return Ok(());
                }
                Err(error) => {
                    self.gpu_capture_unavailable = true;
                    self.flags.status.lock().error = Some(format!(
                        "GPU capture copy is unavailable; using the CPU compatibility path: {error}"
                    ));
                }
            }
        }
        let buffer = frame.buffer().map_err(|error| error.to_string())?;
        let bytes = buffer.as_nopadding_buffer(&mut self.scratch);
        let source_stride = source_width as usize * 4;
        let row_bytes = width as usize * 4;
        let mut bgra = Vec::with_capacity(row_bytes * height as usize);
        for row in bytes.chunks_exact(source_stride).take(height as usize) {
            bgra.extend_from_slice(&row[..row_bytes]);
        }
        self.flags.metrics.captured(width, height);
        self.flags.slot.submit(
            CapturedFrame::Cpu(RawFrame { width, height, bgra }),
            &self.flags.metrics,
        );
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.flags.slot.stop();
        Ok(())
    }
}

type Control = CaptureControl<ScreenCapture, String>;

#[derive(Clone)]
struct PipelineSelection {
    generation: u64,
    video: VideoConfig,
    active_mode: EncoderMode,
}

pub struct CaptureManager {
    control: Mutex<Option<Control>>,
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
    live_max_fps: Arc<AtomicU32>,
    live_max_width: Arc<AtomicU32>,
    live_hardware: Arc<AtomicBool>,
    live_memory_mode: Arc<AtomicU32>,
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
        let learned = crate::learned::load();
        let (active_mode, reason, encoder_name) =
            select_encoder(&video, &BrowserVideoCapabilities::default(), &capabilities, &learned).unwrap_or((
                EncoderMode::H264Software,
                "hardware discovery did not produce a functional candidate; using compatibility fallback".to_owned(),
                "OpenH264 software encoder".to_owned(),
            ));
        let preset = video.active_preset();
        let codec = active_mode.codec().unwrap_or(VideoCodec::H264);
        let pipeline = Arc::new(Mutex::new(PipelineSelection {
            generation: 0,
            video: video.clone(),
            active_mode,
        }));
        Self {
            control: Mutex::new(None),
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
                backend: if active_mode == EncoderMode::H264Software {
                    EncoderBackend::OpenH264Software
                } else {
                    EncoderBackend::MediaFoundationHardware
                },
                encoder_name,
                hardware: active_mode != EncoderMode::H264Software,
                pipeline_memory_mode: PipelineMemoryMode::CpuPreprocessing,
                output_width: 0,
                output_height: 0,
                target_fps: preset.max_fps,
                target_bitrate_bps: preset.bitrate_bps(codec),
                restart_count: 0,
                switching: false,
                auto_selection_reason: reason,
                last_error: None,
            }),
            pipeline,
            live_max_fps: Arc::new(AtomicU32::new(preset.max_fps)),
            live_max_width: Arc::new(AtomicU32::new(preset.max_width)),
            live_hardware: Arc::new(AtomicBool::new(active_mode.requires_hardware())),
            live_memory_mode: Arc::new(AtomicU32::new(memory_mode_value(PipelineMemoryMode::CpuPreprocessing))),
            source: Mutex::new(None),
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        }
    }

    pub fn start_monitor(&self, index: usize) -> Result<(), String> {
        self.stop();
        *self.source.lock() = Some(CaptureSource::Monitor(index));
        let monitor = Monitor::from_index(index).map_err(|error| error.to_string())?;
        let width = monitor.width().map_err(|error| error.to_string())?;
        let height = monitor.height().map_err(|error| error.to_string())?;
        let source = monitor.name().unwrap_or_else(|_| format!("Display {index}"));
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);

        let encoder_slot = Arc::clone(&slot);
        let encoder_tx = self.video_tx.clone();
        let encoder_metrics = Arc::clone(&self.metrics);
        let pipeline = Arc::clone(&self.pipeline);
        let max_width = Arc::clone(&self.live_max_width);
        let memory_mode = Arc::clone(&self.live_memory_mode);
        let status = Arc::clone(&self.status);
        let encoder_thread = thread::Builder::new()
            .name("nfidb-encoder".to_owned())
            .spawn({
                let keyframe_request = self.keyframe_request.clone();
                move || {
                    encode_loop(
                        encoder_slot,
                        EncodeLoopContext {
                            video_tx: encoder_tx,
                            metrics: encoder_metrics,
                            keyframe_request,
                            pipeline,
                            max_width,
                            memory_mode,
                            status,
                        },
                    )
                }
            })
            .map_err(|error| error.to_string())?;

        let settings = Settings::new(
            monitor,
            if self.video.lock().cursor {
                CursorCaptureSettings::WithCursor
            } else {
                CursorCaptureSettings::WithoutCursor
            },
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            // The WGC minimum-update-interval API is unavailable on some supported Windows 11
            // builds. Frame-rate limiting already happens in `on_frame_arrived`, so use the
            // platform default here for compatibility.
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            CaptureFlags {
                slot,
                metrics: Arc::clone(&self.metrics),
                max_fps: Arc::clone(&self.live_max_fps),
                hardware_active: Arc::clone(&self.live_hardware),
                status: Arc::clone(&self.status),
            },
        );
        match ScreenCapture::start_free_threaded(settings) {
            Ok(control) => {
                *self.control.lock() = Some(control);
                *self.encoder_thread.lock() = Some(encoder_thread);
                *self.status.lock() = CaptureStatus {
                    running: true,
                    source: format!("{source} ({width}×{height})"),
                    encoder: self.runtime.lock().encoder_name.clone(),
                    error: None,
                };
                Ok(())
            }
            Err(error) => {
                self.slot.lock().stop();
                let _ = encoder_thread.join();
                let message = error.to_string();
                self.status.lock().error = Some(message.clone());
                Err(message)
            }
        }
    }

    pub fn start_test_pattern(&self) -> Result<(), String> {
        self.start_test_pattern_size(1280, 720)
    }

    pub fn start_test_pattern_size(&self, width: u32, height: u32) -> Result<(), String> {
        self.stop();
        let width = width.clamp(320, 3840) & !1;
        let height = height.clamp(180, 2160) & !1;
        *self.source.lock() = Some(CaptureSource::TestPattern);
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);
        let encoder_slot = Arc::clone(&slot);
        let encoder_tx = self.video_tx.clone();
        let encoder_metrics = Arc::clone(&self.metrics);
        let pipeline = Arc::clone(&self.pipeline);
        let max_width = Arc::clone(&self.live_max_width);
        let memory_mode = Arc::clone(&self.live_memory_mode);
        let status = Arc::clone(&self.status);
        let encoder_thread = thread::Builder::new()
            .name("nfidb-encoder".to_owned())
            .spawn({
                let keyframe_request = self.keyframe_request.clone();
                move || {
                    encode_loop(
                        encoder_slot,
                        EncodeLoopContext {
                            video_tx: encoder_tx,
                            metrics: encoder_metrics,
                            keyframe_request,
                            pipeline,
                            max_width,
                            memory_mode,
                            status,
                        },
                    )
                }
            })
            .map_err(|error| error.to_string())?;
        *self.encoder_thread.lock() = Some(encoder_thread);
        let pattern_slot = Arc::clone(&slot);
        let pattern_metrics = Arc::clone(&self.metrics);
        let producer_thread = thread::Builder::new()
            .name("nfidb-test-pattern".to_owned())
            .spawn({
                let max_fps = Arc::clone(&self.live_max_fps);
                move || test_pattern_loop(pattern_slot, pattern_metrics, max_fps, width, height)
            })
            .map_err(|error| error.to_string())?;
        *self.producer_thread.lock() = Some(producer_thread);
        *self.status.lock() = CaptureStatus {
            running: true,
            source: format!("Generated integrity test pattern ({width}×{height})"),
            encoder: self.runtime.lock().encoder_name.clone(),
            error: None,
        };
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(control) = self.control.lock().take()
            && let Err(error) = control.stop()
        {
            tracing::warn!(%error, "capture shutdown reported an error");
        }
        self.slot.lock().stop();
        if let Some(thread) = self.producer_thread.lock().take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.encoder_thread.lock().take() {
            let _ = thread.join();
        }
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
        let (mut active_mode, mut reason, mut encoder_name) =
            select_encoder(settings, browser, &self.capabilities, &self.learned.lock())?;
        let preset = settings.active_preset();
        if *self.video.lock() == *settings && self.runtime.lock().active_mode == active_mode {
            *self.browser.lock() = browser.clone();
            let mut runtime = self.runtime.lock();
            runtime.auto_selection_reason = reason;
            runtime.encoder_name = encoder_name;
            return Ok(runtime.clone());
        }
        let source = self.source.lock().clone();
        if source.is_some() {
            let snapshot = self.metrics.snapshot();
            let source_width = snapshot.source_width.max(preset.max_width.min(1920));
            let source_height = snapshot.source_height.max((source_width * 9 / 16).max(2));
            if let Err(error) = preflight_encoder(settings, active_mode, source_width, source_height) {
                if settings.encoder != EncoderMode::Auto {
                    return Err(format!(
                        "{} initialization failed; the existing video path was kept: {error}",
                        active_mode.label()
                    ));
                }
                let mut fallback = None;
                for mode in [EncoderMode::H264Hardware, EncoderMode::H264Software] {
                    if mode == active_mode
                        || !self
                            .capabilities
                            .iter()
                            .any(|candidate| candidate.mode() == mode && candidate.state.is_usable())
                    {
                        continue;
                    }
                    if preflight_encoder(settings, mode, source_width, source_height).is_ok() {
                        fallback = self
                            .capabilities
                            .iter()
                            .find(|candidate| candidate.mode() == mode && candidate.state.is_usable())
                            .map(|candidate| (mode, candidate.encoder_name.clone()));
                        break;
                    }
                }
                let Some((fallback_mode, fallback_name)) = fallback else {
                    return Err(format!(
                        "{} initialization failed and no compatibility fallback initialized: {error}",
                        active_mode.label()
                    ));
                };
                reason = format!(
                    "{} failed its live initialization test ({error}); Auto returned to {}",
                    active_mode.label(),
                    fallback_mode.label()
                );
                active_mode = fallback_mode;
                encoder_name = fallback_name;
            }
        }
        let codec = active_mode.codec().unwrap_or(VideoCodec::H264);
        let cursor_changed = self.video.lock().cursor != settings.cursor;
        self.metrics.reset_video_latency_samples();
        let restart_count = self.runtime.lock().restart_count.saturating_add(1);
        *self.video.lock() = settings.clone();
        *self.browser.lock() = browser.clone();
        self.live_max_fps.store(preset.max_fps, Ordering::Relaxed);
        self.live_max_width.store(preset.max_width, Ordering::Relaxed);
        self.live_hardware
            .store(active_mode.requires_hardware(), Ordering::Relaxed);
        self.live_memory_mode.store(
            memory_mode_value(PipelineMemoryMode::CpuPreprocessing),
            Ordering::Relaxed,
        );
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
            backend: if active_mode == EncoderMode::H264Software {
                EncoderBackend::OpenH264Software
            } else {
                EncoderBackend::MediaFoundationHardware
            },
            encoder_name,
            hardware: active_mode != EncoderMode::H264Software,
            pipeline_memory_mode: PipelineMemoryMode::CpuPreprocessing,
            output_width: 0,
            output_height: 0,
            target_fps: preset.max_fps,
            target_bitrate_bps: preset.bitrate_bps(codec),
            restart_count,
            switching: source.is_some(),
            auto_selection_reason: reason,
            last_error: None,
        };
        // Cursor inclusion is a WGC session property. Every encoder/quality
        // setting is consumed by the running pipeline without restarting WGC.
        if cursor_changed
            && let Some(CaptureSource::Monitor(index)) = source
            && let Err(error) = self.start_monitor(index)
        {
            let mut runtime = self.runtime.lock();
            runtime.switching = false;
            runtime.last_error = Some(error.clone());
            return Err(error);
        }
        self.status.lock().encoder = format!("Switching to {}", active_mode.label());
        self.keyframe_request.request();
        let mut runtime = self.runtime.lock();
        runtime.switching = false;
        Ok(runtime.clone())
    }

    fn video_runtime_status(&self) -> VideoRuntimeStatus {
        let mut runtime = self.runtime.lock().clone();
        let snapshot = self.metrics.snapshot();
        runtime.output_width = snapshot.output_width;
        runtime.output_height = snapshot.output_height;
        runtime.pipeline_memory_mode = memory_mode_from_value(self.live_memory_mode.load(Ordering::Relaxed));
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
            .ok_or_else(|| "benchmark mode does not have a codec".to_owned())?;
        if !browser.get(codec).presented || !observation.end_to_end_verified {
            return Err("an end-to-end benchmark requires verified decoded presentation".to_owned());
        }
        let candidate = self
            .capabilities
            .iter()
            .find(|candidate| candidate.id == observation.encoder_id && candidate.mode() == observation.mode)
            .ok_or_else(|| "benchmark encoder identity is not present on this PC".to_owned())?;
        if !candidate.state.is_usable() {
            return Err("benchmark encoder is not functional".to_owned());
        }
        if observation.metrics.requested_fps <= 0.0
            || observation.metrics.requested_fps > 120.0
            || observation.max_width > 7680
            || observation.max_width < 320
            || [
                observation.metrics.encoded_fps,
                observation.metrics.encode_mean_ms,
                observation.metrics.encode_p95_ms,
                observation.metrics.preprocess_mean_ms,
                observation.metrics.preprocess_p95_ms,
                observation.metrics.actual_mbps,
                observation.metrics.drop_percent,
            ]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err("benchmark observation contains invalid metrics".to_owned());
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
        crate::learned::save(&learned)
    }

    fn clear_auto_benchmarks(&self) -> Result<(), String> {
        self.learned.lock().clear();
        crate::learned::save(&[])
    }

    fn auto_benchmark_results(&self) -> Vec<AutoBenchmarkObservation> {
        self.learned.lock().clone()
    }
}

fn preflight_encoder(
    settings: &VideoConfig,
    mode: EncoderMode,
    source_width: u32,
    source_height: u32,
) -> Result<(), String> {
    let codec = mode
        .codec()
        .ok_or_else(|| "Auto is not a concrete encoder".to_owned())?;
    let preset = settings.active_preset();
    let (width, height) = output_dimensions(source_width, source_height, preset.max_width);
    let mut encoder = create_video_encoder(VideoEncoderConfig {
        codec,
        mode,
        width,
        height,
        max_fps: preset.max_fps,
        bitrate_bps: preset.bitrate_bps(codec),
    })?;
    encoder.request_keyframe()?;
    let yuv = YUVBuffer::new(width as usize, height as usize);
    let result = encoder.encode(VideoFrame {
        width,
        height,
        data: VideoFrameData::I420(&yuv),
    });
    encoder.shutdown();
    match result? {
        Some(packet) if !packet.data.is_empty() => Ok(()),
        _ => Err(format!("{} returned no encoded preflight frame", mode.label())),
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

const fn memory_mode_value(mode: PipelineMemoryMode) -> u32 {
    match mode {
        PipelineMemoryMode::GpuZeroCopy => 0,
        PipelineMemoryMode::GpuAssisted => 1,
        PipelineMemoryMode::CpuCopy => 2,
        PipelineMemoryMode::CpuPreprocessing => 3,
    }
}

const fn memory_mode_from_value(value: u32) -> PipelineMemoryMode {
    match value {
        0 => PipelineMemoryMode::GpuZeroCopy,
        1 => PipelineMemoryMode::GpuAssisted,
        2 => PipelineMemoryMode::CpuCopy,
        _ => PipelineMemoryMode::CpuPreprocessing,
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
            .find(|candidate| candidate.mode() == mode && candidate.state.is_usable())
    };
    let measured = if settings.encoder == EncoderMode::Auto && !browser.user_agent.is_empty() {
        capabilities
            .iter()
            .filter(|candidate| candidate.state.is_usable() && browser.get(candidate.codec).reported)
            .filter_map(|candidate| {
                learned
                    .iter()
                    .filter(|result| {
                        result.schema_version == 1
                            && result.nfidb_version == env!("CARGO_PKG_VERSION")
                            && result.receiver_runtime == browser.user_agent
                            && result.encoder_id == candidate.id
                            && result.profile == settings.profile
                            && result.max_width == settings.active_preset().max_width
                            && result.requested_fps == settings.active_preset().max_fps
                            && result.end_to_end_verified
                            && result.score.passed_gates
                    })
                    .max_by(|left, right| {
                        left.score
                            .score
                            .unwrap_or_default()
                            .total_cmp(&right.score.score.unwrap_or_default())
                    })
                    .map(|result| (candidate, result))
            })
            .max_by(|(_, left), (_, right)| {
                left.score
                    .score
                    .unwrap_or_default()
                    .total_cmp(&right.score.score.unwrap_or_default())
            })
    } else {
        None
    };
    let selected = if settings.encoder != EncoderMode::Auto {
        let codec = settings.encoder.codec().expect("manual mode has a codec");
        if !browser.user_agent.is_empty() && !browser.get(codec).reported {
            return Err(format!(
                "{} cannot be selected because the paired browser did not report {} receive support",
                settings.encoder.label(),
                codec.label()
            ));
        }
        functional(settings.encoder).ok_or_else(|| {
            capabilities
                .iter()
                .find(|candidate| candidate.mode() == settings.encoder)
                .and_then(|candidate| candidate.failure_reason.clone())
                .unwrap_or_else(|| format!("{} is unavailable on this PC", settings.encoder.label()))
        })?
    } else if let Some((candidate, _)) = measured {
        candidate
    } else if browser.hevc.reported {
        functional(EncoderMode::HevcHardware)
            .or_else(|| functional(EncoderMode::H264Hardware))
            .or_else(|| functional(EncoderMode::H264Software))
            .ok_or_else(|| "no functional video encoder is available".to_owned())?
    } else {
        functional(EncoderMode::H264Hardware)
            .or_else(|| functional(EncoderMode::H264Software))
            .ok_or_else(|| "no functional H.264 encoder is available".to_owned())?
    };
    let mode = selected.mode();
    let reason = if settings.encoder != EncoderMode::Auto {
        format!("{} was selected manually and is mutually supported", mode.label())
    } else if let Some((_, result)) = measured {
        format!(
            "{} won the verified Auto benchmark with score {:.1}; it passed latency, frame-rate, reliability, and presentation gates",
            mode.label(),
            result.score.score.unwrap_or_default()
        )
    } else if mode == EncoderMode::HevcHardware {
        "HEVC hardware is functional and the receiver reports HEVC; Auto will verify presentation before learning this path".to_owned()
    } else if mode == EncoderMode::H264Hardware {
        "H.264 hardware is the safe accelerated provisional path before an end-to-end benchmark".to_owned()
    } else {
        "hardware encoding is unavailable; OpenH264 is the universal compatibility fallback".to_owned()
    };
    Ok((mode, reason, selected.encoder_name.clone()))
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
    }
}

fn encode_loop(slot: Arc<LatestFrame>, context: EncodeLoopContext) {
    let EncodeLoopContext {
        video_tx,
        metrics,
        keyframe_request,
        pipeline,
        max_width,
        memory_mode,
        status,
    } = context;
    let prepared = Arc::new(LatestPreparedFrame::default());
    let preprocess_output = Arc::clone(&prepared);
    let preprocess_metrics = Arc::clone(&metrics);
    let preprocess_status = Arc::clone(&status);
    let preprocess_pipeline = Arc::clone(&pipeline);
    let preprocess_thread = match thread::Builder::new()
        .name("nfidb-preprocess".to_owned())
        .spawn(move || {
            preprocess_loop(
                slot,
                preprocess_output,
                preprocess_metrics,
                max_width,
                preprocess_pipeline,
                preprocess_status,
            )
        }) {
        Ok(thread) => thread,
        Err(error) => {
            status.lock().error = Some(format!("video preprocessing thread failed: {error}"));
            return;
        }
    };
    encode_video_loop(
        Arc::clone(&prepared),
        video_tx,
        metrics,
        keyframe_request,
        pipeline,
        memory_mode,
        status,
    );
    let _ = preprocess_thread.join();
}

fn encode_video_loop(
    prepared: Arc<LatestPreparedFrame>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    metrics: Arc<Metrics>,
    keyframe_request: KeyframeRequest,
    pipeline: Arc<Mutex<PipelineSelection>>,
    memory_mode: Arc<AtomicU32>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    const RECOVERY_KEYFRAME_INTERVAL: Duration = Duration::from_secs(5);
    let mut encoder: Option<Box<dyn crate::VideoEncoder>> = None;
    let mut generation = u64::MAX;
    let mut last_sent_at: Option<Instant> = None;
    let mut last_keyframe_at: Option<Instant> = None;
    while let Some(frame) = prepared.take() {
        let selection = pipeline.lock().clone();
        let mode = selection.active_mode;
        let codec = mode.codec().unwrap_or(VideoCodec::H264);
        let preset = selection.video.active_preset();
        let max_fps = preset.max_fps;
        let bitrate_bps = preset.bitrate_bps(codec);
        if selection.generation != generation {
            if let Some(mut previous) = encoder.take() {
                previous.shutdown();
            }
            generation = selection.generation;
            last_sent_at = None;
            last_keyframe_at = None;
        }
        if encoder.is_none() {
            match create_video_encoder(VideoEncoderConfig {
                codec,
                mode,
                width: frame.width,
                height: frame.height,
                max_fps,
                bitrate_bps,
            }) {
                Ok(created) => {
                    status.lock().encoder = created.name().to_owned();
                    encoder = Some(created);
                }
                Err(error) => {
                    status.lock().error = Some(format!("{} initialization failed: {error}", mode.label()));
                    continue;
                }
            }
        }
        // The connection requests its own startup IDR. Software H.264 also gets
        // an infrequent recovery IDR; hardware paths are rebuilt only on an
        // explicit receiver request to avoid needless vendor-MFT churn.
        let recovery_keyframe_due = mode == EncoderMode::H264Software
            && last_keyframe_at.is_some_and(|last_keyframe| last_keyframe.elapsed() >= RECOVERY_KEYFRAME_INTERVAL);
        if (keyframe_request.take() || recovery_keyframe_due)
            && let Err(error) = encoder.as_mut().expect("encoder initialized").request_keyframe()
        {
            status.lock().error = Some(format!("{} keyframe request failed: {error}", mode.label()));
        }
        let started = Instant::now();
        let frame_data = match &frame.data {
            PreparedFrameData::I420(yuv) => VideoFrameData::I420(yuv),
            PreparedFrameData::D3D11Nv12(surface) => VideoFrameData::D3D11Nv12(surface),
        };
        let encoded = {
            let active_encoder = encoder.as_mut().expect("encoder initialized");
            let result = active_encoder.encode(VideoFrame {
                width: frame.width,
                height: frame.height,
                data: frame_data,
            });
            memory_mode.store(
                memory_mode_value(active_encoder.pipeline_memory_mode()),
                Ordering::Relaxed,
            );
            result
        };
        match encoded {
            Ok(Some(packet)) => {
                metrics.encoded(
                    packet.data.len(),
                    started.elapsed().as_micros() as u64,
                    frame.width,
                    frame.height,
                );
                let sent_at = Instant::now();
                if packet.keyframe {
                    last_keyframe_at = Some(sent_at);
                    metrics.encoded_keyframe();
                }
                let nominal_duration = Duration::from_secs_f64(1.0 / f64::from(max_fps.max(1)));
                let duration = last_sent_at
                    .replace(sent_at)
                    .map_or(nominal_duration, |previous| sent_at.duration_since(previous));
                let _ = video_tx.send(EncodedVideoFrame {
                    data: Arc::from(packet.data),
                    codec,
                    // RTP timestamps must follow the frames we actually encode. When the
                    // bounded latest-frame queue sheds work, using the requested frame
                    // rate here would make media time run behind wall time and grow lag.
                    duration,
                    width: frame.width,
                    height: frame.height,
                    keyframe: packet.keyframe,
                });
            }
            Ok(None) => {}
            Err(error) => {
                status.lock().error = Some(format!("{} encode failed: {error}", mode.label()));
                encoder.as_mut().expect("encoder initialized").shutdown();
                encoder = None;
            }
        }
    }
    if let Some(mut encoder) = encoder {
        encoder.shutdown();
    }
}

fn preprocess_loop(
    slot: Arc<LatestFrame>,
    output: Arc<LatestPreparedFrame>,
    metrics: Arc<Metrics>,
    max_width: Arc<AtomicU32>,
    pipeline: Arc<Mutex<PipelineSelection>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
    let mut resizer = Resizer::new();
    let mut gpu_processor: Option<GpuVideoProcessor> = None;
    let mut gpu_failed_generation = None;
    while let Some(frame) = slot.take() {
        let started = Instant::now();
        let selection = pipeline.lock().clone();
        if let CapturedFrame::Gpu(surface) = &frame
            && selection.active_mode.requires_hardware()
            && gpu_failed_generation != Some(selection.generation)
        {
            let (width, height) = output_dimensions(surface.width, surface.height, max_width.load(Ordering::Relaxed));
            let fps = selection.video.active_preset().max_fps;
            let needs_rebuild = gpu_processor
                .as_ref()
                .is_none_or(|processor| !processor.matches(surface, width, height, fps));
            if needs_rebuild {
                gpu_processor = match GpuVideoProcessor::new(surface, width, height, fps) {
                    Ok(processor) => Some(processor),
                    Err(error) => {
                        gpu_failed_generation = Some(selection.generation);
                        status.lock().error = Some(format!(
                            "GPU preprocessing is unavailable; using CPU preprocessing: {error}"
                        ));
                        None
                    }
                };
            }
            if let Some(processor) = gpu_processor.as_mut() {
                match processor.process(surface) {
                    Ok(Some(surface)) => {
                        metrics.preprocessed(started.elapsed().as_micros() as u64);
                        output.submit(
                            PreparedFrame {
                                width,
                                height,
                                data: PreparedFrameData::D3D11Nv12(surface),
                            },
                            &metrics,
                        );
                        continue;
                    }
                    Ok(None) => {
                        metrics.dropped_frame();
                        continue;
                    }
                    Err(error) => {
                        gpu_failed_generation = Some(selection.generation);
                        gpu_processor = None;
                        status.lock().error =
                            Some(format!("GPU preprocessing failed; using CPU preprocessing: {error}"));
                    }
                }
            }
        }
        let raw = match frame {
            CapturedFrame::Cpu(frame) => frame,
            CapturedFrame::Gpu(surface) => match read_bgra(&surface) {
                Ok(bgra) => RawFrame {
                    width: surface.width,
                    height: surface.height,
                    bgra,
                },
                Err(error) => {
                    status.lock().error = Some(format!("GPU frame readback failed: {error}"));
                    continue;
                }
            },
        };
        let (bgra, width, height) = match resize_frame(raw, max_width.load(Ordering::Relaxed), &mut resizer, &options) {
            Ok(frame) => frame,
            Err(error) => {
                status.lock().error = Some(error);
                continue;
            }
        };
        let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&bgra, (width as usize, height as usize)));
        metrics.preprocessed(started.elapsed().as_micros() as u64);
        output.submit(
            PreparedFrame {
                width,
                height,
                data: PreparedFrameData::I420(yuv),
            },
            &metrics,
        );
    }
    output.stop();
}

fn resize_frame(
    frame: RawFrame,
    max_width: u32,
    resizer: &mut Resizer,
    options: &ResizeOptions,
) -> Result<(Vec<u8>, u32, u32), String> {
    if frame.width <= max_width {
        return Ok((frame.bgra, frame.width, frame.height));
    }
    let width = max_width & !1;
    let height = (((u64::from(frame.height) * u64::from(width)) / u64::from(frame.width)) as u32).max(2) & !1;
    let source =
        ImageRef::new(frame.width, frame.height, &frame.bgra, PixelType::U8x4).map_err(|error| error.to_string())?;
    let mut output = Image::new(width, height, PixelType::U8x4);
    resizer
        .resize(&source, &mut output, Some(options))
        .map_err(|error| error.to_string())?;
    Ok((output.into_vec(), width, height))
}

fn test_pattern_loop(slot: Arc<LatestFrame>, metrics: Arc<Metrics>, max_fps: Arc<AtomicU32>, width: u32, height: u32) {
    let mut frame_number = 0_u64;
    let mut background = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            background[offset] = (x * 160 / width) as u8 + 32;
            background[offset + 1] = (y * 160 / height) as u8 + 32;
            background[offset + 2] = 24;
            background[offset + 3] = 255;
        }
    }
    while !slot.stopped.load(Ordering::Acquire) {
        let started = Instant::now();
        let period = Duration::from_secs_f64(1.0 / f64::from(max_fps.load(Ordering::Relaxed).max(1)));
        let mut bgra = background.clone();
        let bar = (frame_number % u64::from(width)) as u32;
        for y in 0..height {
            for x in bar.saturating_sub(7)..=(bar + 7).min(width - 1) {
                let offset = (y as usize * width as usize + x as usize) * 4;
                bgra[offset] = 238;
                bgra[offset + 1] = 246;
                bgra[offset + 2] = 255;
            }
        }
        draw_integrity_marker(&mut bgra, width, height, frame_number);
        metrics.captured(width, height);
        slot.submit(CapturedFrame::Cpu(RawFrame { width, height, bgra }), &metrics);
        frame_number = frame_number.wrapping_add(1);
        thread::sleep(period.saturating_sub(started.elapsed()));
    }
}

fn draw_integrity_marker(frame: &mut [u8], width: u32, height: u32, frame_number: u64) {
    let block = (width / 64).clamp(8, 64);
    let margin = block;
    for bit in 0..16_u32 {
        let white = frame_number & (1_u64 << bit) != 0;
        let color = if white { [245, 245, 245, 255] } else { [8, 8, 8, 255] };
        let top_x = margin + bit * block;
        let bottom_x = width.saturating_sub(margin + (bit + 1) * block);
        paint_block(frame, width, height, top_x, margin, block, color);
        paint_block(
            frame,
            width,
            height,
            bottom_x,
            height.saturating_sub(margin + block),
            block,
            color,
        );
    }
}

fn paint_block(frame: &mut [u8], width: u32, height: u32, x: u32, y: u32, size: u32, color: [u8; 4]) {
    for row in y..(y + size).min(height) {
        for column in x..(x + size).min(width) {
            let offset = (row as usize * width as usize + column as usize) * 4;
            frame[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfidb_core::{BenchmarkMetrics, CapabilityState, VideoProfile};

    fn capability(mode: EncoderMode) -> EncoderCapability {
        EncoderCapability {
            id: format!("id-{:?}", mode),
            codec: mode.codec().unwrap(),
            backend: if mode == EncoderMode::H264Software {
                EncoderBackend::OpenH264Software
            } else {
                EncoderBackend::MediaFoundationHardware
            },
            hardware: mode.requires_hardware(),
            encoder_name: mode.label().to_owned(),
            adapter_name: None,
            adapter_luid: None,
            vendor: None,
            driver_version: None,
            input_formats: vec!["NV12".to_owned()],
            profiles: Vec::new(),
            low_latency: Some(true),
            rate_control: vec!["mean-bitrate".to_owned()],
            maximum_tested_width: Some(1920),
            maximum_tested_height: Some(1080),
            maximum_tested_fps: Some(60),
            state: CapabilityState::Functional,
            failure_reason: None,
        }
    }

    fn observation(mode: EncoderMode, metrics: BenchmarkMetrics) -> AutoBenchmarkObservation {
        AutoBenchmarkObservation {
            schema_version: 1,
            nfidb_version: env!("CARGO_PKG_VERSION").to_owned(),
            receiver_runtime: "test-browser".to_owned(),
            encoder_id: format!("id-{:?}", mode),
            mode,
            profile: VideoProfile::Balanced,
            max_width: 1920,
            requested_fps: 60,
            end_to_end_verified: true,
            recorded_unix_ms: 1,
            score: score_auto_candidate(mode, &metrics),
            metrics,
        }
    }

    fn healthy(actual_mbps: f64, encode_p95_ms: f64) -> BenchmarkMetrics {
        BenchmarkMetrics {
            requested_fps: 60.0,
            encoded_fps: 60.0,
            presented_fps: Some(60.0),
            encode_mean_ms: 2.0,
            encode_p95_ms,
            preprocess_mean_ms: 1.0,
            preprocess_p95_ms: 1.5,
            actual_mbps,
            cpu_percent: Some(5.0),
            working_set_mib: Some(150.0),
            drop_percent: 0.0,
            freeze_count: Some(0),
            pipeline_p95_ms: Some(35.0),
            quality_score: None,
        }
    }

    fn browser() -> BrowserVideoCapabilities {
        let mut browser = BrowserVideoCapabilities {
            user_agent: "test-browser".to_owned(),
            ..BrowserVideoCapabilities::default()
        };
        browser.h264.reported = true;
        browser.hevc.reported = true;
        browser.av1.reported = true;
        browser
    }

    #[test]
    fn auto_uses_best_verified_score_instead_of_codec_age() {
        let capabilities = [
            capability(EncoderMode::H264Hardware),
            capability(EncoderMode::HevcHardware),
            capability(EncoderMode::Av1Hardware),
        ];
        let learned = [
            observation(EncoderMode::H264Hardware, healthy(9.0, 2.0)),
            observation(EncoderMode::HevcHardware, healthy(5.5, 3.0)),
            observation(EncoderMode::Av1Hardware, healthy(4.0, 10.0)),
        ];
        let selected = select_encoder(&VideoConfig::default(), &browser(), &capabilities, &learned).unwrap();
        assert_eq!(selected.0, EncoderMode::HevcHardware);
    }

    #[test]
    fn efficient_codec_that_fails_fps_gate_is_rejected() {
        let capabilities = [
            capability(EncoderMode::H264Hardware),
            capability(EncoderMode::HevcHardware),
        ];
        let mut slow = healthy(4.0, 3.0);
        slow.presented_fps = Some(40.0);
        let learned = [
            observation(EncoderMode::H264Hardware, healthy(9.0, 2.0)),
            observation(EncoderMode::HevcHardware, slow),
        ];
        let selected = select_encoder(&VideoConfig::default(), &browser(), &capabilities, &learned).unwrap();
        assert_eq!(selected.0, EncoderMode::H264Hardware);
    }

    #[test]
    fn stale_encoder_identity_is_not_trusted() {
        let capabilities = [
            capability(EncoderMode::H264Hardware),
            capability(EncoderMode::Av1Hardware),
        ];
        let mut stale = observation(EncoderMode::Av1Hardware, healthy(3.0, 2.0));
        stale.encoder_id = "old-adapter-or-driver".to_owned();
        let selected = select_encoder(&VideoConfig::default(), &browser(), &capabilities, &[stale]).unwrap();
        assert_eq!(selected.0, EncoderMode::H264Hardware);
    }

    #[test]
    fn unavailable_hardware_uses_software_fallback() {
        let capabilities = [capability(EncoderMode::H264Software)];
        let selected = select_encoder(
            &VideoConfig::default(),
            &BrowserVideoCapabilities::default(),
            &capabilities,
            &[],
        )
        .unwrap();
        assert_eq!(selected.0, EncoderMode::H264Software);
    }

    #[test]
    fn unsupported_av1_is_never_selected() {
        let capabilities = [
            capability(EncoderMode::H264Hardware),
            capability(EncoderMode::H264Software),
        ];
        let mut browser = browser();
        browser.av1.reported = true;
        let selected = select_encoder(&VideoConfig::default(), &browser, &capabilities, &[]).unwrap();
        assert_eq!(selected.0, EncoderMode::H264Hardware);
    }

    #[test]
    fn preflight_dimensions_preserve_aspect_and_even_sizes() {
        assert_eq!(output_dimensions(3840, 2160, 1920), (1920, 1080));
        assert_eq!(output_dimensions(1919, 1079, 2560), (1918, 1078));
    }
}
