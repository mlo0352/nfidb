use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{EncoderMode, VideoCodec, VideoConfig, VideoProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EncoderBackend {
    MediaFoundationHardware,
    VideoToolboxHardware,
    OpenH264Software,
}

impl EncoderBackend {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MediaFoundationHardware => "Media Foundation hardware",
            Self::VideoToolboxHardware => "VideoToolbox hardware",
            Self::OpenH264Software => "OpenH264 software",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PipelineMemoryMode {
    GpuZeroCopy,
    GpuAssisted,
    CpuCopy,
    CpuPreprocessing,
}

impl PipelineMemoryMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GpuZeroCopy => "GPU zero-copy",
            Self::GpuAssisted => "GPU assisted",
            Self::CpuCopy => "CPU copy",
            Self::CpuPreprocessing => "CPU preprocess",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityState {
    Detected,
    Initializeable,
    Functional,
    BenchmarkTested,
    Unavailable,
    Failed,
}

impl CapabilityState {
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Functional | Self::BenchmarkTested)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoderCapability {
    pub id: String,
    pub codec: VideoCodec,
    pub backend: EncoderBackend,
    pub hardware: bool,
    pub encoder_name: String,
    pub adapter_name: Option<String>,
    pub adapter_luid: Option<String>,
    pub vendor: Option<String>,
    pub driver_version: Option<String>,
    pub input_formats: Vec<String>,
    pub profiles: Vec<String>,
    pub low_latency: Option<bool>,
    pub rate_control: Vec<String>,
    pub maximum_tested_width: Option<u32>,
    pub maximum_tested_height: Option<u32>,
    pub maximum_tested_fps: Option<u32>,
    pub state: CapabilityState,
    pub failure_reason: Option<String>,
}

impl EncoderCapability {
    #[must_use]
    pub fn mode(&self) -> EncoderMode {
        match (self.codec, self.hardware) {
            (VideoCodec::H264, true) => EncoderMode::H264Hardware,
            (VideoCodec::Hevc, true) => EncoderMode::HevcHardware,
            (VideoCodec::Av1, true) => EncoderMode::Av1Hardware,
            (VideoCodec::H264, false) => EncoderMode::H264Software,
            // NFiDB intentionally does not provide software HEVC/AV1 modes.
            (_, false) => EncoderMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserCodecCapability {
    pub reported: bool,
    pub included_in_sdp: bool,
    pub negotiated: bool,
    pub first_keyframe_received: bool,
    pub presented: bool,
    pub mime_types: Vec<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserVideoCapabilities {
    pub user_agent: String,
    pub set_codec_preferences: bool,
    pub h264: BrowserCodecCapability,
    pub hevc: BrowserCodecCapability,
    pub av1: BrowserCodecCapability,
}

impl BrowserVideoCapabilities {
    #[must_use]
    pub const fn get(&self, codec: VideoCodec) -> &BrowserCodecCapability {
        match codec {
            VideoCodec::H264 => &self.h264,
            VideoCodec::Hevc => &self.hevc,
            VideoCodec::Av1 => &self.av1,
        }
    }

    #[must_use]
    pub const fn get_mut(&mut self, codec: VideoCodec) -> &mut BrowserCodecCapability {
        match codec {
            VideoCodec::H264 => &mut self.h264,
            VideoCodec::Hevc => &mut self.hevc,
            VideoCodec::Av1 => &mut self.av1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeAvailability {
    Available,
    Provisional,
    Experimental,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityEntry {
    pub mode: EncoderMode,
    pub codec: VideoCodec,
    pub host_detected: bool,
    pub host_functional: bool,
    pub browser_reported: bool,
    pub negotiated: bool,
    pub presentation_verified: bool,
    pub availability: ModeAvailability,
    pub reason: String,
}

#[must_use]
pub fn compatibility_matrix(host: &[EncoderCapability], browser: &BrowserVideoCapabilities) -> Vec<CompatibilityEntry> {
    [
        EncoderMode::H264Hardware,
        EncoderMode::HevcHardware,
        EncoderMode::Av1Hardware,
        EncoderMode::H264Software,
    ]
    .into_iter()
    .map(|mode| {
        let codec = mode.codec().expect("manual encoder modes have a codec");
        let candidates: Vec<_> = host.iter().filter(|item| item.mode() == mode).collect();
        let host_detected = !candidates.is_empty();
        let host_functional = candidates.iter().any(|item| item.state.is_usable());
        let receiver = browser.get(codec);
        let availability = if !host_detected || !host_functional {
            ModeAvailability::Unavailable
        } else if receiver.presented {
            ModeAvailability::Available
        } else if receiver.negotiated || receiver.first_keyframe_received {
            ModeAvailability::Experimental
        } else if receiver.reported {
            ModeAvailability::Provisional
        } else {
            ModeAvailability::Unavailable
        };
        let reason = match availability {
            ModeAvailability::Available => format!("{} encode and browser presentation verified", codec.label()),
            ModeAvailability::Experimental => {
                format!("{} negotiated, but decoded presentation is not verified", codec.label())
            }
            ModeAvailability::Provisional => {
                format!("browser reports {}, pending an end-to-end playback test", codec.label())
            }
            ModeAvailability::Unavailable if !host_detected => {
                format!("no {} encoder candidate was detected on this PC", mode.label())
            }
            ModeAvailability::Unavailable if !host_functional => candidates
                .iter()
                .find_map(|item| item.failure_reason.clone())
                .unwrap_or_else(|| format!("{} was detected but failed its initialization test", mode.label())),
            ModeAvailability::Unavailable => format!("the browser did not report {} receive support", codec.label()),
        };
        CompatibilityEntry {
            mode,
            codec,
            host_detected,
            host_functional,
            browser_reported: receiver.reported,
            negotiated: receiver.negotiated,
            presentation_verified: receiver.presented,
            availability,
            reason,
        }
    })
    .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchmarkMetrics {
    pub requested_fps: f64,
    pub encoded_fps: f64,
    pub presented_fps: Option<f64>,
    pub encode_mean_ms: f64,
    pub encode_p95_ms: f64,
    pub preprocess_mean_ms: f64,
    pub preprocess_p95_ms: f64,
    pub actual_mbps: f64,
    pub cpu_percent: Option<f64>,
    pub working_set_mib: Option<f64>,
    pub drop_percent: f64,
    pub freeze_count: Option<u64>,
    pub pipeline_p95_ms: Option<f64>,
    pub quality_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoScore {
    pub mode: EncoderMode,
    pub passed_gates: bool,
    pub score: Option<f64>,
    pub components: BTreeMap<String, f64>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBenchmarkObservation {
    pub schema_version: u32,
    pub nfidb_version: String,
    pub receiver_runtime: String,
    pub encoder_id: String,
    pub mode: EncoderMode,
    pub profile: VideoProfile,
    pub max_width: u32,
    pub requested_fps: u32,
    pub end_to_end_verified: bool,
    pub recorded_unix_ms: u128,
    pub metrics: BenchmarkMetrics,
    pub score: AutoScore,
}

pub const MAX_ENCODE_P95_FRACTION_OF_FRAME: f64 = 0.72;
pub const MIN_FPS_FRACTION: f64 = 0.92;
pub const MAX_DROP_PERCENT: f64 = 5.0;

#[must_use]
pub fn score_auto_candidate(mode: EncoderMode, metrics: &BenchmarkMetrics) -> AutoScore {
    let frame_budget_ms = 1000.0 / metrics.requested_fps.max(1.0);
    let actual_fps = metrics.presented_fps.unwrap_or(metrics.encoded_fps);
    let mut reasons = Vec::new();
    if actual_fps < metrics.requested_fps * MIN_FPS_FRACTION {
        reasons.push(format!(
            "{actual_fps:.1} fps missed the {:.1} fps gate",
            metrics.requested_fps * MIN_FPS_FRACTION
        ));
    }
    if metrics.encode_p95_ms > frame_budget_ms * MAX_ENCODE_P95_FRACTION_OF_FRAME {
        reasons.push(format!(
            "encode p95 {:.2} ms exceeded the interactive gate",
            metrics.encode_p95_ms
        ));
    }
    if metrics.drop_percent > MAX_DROP_PERCENT {
        reasons.push(format!(
            "{:.2}% drops exceeded the reliability gate",
            metrics.drop_percent
        ));
    }
    if metrics.freeze_count.is_some_and(|count| count > 0) {
        reasons.push("presentation freezes were observed".to_owned());
    }
    let passed_gates = reasons.is_empty();
    if !passed_gates {
        return AutoScore {
            mode,
            passed_gates,
            score: None,
            components: BTreeMap::new(),
            reasons,
        };
    }

    // The weights are deliberately transparent. Latency and stable frame delivery
    // dominate; bandwidth and host resources decide between otherwise healthy paths.
    let latency = (1.0 - metrics.encode_p95_ms / frame_budget_ms).clamp(0.0, 1.0) * 35.0;
    let stability = (actual_fps / metrics.requested_fps.max(1.0)).clamp(0.0, 1.0) * 25.0;
    // Interactive screen sharing normally targets well below 20 Mbps. This
    // scale makes a material HEVC/AV1 saving visible without allowing bitrate
    // to compensate for a failed latency gate.
    let bandwidth = (1.0 - metrics.actual_mbps / 20.0).clamp(0.0, 1.0) * 18.0;
    let cpu = metrics
        .cpu_percent
        .map_or(7.5, |value| (1.0 - value / 100.0).clamp(0.0, 1.0) * 10.0);
    let memory = metrics
        .working_set_mib
        .map_or(4.5, |value| (1.0 - value / 2048.0).clamp(0.0, 1.0) * 6.0);
    let quality = metrics.quality_score.map_or(3.0, |value| value.clamp(0.0, 1.0) * 6.0);
    let components = BTreeMap::from([
        ("latency".to_owned(), latency),
        ("frame_stability".to_owned(), stability),
        ("bandwidth".to_owned(), bandwidth),
        ("cpu".to_owned(), cpu),
        ("memory".to_owned(), memory),
        ("quality".to_owned(), quality),
    ]);
    let score = components.values().sum();
    AutoScore {
        mode,
        passed_gates,
        score: Some(score),
        components,
        reasons,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSettingsSnapshot {
    pub revision: u64,
    pub settings: VideoConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetVideoSettingsRequest {
    pub base_revision: u64,
    pub settings: VideoConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRuntimeStatus {
    pub requested_mode: EncoderMode,
    pub active_mode: EncoderMode,
    pub codec: VideoCodec,
    pub backend: EncoderBackend,
    pub encoder_name: String,
    pub hardware: bool,
    pub pipeline_memory_mode: PipelineMemoryMode,
    pub output_width: u32,
    pub output_height: u32,
    pub target_fps: u32,
    pub target_bitrate_bps: u32,
    pub restart_count: u64,
    pub switching: bool,
    pub auto_selection_reason: String,
    pub last_error: Option<String>,
}

pub trait VideoSettingsRuntime: Send + Sync + 'static {
    fn apply_video_settings(
        &self,
        settings: &VideoConfig,
        browser: &BrowserVideoCapabilities,
    ) -> Result<VideoRuntimeStatus, String>;
    fn video_runtime_status(&self) -> VideoRuntimeStatus;
    fn encoder_capabilities(&self) -> Vec<EncoderCapability>;
    fn request_video_keyframe(&self);
    fn record_auto_benchmark(&self, observation: AutoBenchmarkObservation) -> Result<(), String>;
    fn clear_auto_benchmarks(&self) -> Result<(), String>;
    fn auto_benchmark_results(&self) -> Vec<AutoBenchmarkObservation>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(mode: EncoderMode, state: CapabilityState) -> EncoderCapability {
        EncoderCapability {
            id: mode.label().to_owned(),
            codec: mode.codec().unwrap(),
            backend: if mode == EncoderMode::H264Software {
                EncoderBackend::OpenH264Software
            } else {
                EncoderBackend::MediaFoundationHardware
            },
            hardware: mode != EncoderMode::H264Software,
            encoder_name: mode.label().to_owned(),
            adapter_name: None,
            adapter_luid: None,
            vendor: None,
            driver_version: None,
            input_formats: vec!["NV12".to_owned()],
            profiles: Vec::new(),
            low_latency: None,
            rate_control: Vec::new(),
            maximum_tested_width: None,
            maximum_tested_height: None,
            maximum_tested_fps: None,
            state,
            failure_reason: None,
        }
    }

    #[test]
    fn capability_matrix_requires_both_ends() {
        let mut browser = BrowserVideoCapabilities::default();
        browser.h264.reported = true;
        browser.h264.presented = true;
        browser.hevc.reported = true;
        let matrix = compatibility_matrix(
            &[
                host(EncoderMode::H264Hardware, CapabilityState::Functional),
                host(EncoderMode::HevcHardware, CapabilityState::Functional),
            ],
            &browser,
        );
        assert_eq!(matrix[0].availability, ModeAvailability::Available);
        assert_eq!(matrix[1].availability, ModeAvailability::Provisional);
        assert_eq!(matrix[2].availability, ModeAvailability::Unavailable);
    }

    #[test]
    fn slow_efficient_codec_fails_before_scoring() {
        let metrics = BenchmarkMetrics {
            requested_fps: 60.0,
            encoded_fps: 60.0,
            encode_p95_ms: 24.0,
            actual_mbps: 4.0,
            ..BenchmarkMetrics::default()
        };
        let score = score_auto_candidate(EncoderMode::Av1Hardware, &metrics);
        assert!(!score.passed_gates);
        assert!(score.score.is_none());
    }

    #[test]
    fn efficient_healthy_codec_scores_higher() {
        let base = BenchmarkMetrics {
            requested_fps: 60.0,
            encoded_fps: 60.0,
            presented_fps: Some(60.0),
            encode_mean_ms: 2.0,
            encode_p95_ms: 3.0,
            actual_mbps: 10.0,
            cpu_percent: Some(7.0),
            working_set_mib: Some(150.0),
            ..BenchmarkMetrics::default()
        };
        let h264 = score_auto_candidate(EncoderMode::H264Hardware, &base);
        let hevc = score_auto_candidate(
            EncoderMode::HevcHardware,
            &BenchmarkMetrics {
                actual_mbps: 5.5,
                encode_p95_ms: 3.5,
                ..base
            },
        );
        assert!(hevc.score.unwrap() > h264.score.unwrap());
    }
}
