use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use nfidb_core::MetricsSnapshot;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_SAMPLES: usize = 6 * 60 * 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientDiagnosticSample {
    pub sequence: u64,
    pub client_epoch_ms: f64,
    pub sample_interval_ms: f64,
    pub device: Value,
    pub connection: Value,
    pub video: ClientVideoDiagnostic,
    pub network: ClientNetworkDiagnostic,
    pub frame_timing: ClientFrameTimingDiagnostic,
    pub buffers: Value,
    pub raw_rtc: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientVideoDiagnostic {
    pub width: u32,
    pub height: u32,
    pub ready_state: u32,
    pub current_time_seconds: f64,
    pub total_frames: u64,
    pub dropped_frames: u64,
    pub frames_received: u64,
    pub frames_decoded: u64,
    pub decoder_dropped_frames: u64,
    pub decode_fps: f64,
    pub playback_fps: f64,
    pub presentation_drop_percent: f64,
    pub decode_ms_per_frame: f64,
    pub jitter_buffer_ms_per_frame: f64,
    pub freeze_count: u64,
    pub total_freeze_seconds: f64,
    pub startup_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientNetworkDiagnostic {
    pub rtt_ms: f64,
    pub clock_offset_ms: f64,
    pub one_way_estimate_ms: f64,
    pub receive_mbps: f64,
    pub available_incoming_mbps: f64,
    pub bytes_received: u64,
    pub packets_received: u64,
    pub packets_lost: i64,
    pub packet_loss_delta: i64,
    pub jitter_ms: f64,
    pub candidate_type: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientFrameTimingDiagnostic {
    pub callback_count: u64,
    pub frame_gap_p50_ms: f64,
    pub frame_gap_p95_ms: f64,
    pub frame_gap_p99_ms: f64,
    pub frame_gap_max_ms: f64,
    pub capture_to_present_p50_ms: Option<f64>,
    pub capture_to_present_p95_ms: Option<f64>,
    pub capture_to_present_p99_ms: Option<f64>,
    pub receive_to_present_p95_ms: Option<f64>,
    pub processing_p95_ms: Option<f64>,
    pub estimated_pipeline_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedDiagnosticSample {
    pub host_received_epoch_ms: f64,
    pub client: ClientDiagnosticSample,
    pub host: MetricsSnapshot,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DiagnosticSummary {
    pub sample_count: usize,
    pub retained_seconds: f64,
    pub discarded_samples: u64,
    pub rtt_ms: Distribution,
    pub receive_mbps: Distribution,
    pub decode_fps: Distribution,
    pub playback_fps: Distribution,
    pub jitter_buffer_ms_per_frame: Distribution,
    pub decode_ms_per_frame: Distribution,
    pub frame_gap_p95_ms: Distribution,
    pub capture_to_present_p95_ms: Distribution,
    pub estimated_pipeline_ms: Distribution,
    pub host_encode_ms: Distribution,
    pub input_arrival_ms: Distribution,
    pub input_inject_ms: Distribution,
    pub packet_loss_total: i64,
    pub latest_input_sample_gaps: u64,
    pub latest_input_errors: u64,
    pub latest_video_transport_drops: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticReport {
    pub schema_version: u32,
    pub generated_epoch_ms: f64,
    pub limitations: Vec<String>,
    pub summary: DiagnosticSummary,
    pub samples: Vec<RecordedDiagnosticSample>,
}

#[derive(Debug, Default)]
struct RecorderState {
    samples: VecDeque<RecordedDiagnosticSample>,
    discarded_samples: u64,
}

#[derive(Debug, Default)]
pub struct DiagnosticRecorder {
    state: Mutex<RecorderState>,
}

impl DiagnosticRecorder {
    pub fn record(&self, client: ClientDiagnosticSample, host: MetricsSnapshot) {
        let mut state = self.state.lock();
        if state.samples.len() == MAX_SAMPLES {
            state.samples.pop_front();
            state.discarded_samples += 1;
        }
        state.samples.push_back(RecordedDiagnosticSample {
            host_received_epoch_ms: epoch_ms(),
            client,
            host,
        });
    }

    #[must_use]
    pub fn latest(&self) -> Option<RecordedDiagnosticSample> {
        self.state.lock().samples.back().cloned()
    }

    pub fn clear(&self) {
        *self.state.lock() = RecorderState::default();
    }

    #[must_use]
    pub fn summary(&self) -> DiagnosticSummary {
        let state = self.state.lock();
        summarize(&state.samples, state.discarded_samples)
    }

    #[must_use]
    pub fn report(&self) -> DiagnosticReport {
        let state = self.state.lock();
        DiagnosticReport {
            schema_version: 1,
            generated_epoch_ms: epoch_ms(),
            limitations: vec![
                "capture-to-present timing depends on Safari exposing WebRTC frame metadata".to_owned(),
                "one-way input age is an NTP-style estimate derived from LAN RTT and clock offset".to_owned(),
                "exact Pencil-to-photon glass latency requires synchronized high-speed camera measurement".to_owned(),
            ],
            summary: summarize(&state.samples, state.discarded_samples),
            samples: state.samples.iter().cloned().collect(),
        }
    }
}

fn summarize(samples: &VecDeque<RecordedDiagnosticSample>, discarded_samples: u64) -> DiagnosticSummary {
    let values = |read: fn(&RecordedDiagnosticSample) -> Option<f64>| {
        samples
            .iter()
            .filter_map(read)
            .filter(|value| value.is_finite())
            .collect()
    };
    let latest = samples.back();
    DiagnosticSummary {
        sample_count: samples.len(),
        retained_seconds: samples.front().zip(samples.back()).map_or(0.0, |(first, last)| {
            (last.host_received_epoch_ms - first.host_received_epoch_ms).max(0.0) / 1000.0
        }),
        discarded_samples,
        rtt_ms: distribution(values(|sample| finite_nonzero(sample.client.network.rtt_ms))),
        receive_mbps: distribution(values(|sample| {
            measured_rate(sample, sample.client.network.receive_mbps)
        })),
        decode_fps: distribution(values(|sample| measured_rate(sample, sample.client.video.decode_fps))),
        playback_fps: distribution(values(|sample| measured_rate(sample, sample.client.video.playback_fps))),
        jitter_buffer_ms_per_frame: distribution(values(|sample| {
            measured_rate(sample, sample.client.video.jitter_buffer_ms_per_frame)
        })),
        decode_ms_per_frame: distribution(values(|sample| {
            measured_rate(sample, sample.client.video.decode_ms_per_frame)
        })),
        frame_gap_p95_ms: distribution(values(|sample| {
            measured_rate(sample, sample.client.frame_timing.frame_gap_p95_ms)
        })),
        capture_to_present_p95_ms: distribution(values(|sample| sample.client.frame_timing.capture_to_present_p95_ms)),
        estimated_pipeline_ms: distribution(values(|sample| {
            measured_rate(sample, sample.client.frame_timing.estimated_pipeline_ms)
        })),
        host_encode_ms: distribution(values(|sample| Some(sample.host.encode_ms))),
        input_arrival_ms: distribution(values(|sample| {
            (sample.host.input_samples > 0)
                .then(|| finite_positive(sample.host.input_arrival_ms))
                .flatten()
        })),
        input_inject_ms: distribution(values(|sample| {
            (sample.host.input_samples > 0)
                .then(|| finite_positive(sample.host.input_inject_ms))
                .flatten()
        })),
        packet_loss_total: latest.map_or(0, |sample| sample.client.network.packets_lost),
        latest_input_sample_gaps: latest.map_or(0, |sample| sample.host.sample_sequence_gaps),
        latest_input_errors: latest.map_or(0, |sample| sample.host.input_errors),
        latest_video_transport_drops: latest.map_or(0, |sample| sample.host.video_transport_drops),
    }
}

fn finite_positive(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn finite_nonzero(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn measured_rate(sample: &RecordedDiagnosticSample, value: f64) -> Option<f64> {
    (sample.client.sequence > 0).then(|| finite_positive(value)).flatten()
}

fn distribution(mut values: Vec<f64>) -> Distribution {
    if values.is_empty() {
        return Distribution::default();
    }
    values.sort_by(f64::total_cmp);
    let sum: f64 = values.iter().sum();
    Distribution {
        count: values.len(),
        min: values[0],
        mean: sum / values.len() as f64,
        p50: percentile(&values, 0.50),
        p95: percentile(&values, 0.95),
        p99: percentile(&values, 0.99),
        max: values[values.len() - 1],
    }
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index]
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

    #[test]
    fn summary_processes_percentiles_and_bounds_history() {
        let recorder = DiagnosticRecorder::default();
        for sequence in 0..3 {
            let mut client = ClientDiagnosticSample {
                sequence,
                ..ClientDiagnosticSample::default()
            };
            client.network.rtt_ms = 2.0 + sequence as f64;
            client.network.receive_mbps = 5.0 + sequence as f64;
            recorder.record(client, nfidb_core::Metrics::default().snapshot());
        }
        let summary = recorder.summary();
        assert_eq!(summary.sample_count, 3);
        assert_eq!(summary.rtt_ms.min, 2.0);
        assert_eq!(summary.rtt_ms.p95, 4.0);
        assert_eq!(recorder.report().schema_version, 1);
        recorder.clear();
        assert_eq!(recorder.summary().sample_count, 0);
    }

    #[test]
    fn summary_excludes_uninitialized_latency_and_rate_samples() {
        let recorder = DiagnosticRecorder::default();
        recorder.record(
            ClientDiagnosticSample::default(),
            nfidb_core::Metrics::default().snapshot(),
        );

        let mut measured_client = ClientDiagnosticSample {
            sequence: 1,
            ..ClientDiagnosticSample::default()
        };
        measured_client.network.rtt_ms = 3.5;
        measured_client.network.receive_mbps = 7.25;
        let mut measured_host = nfidb_core::Metrics::default().snapshot();
        measured_host.input_samples = 1;
        measured_host.input_arrival_ms = 2.25;
        recorder.record(measured_client, measured_host);

        let summary = recorder.summary();
        assert_eq!(summary.rtt_ms.count, 1);
        assert_eq!(summary.rtt_ms.min, 3.5);
        assert_eq!(summary.receive_mbps.count, 1);
        assert_eq!(summary.receive_mbps.min, 7.25);
        assert_eq!(summary.input_arrival_ms.count, 1);
        assert_eq!(summary.input_arrival_ms.min, 2.25);
        assert_eq!(summary.input_inject_ms.count, 1);
    }
}
