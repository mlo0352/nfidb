use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use nfidb_core::{EncodedVideoFrame, Metrics, VideoProfile};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    RateControlMode, UsageType, VuiConfig,
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
            encoder: "OpenH264 software (Media Foundation hardware path planned)".to_owned(),
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

struct PreparedFrame {
    width: u32,
    height: u32,
    yuv: YUVBuffer,
}

#[derive(Debug, Default)]
struct LatestFrame {
    frame: Mutex<Option<RawFrame>>,
    ready: Condvar,
    stopped: AtomicBool,
}

impl LatestFrame {
    fn submit(&self, frame: RawFrame, metrics: &Metrics) {
        let mut current = self.frame.lock();
        if current.replace(frame).is_some() {
            metrics.dropped_frame();
        }
        self.ready.notify_one();
    }

    fn take(&self) -> Option<RawFrame> {
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
    max_fps: u32,
}

struct ScreenCapture {
    flags: CaptureFlags,
    scratch: Vec<u8>,
    last_frame: Instant,
}

impl GraphicsCaptureApiHandler for ScreenCapture {
    type Flags = CaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            flags: ctx.flags,
            scratch: Vec::new(),
            last_frame: Instant::now() - Duration::from_secs(1),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let min_interval = Duration::from_secs_f64(1.0 / f64::from(self.flags.max_fps.max(1)));
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
        let buffer = frame.buffer().map_err(|error| error.to_string())?;
        let bytes = buffer.as_nopadding_buffer(&mut self.scratch);
        let source_stride = source_width as usize * 4;
        let row_bytes = width as usize * 4;
        let mut bgra = Vec::with_capacity(row_bytes * height as usize);
        for row in bytes.chunks_exact(source_stride).take(height as usize) {
            bgra.extend_from_slice(&row[..row_bytes]);
        }
        self.flags.metrics.captured(width, height);
        self.flags
            .slot
            .submit(RawFrame { width, height, bgra }, &self.flags.metrics);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.flags.slot.stop();
        Ok(())
    }
}

type Control = CaptureControl<ScreenCapture, String>;

pub struct CaptureManager {
    control: Mutex<Option<Control>>,
    producer_thread: Mutex<Option<JoinHandle<()>>>,
    encoder_thread: Mutex<Option<JoinHandle<()>>>,
    slot: Mutex<Arc<LatestFrame>>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    metrics: Arc<Metrics>,
    profile: VideoProfile,
    max_fps: u32,
    cursor: bool,
    status: Arc<Mutex<CaptureStatus>>,
}

impl CaptureManager {
    #[must_use]
    pub fn new(
        video_tx: broadcast::Sender<EncodedVideoFrame>,
        metrics: Arc<Metrics>,
        profile: VideoProfile,
        max_fps: u32,
        cursor: bool,
    ) -> Self {
        Self {
            control: Mutex::new(None),
            producer_thread: Mutex::new(None),
            encoder_thread: Mutex::new(None),
            slot: Mutex::new(Arc::new(LatestFrame::default())),
            video_tx,
            metrics,
            profile,
            max_fps: max_fps.clamp(1, 120),
            cursor,
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        }
    }

    pub fn start_monitor(&self, index: usize) -> Result<(), String> {
        self.stop();
        let monitor = Monitor::from_index(index).map_err(|error| error.to_string())?;
        let width = monitor.width().map_err(|error| error.to_string())?;
        let height = monitor.height().map_err(|error| error.to_string())?;
        let source = monitor.name().unwrap_or_else(|_| format!("Display {index}"));
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);

        let encoder_slot = Arc::clone(&slot);
        let encoder_tx = self.video_tx.clone();
        let encoder_metrics = Arc::clone(&self.metrics);
        let profile = self.profile;
        let max_fps = self.max_fps;
        let status = Arc::clone(&self.status);
        let encoder_thread = thread::Builder::new()
            .name("nfidb-encoder".to_owned())
            .spawn(move || encode_loop(encoder_slot, encoder_tx, encoder_metrics, profile, max_fps, status))
            .map_err(|error| error.to_string())?;

