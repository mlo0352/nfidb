use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nfidb_protocol::{Action, DeviceType, PointerBatch, TextInput};
use parking_lot::Mutex;
use serde::Serialize;

const RATE_WINDOW: Duration = Duration::from_millis(250);

#[derive(Debug)]
struct RateState {
    at: Instant,
    capture_frames: u64,
    encoded_frames: u64,
    input_samples: u64,
    coalesced_samples: u64,
    capture_fps: f64,
    encoded_fps: f64,
    input_samples_per_sec: f64,
    coalesced_samples_per_sec: f64,
}

impl Default for RateState {
    fn default() -> Self {
        Self {
            at: Instant::now(),
            capture_frames: 0,
            encoded_frames: 0,
            input_samples: 0,
            coalesced_samples: 0,
            capture_fps: 0.0,
            encoded_fps: 0.0,
            input_samples_per_sec: 0.0,
            coalesced_samples_per_sec: 0.0,
        }
    }
}

#[derive(Debug)]
struct InputContinuity {
    expected_batch: Option<u32>,
    expected_sample: Option<u32>,
    active_pointers: BTreeSet<(u8, u32)>,
    pressure_min: f32,
    pressure_max: f32,
    tilt_x_min: f32,
    tilt_x_max: f32,
    tilt_y_min: f32,
    tilt_y_max: f32,
}

