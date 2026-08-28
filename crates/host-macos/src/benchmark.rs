use std::fs;
use std::path::Path;
use std::time::Instant;

use nfidb_core::{
    AutoScore, BenchmarkMetrics, EncoderCapability, EncoderMode, PipelineMemoryMode, VideoCodec, score_auto_candidate,
};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, Level, Profile, RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use serde::{Deserialize, Serialize};

use crate::capture::bgra_iosurface;
use crate::videotoolbox_encoder::{VideoToolboxEncoder, VideoToolboxEncoderConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkWorkload {
    StaticDetail,
    Drawing,
    HighMotion,
}

impl BenchmarkWorkload {
    pub const ALL: [Self; 3] = [Self::StaticDetail, Self::Drawing, Self::HighMotion];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::StaticDetail => "static-detail",
            Self::Drawing => "drawing",
            Self::HighMotion => "high-motion",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostBenchmarkCase {
    pub name: String,
    pub source_width: u32,
    pub source_height: u32,
    pub max_width: u32,
    pub requested_fps: u32,
    pub bitrate_bps: u32,
    pub workload: BenchmarkWorkload,
    pub frames: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostBenchmarkResult {
    pub case: HostBenchmarkCase,
    pub mode: EncoderMode,
    pub codec: VideoCodec,
    pub backend: String,
    pub encoder_name: String,
    pub hardware: bool,
    pub pipeline_memory_mode: PipelineMemoryMode,
    pub state: String,
    pub reason: Option<String>,
    pub output_width: u32,
    pub output_height: u32,
    pub input_frames: u32,
    pub encoded_frames: u32,
    pub keyframes: u32,
    pub bytes_encoded: u64,
    pub average_bytes_per_frame: Option<f64>,
    pub actual_fps: Option<f64>,
    pub actual_mbps: Option<f64>,
    pub preprocess_mean_ms: Option<f64>,
    pub preprocess_p50_ms: Option<f64>,
    pub preprocess_p95_ms: Option<f64>,
    pub preprocess_p99_ms: Option<f64>,
    pub encode_mean_ms: Option<f64>,
    pub encode_p50_ms: Option<f64>,
    pub encode_p95_ms: Option<f64>,
    pub encode_p99_ms: Option<f64>,
    pub encode_max_ms: Option<f64>,
    pub startup_to_first_frame_ms: Option<f64>,
    pub process_cpu_percent: Option<f64>,
    pub working_set_mean_mib: Option<f64>,
    pub working_set_peak_mib: Option<f64>,
    pub auto_score: Option<AutoScore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostBenchmarkReport {
    pub schema_version: u32,
    pub nfidb_version: String,
    pub generated_unix_ms: u128,
    pub benchmark_kind: String,
    pub capabilities: Vec<EncoderCapability>,
    pub results: Vec<HostBenchmarkResult>,
}

#[must_use]
pub fn quick_benchmark_cases(frames: u32) -> Vec<HostBenchmarkCase> {
    vec![HostBenchmarkCase {
        name: "1080p-balanced-drawing".to_owned(),
        source_width: 1920,
        source_height: 1080,
        max_width: 1920,
        requested_fps: 60,
        bitrate_bps: 10_000_000,
        workload: BenchmarkWorkload::Drawing,
        frames,
    }]
}

#[must_use]
pub fn full_benchmark_cases(frames: u32) -> Vec<HostBenchmarkCase> {
    [
        ("4k-to-720p-fast", 3840, 2160, 1280, 5_000_000),
        ("1080p-balanced", 1920, 1080, 1920, 10_000_000),
        ("4k-to-1080p-balanced", 3840, 2160, 1920, 10_000_000),
        ("4k-to-1440p-sharp", 3840, 2160, 2560, 18_000_000),
    ]
    .into_iter()
    .flat_map(|(name, source_width, source_height, max_width, bitrate_bps)| {
        BenchmarkWorkload::ALL
            .into_iter()
            .map(move |workload| HostBenchmarkCase {
                name: format!("{name}-{}", workload.label()),
                source_width,
                source_height,
                max_width,
                requested_fps: 60,
                bitrate_bps,
                workload,
                frames,
            })
    })
    .collect()
}

pub fn run_host_benchmark_suite(
    capabilities: Vec<EncoderCapability>,
    cases: &[HostBenchmarkCase],
    requested_modes: &[EncoderMode],
) -> HostBenchmarkReport {
    let mut results = Vec::new();
    for case in cases {
        for &mode in requested_modes {
            results.push(run_case(case.clone(), mode, &capabilities));
        }
    }
    HostBenchmarkReport {
        schema_version: 1,
        nfidb_version: env!("CARGO_PKG_VERSION").to_owned(),
        generated_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        benchmark_kind: "host-encoder".to_owned(),
        capabilities,
        results,
    }
}

fn run_case(case: HostBenchmarkCase, mode: EncoderMode, capabilities: &[EncoderCapability]) -> HostBenchmarkResult {
    let codec = mode.codec().unwrap_or(VideoCodec::H264);
    let candidate = capabilities
        .iter()
        .find(|item| item.mode() == mode && item.state.is_usable());
    let Some(candidate) = candidate else {
        let reason = capabilities
            .iter()
            .find(|item| item.mode() == mode)
            .and_then(|item| item.failure_reason.clone())
            .unwrap_or_else(|| format!("{} is unavailable on this Mac", mode.label()));
        return empty_result(case, mode, codec, "skipped", Some(reason));
    };
    run_functional_case(&case, mode, codec, &candidate.encoder_name)
        .unwrap_or_else(|error| empty_result(case, mode, codec, "failed", Some(error)))
}

fn run_functional_case(
    case: &HostBenchmarkCase,
    mode: EncoderMode,
    codec: VideoCodec,
    encoder_name: &str,
) -> Result<HostBenchmarkResult, String> {
    let width = case.source_width.min(case.max_width).max(2) & !1;
    let height =
        (((u64::from(case.source_height) * u64::from(width)) / u64::from(case.source_width)) as u32).max(2) & !1;
    let mut hardware = if mode.requires_hardware() {
        Some(VideoToolboxEncoder::new(VideoToolboxEncoderConfig {
            codec,
            width,
            height,
            max_fps: case.requested_fps,
            bitrate_bps: case.bitrate_bps,
        })?)
    } else {
        None
    };
    let mut software = if mode == EncoderMode::H264Software {
        Some(
            Encoder::with_api_config(
                OpenH264API::from_source(),
                EncoderConfig::new()
                    .bitrate(BitRate::from_bps(case.bitrate_bps))
                    .max_frame_rate(FrameRate::from_hz(case.requested_fps as f32))
                    .rate_control_mode(RateControlMode::Bitrate)
                    .usage_type(UsageType::ScreenContentRealTime)
                    .profile(Profile::Baseline)
                    .level(Level::Level_4_1)
                    .complexity(Complexity::Low)
                    .skip_frames(true),
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if let Some(encoder) = hardware.as_mut() {
        encoder.request_keyframe();
    }
    if let Some(encoder) = software.as_mut() {
        encoder.force_intra_frame();
    }

    let mut preprocess_ms = Vec::with_capacity(case.frames as usize);
    let mut encode_ms = Vec::with_capacity(case.frames as usize);
    let mut measured_cpu_seconds = 0.0;
    let mut encoded_frames = 0_u32;
    let mut keyframes = 0_u32;
    let mut bytes_encoded = 0_u64;
    let mut startup_to_first_frame_ms = None;
    for frame_number in 0..case.frames {
        // Pattern generation models the source supplied by ScreenCaptureKit; it
        // is deliberately outside the encoder/preprocessor timing window.
        let bgra = render_frame(width, height, frame_number, case.workload);
        let cpu_started = cpu_seconds();
        let preprocess_started = Instant::now();
        if let Some(encoder) = hardware.as_mut() {
            let surface = bgra_iosurface(width, height, &bgra)?;
            preprocess_ms.push(preprocess_started.elapsed().as_secs_f64() * 1000.0);
            let encode_started = Instant::now();
            let encoded = encoder.encode(&surface)?;
            encode_ms.push(encode_started.elapsed().as_secs_f64() * 1000.0);
            encoded_frames += 1;
            keyframes += u32::from(encoded.keyframe);
            bytes_encoded += encoded.data.len() as u64;
        } else if let Some(encoder) = software.as_mut() {
            let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&bgra, (width as usize, height as usize)));
            preprocess_ms.push(preprocess_started.elapsed().as_secs_f64() * 1000.0);
            let encode_started = Instant::now();
            let encoded = encoder.encode(&yuv).map_err(|error| error.to_string())?;
            encode_ms.push(encode_started.elapsed().as_secs_f64() * 1000.0);
            if encoded.frame_type() != FrameType::Skip {
                encoded_frames += 1;
                keyframes += u32::from(matches!(encoded.frame_type(), FrameType::IDR | FrameType::I));
                bytes_encoded += encoded.to_vec().len() as u64;
            }
        }
        if let (Some(before), Some(after)) = (cpu_started, cpu_seconds()) {
            measured_cpu_seconds += (after - before).max(0.0);
        }
        if startup_to_first_frame_ms.is_none() && encoded_frames > 0 {
            startup_to_first_frame_ms = Some(preprocess_ms.iter().sum::<f64>() + encode_ms.iter().sum::<f64>());
        }
    }
    let measured_pipeline_seconds = (preprocess_ms.iter().sum::<f64>() + encode_ms.iter().sum::<f64>()) / 1000.0;
    let actual_fps = f64::from(encoded_frames) / measured_pipeline_seconds.max(0.000_001);
    let media_seconds = f64::from(case.frames) / f64::from(case.requested_fps.max(1));
    let actual_mbps = bytes_encoded as f64 * 8.0 / media_seconds.max(0.000_001) / 1_000_000.0;
    // Like Activity Monitor, 100% represents one fully occupied CPU core.
    let process_cpu_percent = Some(measured_cpu_seconds / measured_pipeline_seconds.max(0.000_001) * 100.0);
    let peak_mib = peak_working_set_mib();
    let metrics = BenchmarkMetrics {
        requested_fps: f64::from(case.requested_fps),
        encoded_fps: actual_fps,
        presented_fps: None,
        encode_mean_ms: mean(&encode_ms).unwrap_or_default(),
        encode_p95_ms: percentile(&encode_ms, 0.95).unwrap_or_default(),
        preprocess_mean_ms: mean(&preprocess_ms).unwrap_or_default(),
        preprocess_p95_ms: percentile(&preprocess_ms, 0.95).unwrap_or_default(),
        actual_mbps,
        cpu_percent: process_cpu_percent,
        working_set_mib: peak_mib,
        drop_percent: f64::from(case.frames.saturating_sub(encoded_frames)) / f64::from(case.frames.max(1)) * 100.0,
        freeze_count: None,
        pipeline_p95_ms: None,
        quality_score: None,
    };
    Ok(HostBenchmarkResult {
        case: case.clone(),
        mode,
        codec,
        backend: if mode.requires_hardware() {
            "VideoToolbox hardware"
        } else {
            "OpenH264 software"
        }
        .to_owned(),
        encoder_name: encoder_name.to_owned(),
        hardware: mode.requires_hardware(),
        pipeline_memory_mode: if mode.requires_hardware() {
            PipelineMemoryMode::GpuAssisted
        } else {
            PipelineMemoryMode::CpuPreprocessing
        },
        state: "completed".to_owned(),
        reason: None,
        output_width: width,
        output_height: height,
        input_frames: case.frames,
        encoded_frames,
        keyframes,
        bytes_encoded,
        average_bytes_per_frame: (encoded_frames > 0).then_some(bytes_encoded as f64 / f64::from(encoded_frames)),
        actual_fps: Some(actual_fps),
        actual_mbps: Some(actual_mbps),
        preprocess_mean_ms: mean(&preprocess_ms),
        preprocess_p50_ms: percentile(&preprocess_ms, 0.50),
        preprocess_p95_ms: percentile(&preprocess_ms, 0.95),
        preprocess_p99_ms: percentile(&preprocess_ms, 0.99),
        encode_mean_ms: mean(&encode_ms),
        encode_p50_ms: percentile(&encode_ms, 0.50),
        encode_p95_ms: percentile(&encode_ms, 0.95),
        encode_p99_ms: percentile(&encode_ms, 0.99),
        encode_max_ms: encode_ms.iter().copied().reduce(f64::max),
        startup_to_first_frame_ms,
        process_cpu_percent,
        working_set_mean_mib: peak_mib,
        working_set_peak_mib: peak_mib,
        auto_score: Some(score_auto_candidate(mode, &metrics)),
    })
}

fn empty_result(
    case: HostBenchmarkCase,
    mode: EncoderMode,
    codec: VideoCodec,
    state: &str,
    reason: Option<String>,
) -> HostBenchmarkResult {
    HostBenchmarkResult {
        case,
        mode,
        codec,
        backend: if mode.requires_hardware() {
            "VideoToolbox hardware"
        } else {
            "OpenH264 software"
        }
        .to_owned(),
        encoder_name: mode.label().to_owned(),
        hardware: mode.requires_hardware(),
        pipeline_memory_mode: PipelineMemoryMode::CpuPreprocessing,
        state: state.to_owned(),
        reason,
        output_width: 0,
        output_height: 0,
        input_frames: 0,
        encoded_frames: 0,
        keyframes: 0,
        bytes_encoded: 0,
        average_bytes_per_frame: None,
        actual_fps: None,
        actual_mbps: None,
        preprocess_mean_ms: None,
        preprocess_p50_ms: None,
        preprocess_p95_ms: None,
        preprocess_p99_ms: None,
        encode_mean_ms: None,
        encode_p50_ms: None,
        encode_p95_ms: None,
        encode_p99_ms: None,
        encode_max_ms: None,
        startup_to_first_frame_ms: None,
        process_cpu_percent: None,
        working_set_mean_mib: None,
        working_set_peak_mib: None,
        auto_score: None,
    }
}

pub fn write_benchmark_exports(directory: &Path, report: &HostBenchmarkReport) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    fs::write(directory.join("environment.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "nfidb_version": report.nfidb_version,
        "operating_system": "macOS",
        "architecture": std::env::consts::ARCH,
        "measurement_scope": "deterministic host preprocessing and real VideoToolbox/OpenH264 encoding; no receiver decode or presentation metrics",
        "generated_unix_ms": report.generated_unix_ms,
    })).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    fs::write(
        directory.join("capabilities.json"),
        serde_json::to_vec_pretty(&report.capabilities).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("results.json"),
        serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("results.csv"), report_csv(report)).map_err(|error| error.to_string())?;
    fs::write(directory.join("summary.md"), report_markdown(report)).map_err(|error| error.to_string())?;
    Ok(())
}

fn report_csv(report: &HostBenchmarkReport) -> String {
    let mut csv = "case,workload,mode,codec,backend,memory_path,state,source,output,requested_fps,actual_fps,encode_mean_ms,encode_p95_ms,preprocess_mean_ms,preprocess_p95_ms,cpu_percent,working_set_peak_mib,actual_mbps,auto_score,reason\n".to_owned();
    let field = |value: Option<f64>| value.map(|value| format!("{value:.4}")).unwrap_or_default();
    for result in &report.results {
        csv.push_str(
            &[
                result.case.name.clone(),
                result.case.workload.label().to_owned(),
                format!("{:?}", result.mode),
                format!("{:?}", result.codec),
                result.backend.clone(),
                result.pipeline_memory_mode.label().to_owned(),
                result.state.clone(),
                format!("{}x{}", result.case.source_width, result.case.source_height),
                format!("{}x{}", result.output_width, result.output_height),
                result.case.requested_fps.to_string(),
                field(result.actual_fps),
                field(result.encode_mean_ms),
                field(result.encode_p95_ms),
                field(result.preprocess_mean_ms),
                field(result.preprocess_p95_ms),
                field(result.process_cpu_percent),
                field(result.working_set_peak_mib),
                field(result.actual_mbps),
                field(result.auto_score.as_ref().and_then(|score| score.score)),
                result.reason.as_deref().unwrap_or("").replace(',', ";"),
            ]
            .join(","),
        );
        csv.push('\n');
    }
    csv
}

fn report_markdown(report: &HostBenchmarkReport) -> String {
    let mut markdown = "# NFiDB macOS codec benchmark\n\nHost-only deterministic benchmark. Missing end-to-end values are unavailable, not zero.\n\n| Case | Encoder | Memory path | State | FPS | Encode p95 | CPU | RAM peak | Mbps | Score |\n| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n".to_owned();
    let f = |value: Option<f64>, suffix: &str| {
        value
            .map(|value| format!("{value:.2}{suffix}"))
            .unwrap_or_else(|| "—".to_owned())
    };
    for result in &report.results {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.case.name,
            result.mode.label(),
            result.pipeline_memory_mode.label(),
            result.state,
            f(result.actual_fps, ""),
            f(result.encode_p95_ms, " ms"),
            f(result.process_cpu_percent, "%"),
            f(result.working_set_peak_mib, " MiB"),
            f(result.actual_mbps, ""),
            f(result.auto_score.as_ref().and_then(|score| score.score), "")
        ));
    }
    markdown
}

fn render_frame(width: u32, height: u32, frame: u32, workload: BenchmarkWorkload) -> Vec<u8> {
    let mut bytes = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            let grid = u8::from(x % 64 == 0 || y % 64 == 0) * 36;
            let motion = match workload {
                BenchmarkWorkload::StaticDetail => 0,
                BenchmarkWorkload::Drawing => u8::from(x.abs_diff(frame.wrapping_mul(17) % width.max(1)) < 5) * 200,
                BenchmarkWorkload::HighMotion => (((x + frame * 23) / 16 + y / 16) & 1) as u8 * 190,
            };
            bytes[offset] = ((x * 150 / width.max(1)) as u8)
                .saturating_add(grid)
                .saturating_add(motion);
            bytes[offset + 1] = ((y * 140 / height.max(1)) as u8).saturating_add(grid);
            bytes[offset + 2] = 30_u8.saturating_add(motion / 2);
            bytes[offset + 3] = 255;
        }
    }
    bytes
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn percentile(values: &[f64], quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
        .get(((sorted.len() - 1) as f64 * quantile).ceil() as usize)
        .copied()
}

fn cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    Some(
        usage.ru_utime.tv_sec as f64
            + usage.ru_utime.tv_usec as f64 / 1_000_000.0
            + usage.ru_stime.tv_sec as f64
            + usage.ru_stime.tv_usec as f64 / 1_000_000.0,
    )
}

fn peak_working_set_mib() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    Some(unsafe { usage.assume_init() }.ru_maxrss.max(0) as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_matrix_covers_all_workloads() {
        assert_eq!(full_benchmark_cases(3).len(), 12);
    }

    #[test]
    fn percentiles_are_stable() {
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.5), Some(3.0));
    }
}