        let settings = Settings::new(
            monitor,
            if self.cursor {
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
                max_fps: self.max_fps,
            },
        );
        match ScreenCapture::start_free_threaded(settings) {
            Ok(control) => {
                *self.control.lock() = Some(control);
                *self.encoder_thread.lock() = Some(encoder_thread);
                *self.status.lock() = CaptureStatus {
                    running: true,
                    source: format!("{source} ({width}×{height})"),
                    ..CaptureStatus::default()
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
        let slot = Arc::new(LatestFrame::default());
        *self.slot.lock() = Arc::clone(&slot);
        let encoder_slot = Arc::clone(&slot);
        let encoder_tx = self.video_tx.clone();
        let encoder_metrics = Arc::clone(&self.metrics);
        let profile = self.profile;
        let max_fps = self.max_fps;
        let status = Arc::clone(&self.status);
        let encoder_thread = thread::Builder::new()
            .name("nfidb-encoder".to_owned())
            .spawn(move || encode_loop(encoder_slot, encoder_tx, encoder_metrics, profile, max_fps, status))
            .map_err(|error| error.to_string())?;
        *self.encoder_thread.lock() = Some(encoder_thread);
        let pattern_slot = Arc::clone(&slot);
        let pattern_metrics = Arc::clone(&self.metrics);
        let producer_thread = thread::Builder::new()
            .name("nfidb-test-pattern".to_owned())
            .spawn(move || test_pattern_loop(pattern_slot, pattern_metrics, max_fps, width, height))
            .map_err(|error| error.to_string())?;
        *self.producer_thread.lock() = Some(producer_thread);
        *self.status.lock() = CaptureStatus {
            running: true,
            source: format!("Generated integrity test pattern ({width}×{height})"),
            ..CaptureStatus::default()
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
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
    }
}

fn encode_loop(
    slot: Arc<LatestFrame>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    metrics: Arc<Metrics>,
    profile: VideoProfile,
    max_fps: u32,
    status: Arc<Mutex<CaptureStatus>>,
) {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(profile.bitrate_bps()))
        .max_frame_rate(FrameRate::from_hz(max_fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .usage_type(UsageType::ScreenContentRealTime)
        .profile(Profile::Baseline)
        .level(Level::Level_4_1)
        .complexity(Complexity::Low)
        .skip_frames(true)
        .scene_change_detect(true)
        .adaptive_quantization(false)
        .background_detection(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(max_fps.max(1)))
        .vui(VuiConfig::srgb());
    let mut encoder = match Encoder::with_api_config(OpenH264API::from_source(), config) {
        Ok(encoder) => encoder,
        Err(error) => {
            status.lock().error = Some(format!("H.264 encoder initialization failed: {error}"));
            return;
        }
    };
    let prepared = Arc::new(LatestPreparedFrame::default());
    let preprocess_output = Arc::clone(&prepared);
    let preprocess_metrics = Arc::clone(&metrics);
    let preprocess_status = Arc::clone(&status);
    let preprocess_thread = match thread::Builder::new()
        .name("nfidb-preprocess".to_owned())
        .spawn(move || preprocess_loop(slot, preprocess_output, preprocess_metrics, profile, preprocess_status))
    {
        Ok(thread) => thread,
        Err(error) => {
            status.lock().error = Some(format!("video preprocessing thread failed: {error}"));
            return;
        }
    };
    let nominal_duration = Duration::from_secs_f64(1.0 / f64::from(max_fps.max(1)));
    let mut last_sent_at: Option<Instant> = None;
    while let Some(frame) = prepared.take() {
        let started = Instant::now();
        match encoder.encode(&frame.yuv) {
            Ok(bitstream) if bitstream.frame_type() != FrameType::Skip => {
                let keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
                let bytes = bitstream.to_vec();
                metrics.encoded(
                    bytes.len(),
                    started.elapsed().as_micros() as u64,
                    frame.width,
                    frame.height,
                );
                let sent_at = Instant::now();
                let duration = last_sent_at
                    .replace(sent_at)
                    .map_or(nominal_duration, |previous| sent_at.duration_since(previous));
                let _ = video_tx.send(EncodedVideoFrame {
                    data: Arc::from(bytes),
                    // RTP timestamps must follow the frames we actually encode. When the
                    // bounded latest-frame queue sheds work, using the requested frame
                    // rate here would make media time run behind wall time and grow lag.
                    duration,
                    width: frame.width,
                    height: frame.height,
                    keyframe,
                });
            }
            Ok(_) => {}
            Err(error) => status.lock().error = Some(format!("H.264 encode failed: {error}")),
        }
    }
    let _ = preprocess_thread.join();
}

fn preprocess_loop(
    slot: Arc<LatestFrame>,
    output: Arc<LatestPreparedFrame>,
    metrics: Arc<Metrics>,
    profile: VideoProfile,
    status: Arc<Mutex<CaptureStatus>>,
) {
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
    let mut resizer = Resizer::new();
    while let Some(frame) = slot.take() {
        let started = Instant::now();
        let (bgra, width, height) = match resize_frame(frame, profile.max_width(), &mut resizer, &options) {
            Ok(frame) => frame,
            Err(error) => {
                status.lock().error = Some(error);
                continue;
            }
        };
        let source = BgraSliceU8::new(&bgra, (width as usize, height as usize));
        let yuv = YUVBuffer::from_bgra8_source(source);
        metrics.preprocessed(started.elapsed().as_micros() as u64);
        output.submit(PreparedFrame { width, height, yuv }, &metrics);
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

fn test_pattern_loop(slot: Arc<LatestFrame>, metrics: Arc<Metrics>, max_fps: u32, width: u32, height: u32) {
    let period = Duration::from_secs_f64(1.0 / f64::from(max_fps.max(1)));
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
        slot.submit(RawFrame { width, height, bgra }, &metrics);
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