impl Default for InputContinuity {
    fn default() -> Self {
        Self {
            expected_batch: None,
            expected_sample: None,
            active_pointers: BTreeSet::new(),
            pressure_min: 1.0,
            pressure_max: 0.0,
            tilt_x_min: 0.0,
            tilt_x_max: 0.0,
            tilt_y_min: 0.0,
            tilt_y_max: 0.0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Metrics {
    connected: AtomicBool,
    capture_frames: AtomicU64,
    encoded_frames: AtomicU64,
    encoded_keyframes: AtomicU64,
    dropped_frames: AtomicU64,
    video_transport_drops: AtomicU64,
    video_startup_delta_frames: AtomicU64,
    video_startup_wait_micros: AtomicU64,
    video_recovery_requests: AtomicU64,
    encoded_bytes: AtomicU64,
    preprocessed_frames: AtomicU64,
    preprocess_micros: AtomicU64,
    preprocess_micros_total: AtomicU64,
    preprocess_micros_max: AtomicU64,
    encode_micros: AtomicU64,
    encode_micros_total: AtomicU64,
    encode_micros_max: AtomicU64,
    input_batches: AtomicU64,
    input_samples: AtomicU64,
    injected_samples: AtomicU64,
    input_errors: AtomicU64,
    mouse_samples: AtomicU64,
    wheel_events: AtomicU64,
    keyboard_events: AtomicU64,
    text_events: AtomicU64,
    text_bytes: AtomicU64,
    command_events: AtomicU64,
    client_clock_offset_bits: AtomicU64,
    input_arrival_micros: AtomicU64,
    input_arrival_micros_total: AtomicU64,
    input_arrival_micros_max: AtomicU64,
    input_arrival_samples: AtomicU64,
    input_inject_micros: AtomicU64,
    input_inject_micros_total: AtomicU64,
    input_inject_micros_max: AtomicU64,
    input_inject_samples: AtomicU64,
    coalesced_samples: AtomicU64,
    batch_sequence_gaps: AtomicU64,
    sample_sequence_gaps: AtomicU64,
    out_of_order_batches: AtomicU64,
    out_of_order_samples: AtomicU64,
    lifecycle_errors: AtomicU64,
    last_batch_sequence: AtomicU32,
    last_sample_sequence: AtomicU32,
    last_pressure_bits: AtomicU32,
    last_tilt_x_bits: AtomicU32,
    last_tilt_y_bits: AtomicU32,
    rtt_micros: AtomicU64,
    source_width: AtomicU32,
    source_height: AtomicU32,
    output_width: AtomicU32,
    output_height: AtomicU32,
    rate_state: Mutex<RateState>,
    input_continuity: Mutex<InputContinuity>,
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

    pub fn encoded(&self, bytes: usize, elapsed_micros: u64, width: u32, height: u32) {
        self.encoded_frames.fetch_add(1, Ordering::Relaxed);
        self.encoded_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.encode_micros.store(elapsed_micros, Ordering::Relaxed);
        self.encode_micros_total.fetch_add(elapsed_micros, Ordering::Relaxed);
        self.encode_micros_max.fetch_max(elapsed_micros, Ordering::Relaxed);
        self.output_width.store(width, Ordering::Relaxed);
        self.output_height.store(height, Ordering::Relaxed);
    }

    pub fn preprocessed(&self, elapsed_micros: u64) {
        self.preprocessed_frames.fetch_add(1, Ordering::Relaxed);
        self.preprocess_micros.store(elapsed_micros, Ordering::Relaxed);
        self.preprocess_micros_total
            .fetch_add(elapsed_micros, Ordering::Relaxed);
        self.preprocess_micros_max.fetch_max(elapsed_micros, Ordering::Relaxed);
    }

    pub fn encoded_keyframe(&self) {
        self.encoded_keyframes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dropped_frame(&self) {
        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn video_transport_dropped(&self, count: u64) {
        self.video_transport_drops.fetch_add(count, Ordering::Relaxed);
    }

    pub fn video_startup_delta_frame_skipped(&self) {
        self.video_startup_delta_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn video_started(&self, wait: Duration) {
        self.video_startup_wait_micros
            .store(wait.as_micros().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
    }

    pub fn video_recovery_requested(&self) {
        self.video_recovery_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn input_batch(&self, batch: &PointerBatch) {
        self.input_batches.fetch_add(1, Ordering::Relaxed);
        self.input_samples
            .fetch_add(batch.samples.len() as u64, Ordering::Relaxed);
        self.coalesced_samples
            .fetch_add(batch.samples.len().saturating_sub(1) as u64, Ordering::Relaxed);
        self.last_batch_sequence.store(batch.batch_sequence, Ordering::Relaxed);
        self.observe_input_arrival(batch);

        let mut state = self.input_continuity.lock();
        observe_sequence(
            &mut state.expected_batch,
            batch.batch_sequence,
            &self.batch_sequence_gaps,
            &self.out_of_order_batches,
        );
        for sample in &batch.samples {
            if sample.device_type == DeviceType::Mouse {
                self.mouse_samples.fetch_add(1, Ordering::Relaxed);
            }
            observe_sequence(
                &mut state.expected_sample,
                sample.sample_sequence,
                &self.sample_sequence_gaps,
                &self.out_of_order_samples,
            );
            self.last_sample_sequence
                .store(sample.sample_sequence, Ordering::Relaxed);
            let sample = sample.sanitized();
            if sample.device_type == DeviceType::Pen {
                self.last_pressure_bits
                    .store(sample.pressure.to_bits(), Ordering::Relaxed);
                self.last_tilt_x_bits
                    .store(sample.tilt_x_deg.to_bits(), Ordering::Relaxed);
                self.last_tilt_y_bits
                    .store(sample.tilt_y_deg.to_bits(), Ordering::Relaxed);
                state.pressure_min = state.pressure_min.min(sample.pressure);
                state.pressure_max = state.pressure_max.max(sample.pressure);
                state.tilt_x_min = state.tilt_x_min.min(sample.tilt_x_deg);
                state.tilt_x_max = state.tilt_x_max.max(sample.tilt_x_deg);
                state.tilt_y_min = state.tilt_y_min.min(sample.tilt_y_deg);
                state.tilt_y_max = state.tilt_y_max.max(sample.tilt_y_deg);
            }

            let key = (sample.device_type as u8, sample.pointer_id);
            match sample.action {
                Action::Down => {
                    if !state.active_pointers.insert(key) {
                        self.lifecycle_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Action::Move => {
                    if !state.active_pointers.contains(&key) {
                        self.lifecycle_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Action::Up | Action::Cancel => {
                    if !state.active_pointers.remove(&key) {
                        self.lifecycle_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Action::Hover => {}
            }
        }
    }

    pub fn input_injected(&self, count: usize, elapsed: Duration) {
        self.injected_samples.fetch_add(count as u64, Ordering::Relaxed);
        let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.input_inject_micros.store(micros, Ordering::Relaxed);
        self.input_inject_micros_total.fetch_add(micros, Ordering::Relaxed);
        self.input_inject_micros_max.fetch_max(micros, Ordering::Relaxed);
        self.input_inject_samples.fetch_add(1, Ordering::Relaxed);
    }

    pub fn input_error(&self) {
        self.input_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn wheel_input(&self) {
        self.wheel_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn keyboard_input(&self) {
        self.keyboard_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn text_input(&self, input: &TextInput) {
        self.text_events.fetch_add(1, Ordering::Relaxed);
        self.text_bytes.fetch_add(input.text.len() as u64, Ordering::Relaxed);
    }

    pub fn command_input(&self) {
        self.command_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset_input_continuity(&self) {
        let mut state = self.input_continuity.lock();
        state.expected_batch = None;
        state.expected_sample = None;
        state.active_pointers.clear();
    }

    pub fn set_rtt_ms(&self, rtt_ms: f64) {
        self.rtt_micros
            .store((rtt_ms.max(0.0) * 1000.0) as u64, Ordering::Relaxed);
    }

    pub fn set_client_clock_offset_ms(&self, offset_ms: f64) {
        if offset_ms.is_finite() {
            self.client_clock_offset_bits
                .store(offset_ms.to_bits(), Ordering::Relaxed);
        }
    }

    fn observe_input_arrival(&self, batch: &PointerBatch) {
        let Some(sample) = batch.samples.last() else {
            return;
        };
        let offset_ms = f64::from_bits(self.client_clock_offset_bits.load(Ordering::Relaxed));
        let arrival_ms = epoch_ms() - (sample.client_time_ms + offset_ms);
        if !arrival_ms.is_finite() || !(-50.0..=5_000.0).contains(&arrival_ms) {
            return;
        }
        let micros = (arrival_ms.max(0.0) * 1000.0) as u64;
        self.input_arrival_micros.store(micros, Ordering::Relaxed);
        self.input_arrival_micros_total.fetch_add(micros, Ordering::Relaxed);
        self.input_arrival_micros_max.fetch_max(micros, Ordering::Relaxed);
        self.input_arrival_samples.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        let now = Instant::now();
        let capture_frames = self.capture_frames.load(Ordering::Relaxed);
        let encoded_frames = self.encoded_frames.load(Ordering::Relaxed);
        let input_samples = self.input_samples.load(Ordering::Relaxed);
        let coalesced_samples = self.coalesced_samples.load(Ordering::Relaxed);
        let mut rate = self.rate_state.lock();
        let elapsed = now.duration_since(rate.at);
        if elapsed >= RATE_WINDOW {
            let seconds = elapsed.as_secs_f64().max(0.001);
            rate.capture_fps = capture_frames.saturating_sub(rate.capture_frames) as f64 / seconds;
            rate.encoded_fps = encoded_frames.saturating_sub(rate.encoded_frames) as f64 / seconds;
            rate.input_samples_per_sec = input_samples.saturating_sub(rate.input_samples) as f64 / seconds;
            rate.coalesced_samples_per_sec = coalesced_samples.saturating_sub(rate.coalesced_samples) as f64 / seconds;
            rate.at = now;
            rate.capture_frames = capture_frames;
            rate.encoded_frames = encoded_frames;
            rate.input_samples = input_samples;
            rate.coalesced_samples = coalesced_samples;
        }
        let continuity = self.input_continuity.lock();
        let encode_micros_total = self.encode_micros_total.load(Ordering::Relaxed);
        let preprocessed_frames = self.preprocessed_frames.load(Ordering::Relaxed);
        let preprocess_micros_total = self.preprocess_micros_total.load(Ordering::Relaxed);
        let input_arrival_samples = self.input_arrival_samples.load(Ordering::Relaxed);
        let input_inject_samples = self.input_inject_samples.load(Ordering::Relaxed);
        MetricsSnapshot {
            connected: self.connected.load(Ordering::Relaxed),
            capture_fps: rate.capture_fps,
            encoded_fps: rate.encoded_fps,
            input_samples_per_sec: rate.input_samples_per_sec,
            coalesced_samples_per_sec: rate.coalesced_samples_per_sec,
            capture_frames,
            encoded_frames,
            encoded_keyframes: self.encoded_keyframes.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            video_transport_drops: self.video_transport_drops.load(Ordering::Relaxed),
            video_startup_delta_frames: self.video_startup_delta_frames.load(Ordering::Relaxed),
            video_startup_wait_ms: self.video_startup_wait_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            video_recovery_requests: self.video_recovery_requests.load(Ordering::Relaxed),
            encoded_bytes: self.encoded_bytes.load(Ordering::Relaxed),
            input_batches: self.input_batches.load(Ordering::Relaxed),
            input_samples,
            injected_samples: self.injected_samples.load(Ordering::Relaxed),
            input_errors: self.input_errors.load(Ordering::Relaxed),
            mouse_samples: self.mouse_samples.load(Ordering::Relaxed),
            wheel_events: self.wheel_events.load(Ordering::Relaxed),
            keyboard_events: self.keyboard_events.load(Ordering::Relaxed),
            text_events: self.text_events.load(Ordering::Relaxed),
            text_bytes: self.text_bytes.load(Ordering::Relaxed),
            command_events: self.command_events.load(Ordering::Relaxed),
            client_clock_offset_ms: f64::from_bits(self.client_clock_offset_bits.load(Ordering::Relaxed)),
            input_arrival_ms: self.input_arrival_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            average_input_arrival_ms: average_micros(
                self.input_arrival_micros_total.load(Ordering::Relaxed),
                input_arrival_samples,
            ),
            max_input_arrival_ms: self.input_arrival_micros_max.load(Ordering::Relaxed) as f64 / 1000.0,
            input_inject_ms: self.input_inject_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            average_input_inject_ms: average_micros(
                self.input_inject_micros_total.load(Ordering::Relaxed),
                input_inject_samples,
            ),
            max_input_inject_ms: self.input_inject_micros_max.load(Ordering::Relaxed) as f64 / 1000.0,
            batch_sequence_gaps: self.batch_sequence_gaps.load(Ordering::Relaxed),
            sample_sequence_gaps: self.sample_sequence_gaps.load(Ordering::Relaxed),
            out_of_order_batches: self.out_of_order_batches.load(Ordering::Relaxed),
            out_of_order_samples: self.out_of_order_samples.load(Ordering::Relaxed),
            lifecycle_errors: self.lifecycle_errors.load(Ordering::Relaxed),
            active_pointers: continuity.active_pointers.len() as u32,
            last_batch_sequence: self.last_batch_sequence.load(Ordering::Relaxed),
            last_sample_sequence: self.last_sample_sequence.load(Ordering::Relaxed),
            pressure: f32::from_bits(self.last_pressure_bits.load(Ordering::Relaxed)),
            pressure_min: continuity.pressure_min,
            pressure_max: continuity.pressure_max,
            tilt_x: f32::from_bits(self.last_tilt_x_bits.load(Ordering::Relaxed)),
            tilt_y: f32::from_bits(self.last_tilt_y_bits.load(Ordering::Relaxed)),
            tilt_x_min: continuity.tilt_x_min,
            tilt_x_max: continuity.tilt_x_max,
            tilt_y_min: continuity.tilt_y_min,
            tilt_y_max: continuity.tilt_y_max,
            rtt_ms: self.rtt_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            encode_ms: self.encode_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            preprocess_ms: self.preprocess_micros.load(Ordering::Relaxed) as f64 / 1000.0,
            average_preprocess_ms: if preprocessed_frames == 0 {
                0.0
            } else {
                preprocess_micros_total as f64 / preprocessed_frames as f64 / 1000.0
            },
            max_preprocess_ms: self.preprocess_micros_max.load(Ordering::Relaxed) as f64 / 1000.0,
            average_encode_ms: if encoded_frames == 0 {
                0.0
            } else {
                encode_micros_total as f64 / encoded_frames as f64 / 1000.0
            },
            max_encode_ms: self.encode_micros_max.load(Ordering::Relaxed) as f64 / 1000.0,
            source_width: self.source_width.load(Ordering::Relaxed),
            source_height: self.source_height.load(Ordering::Relaxed),
            output_width: self.output_width.load(Ordering::Relaxed),
            output_height: self.output_height.load(Ordering::Relaxed),
        }
    }
}

fn observe_sequence(expected: &mut Option<u32>, received: u32, gaps: &AtomicU64, out_of_order: &AtomicU64) {
    if let Some(next) = *expected {
        let forward = received.wrapping_sub(next);
        if forward == 0 {
            *expected = Some(received.wrapping_add(1));
        } else if forward < (1_u32 << 31) {
            gaps.fetch_add(u64::from(forward), Ordering::Relaxed);
            *expected = Some(received.wrapping_add(1));
        } else {
            out_of_order.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        *expected = Some(received.wrapping_add(1));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub connected: bool,
    pub capture_fps: f64,
    pub encoded_fps: f64,
    pub input_samples_per_sec: f64,
    pub coalesced_samples_per_sec: f64,
    pub capture_frames: u64,
    pub encoded_frames: u64,
    pub encoded_keyframes: u64,
    pub dropped_frames: u64,
    pub video_transport_drops: u64,
    pub video_startup_delta_frames: u64,
    pub video_startup_wait_ms: f64,
    pub video_recovery_requests: u64,
    pub encoded_bytes: u64,
    pub input_batches: u64,
    pub input_samples: u64,
    pub injected_samples: u64,
    pub input_errors: u64,
    pub mouse_samples: u64,
    pub wheel_events: u64,
    pub keyboard_events: u64,
    pub text_events: u64,
    pub text_bytes: u64,
    pub command_events: u64,
    pub client_clock_offset_ms: f64,
    pub input_arrival_ms: f64,
    pub average_input_arrival_ms: f64,
    pub max_input_arrival_ms: f64,
    pub input_inject_ms: f64,
    pub average_input_inject_ms: f64,
    pub max_input_inject_ms: f64,
    pub batch_sequence_gaps: u64,
    pub sample_sequence_gaps: u64,
    pub out_of_order_batches: u64,
    pub out_of_order_samples: u64,
    pub lifecycle_errors: u64,
    pub active_pointers: u32,
    pub last_batch_sequence: u32,
    pub last_sample_sequence: u32,
    pub pressure: f32,
    pub pressure_min: f32,
    pub pressure_max: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub tilt_x_min: f32,
    pub tilt_x_max: f32,
    pub tilt_y_min: f32,
    pub tilt_y_max: f32,
    pub rtt_ms: f64,
    pub encode_ms: f64,
    pub preprocess_ms: f64,
    pub average_preprocess_ms: f64,
    pub max_preprocess_ms: f64,
    pub average_encode_ms: f64,
    pub max_encode_ms: f64,
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
}

fn average_micros(total: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64 / 1000.0
    }
}

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfidb_protocol::{DeviceType, PointerSample};

    fn sample(action: Action, sequence: u32) -> PointerSample {
        PointerSample {
            device_type: DeviceType::Pen,
            action,
            flags: 0,
            pointer_id: 7,
            sample_sequence: sequence,
            x_norm: 0.5,
            y_norm: 0.5,
            pressure: sequence as f32 / 10.0,
            tilt_x_deg: sequence as f32,
            tilt_y_deg: -(sequence as f32),
            twist_deg: 0.0,
            client_time_ms: 0.0,
        }
    }

    #[test]
    fn counts_continuity_gaps_and_lifecycle_errors() {
        let metrics = Metrics::default();
        metrics.input_batch(&PointerBatch {
            batch_sequence: 10,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Down, 20), sample(Action::Move, 21)],
        });
        metrics.input_batch(&PointerBatch {
            batch_sequence: 12,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Move, 23), sample(Action::Up, 24)],
        });
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.batch_sequence_gaps, 1);
        assert_eq!(snapshot.sample_sequence_gaps, 1);
        assert_eq!(snapshot.lifecycle_errors, 0);
        assert_eq!(snapshot.active_pointers, 0);
        assert_eq!(snapshot.pressure_min, 1.0);
        assert_eq!(snapshot.pressure_max, 1.0);

        metrics.input_batch(&PointerBatch {
            batch_sequence: 11,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Move, 22)],
        });
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.out_of_order_batches, 1);
        assert_eq!(snapshot.out_of_order_samples, 1);
        assert_eq!(snapshot.lifecycle_errors, 1);
    }

    #[test]
    fn sequence_tracking_accepts_u32_wraparound() {
        let gaps = AtomicU64::new(0);
        let out_of_order = AtomicU64::new(0);
        let mut expected = None;
        observe_sequence(&mut expected, u32::MAX, &gaps, &out_of_order);
        observe_sequence(&mut expected, 0, &gaps, &out_of_order);
        assert_eq!(gaps.load(Ordering::Relaxed), 0);
        assert_eq!(out_of_order.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn counts_browser_video_recovery_requests() {
        let metrics = Metrics::default();
        metrics.video_recovery_requested();
        metrics.video_recovery_requested();
        assert_eq!(metrics.snapshot().video_recovery_requests, 2);
    }
}
