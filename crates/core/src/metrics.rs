use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

#[derive(Debug)]
struct RateState {
    at: Instant,
    capture_frames: u64,
    encoded_frames: u64,
    input_samples: u64,
    coalesced_samples: u64,
}

impl Default for RateState {
    fn default() -> Self {
        Self {
            at: Instant::now(),
            capture_frames: 0,
            encoded_frames: 0,
            input_samples: 0,
            coalesced_samples: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Metrics {
    connected: AtomicBool,
    capture_frames: AtomicU64,
    encoded_frames: AtomicU64,
    dropped_frames: AtomicU64,
    encoded_bytes: AtomicU64,
    input_samples: AtomicU64,
    coalesced_samples: AtomicU64,
    last_pressure_bits: AtomicU32,
    last_tilt_x_bits: AtomicU32,
    last_tilt_y_bits: AtomicU32,
    rtt_micros: AtomicU64,
    encode_micros: AtomicU64,
    source_width: AtomicU32,
    source_height: AtomicU32,
    rate_state: Mutex<RateState>,
}

impl Metrics {
    pub fn set_connected(&self, value: bool) {
        self.connected.store(value, Ordering::Relaxed);
    }

    pub fn captured(&self, width: u32, height: u32) {
        self.capture_frames.fetch_add(1, Ordering::Relaxed);
        self.source_width.store(width, Ordering::Relaxed);
        self.source_height.store(height, Ordering::Relaxed);
    }

    pub fn encoded(&self, bytes: usize, elapsed_micros: u64) {
        self.encoded_frames.fetch_add(1, Ordering::Relaxed);
        self.encoded_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.encode_micros.store(elapsed_micros, Ordering::Relaxed);
    }

    pub fn dropped_frame(&self) {
        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn input(&self, count: usize, coalesced: usize, pressure: f32, tilt_x: f32, tilt_y: f32) {
        self.input_samples.fetch_add(count as u64, Ordering::Relaxed);
        self.coalesced_samples.fetch_add(coalesced as u64, Ordering::Relaxed);
        self.last_pressure_bits.store(pressure.to_bits(), Ordering::Relaxed);
        self.last_tilt_x_bits.store(tilt_x.to_bits(), Ordering::Relaxed);
        self.last_tilt_y_bits.store(tilt_y.to_bits(), Ordering::Relaxed);
    }

    pub fn set_rtt_ms(&self, rtt_ms: f64) {
        self.rtt_micros
            .store((rtt_ms.max(0.0) * 1000.0) as u64, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        let now = Instant::now();
        let capture_frames = self.capture_frames.load(Ordering::Relaxed);
        let encoded_frames = self.encoded_frames.load(Ordering::Relaxed);
        let input_samples = self.input_samples.load(Ordering::Relaxed);
        let coalesced_samples = self.coalesced_samples.load(Ordering::Relaxed);
        let mut rate = self.rate_state.lock();
        let seconds = now.duration_since(rate.at).as_secs_f64().max(0.001);
        let snapshot = MetricsSnapshot {
            connected: self.connected.load(Ordering::Relaxed),
            capture_fps: (capture_frames.saturating_sub(rate.capture_frames)) as f64 / seconds,
            encoded_fps: (encoded_frames.saturating_sub(rate.encoded_frames)) as f64 / seconds,
            input_samples_per_sec: (input_samples.saturating_sub(rate.input_samples)) as f64 / seconds,
            coalesced_samples_per_sec: (coalesced_samples.saturating_sub(rate.coalesced_samples)) as f64 / seconds,
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            encoded_bytes: self.encoded_bytes.load(Ordering::Relaxed),
            input_samples,
            pressure: f32::from_bits(self.last_pressure_bits.load(Ordering::Relaxed)),
            tilt_x: f32::from_bits(self.last_tilt_x_bits.load(Ordering::Relaxed)),
            tilt_y: f32::from_bits(self.last_tilt_y_bits.load(Ordering::Relaxed)),
            rtt_ms: self.rtt_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            encode_ms: self.encode_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            source_width: self.source_width.load(Ordering::Relaxed),
            source_height: self.source_height.load(Ordering::Relaxed),
        };
        *rate = RateState {
            at: now,
            capture_frames,
            encoded_frames,
            input_samples,
            coalesced_samples,
        };
        snapshot
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub connected: bool,
    pub capture_fps: f64,
    pub encoded_fps: f64,
    pub input_samples_per_sec: f64,
    pub coalesced_samples_per_sec: f64,
    pub dropped_frames: u64,
    pub encoded_bytes: u64,
    pub input_samples: u64,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rtt_ms: f64,
    pub encode_ms: f64,
    pub source_width: u32,
    pub source_height: u32,
}
