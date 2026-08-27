use std::fs;
use std::path::Path;
use std::time::Instant;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use nfidb_core::{
    AutoScore, BenchmarkMetrics, EncoderCapability, EncoderMode, PipelineMemoryMode, VideoCodec, score_auto_candidate,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::gpu_preprocess::GpuBenchmarkPipeline;
use crate::{VideoEncoderConfig, VideoFrame, VideoFrameData, create_video_encoder};

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
    let geometries = [
        ("4k-to-720p-fast", 3840, 2160, 1280, 5_000_000),
        ("1080p-balanced", 1920, 1080, 1920, 10_000_000),
        ("4k-to-1080p-balanced", 3840, 2160, 1920, 10_000_000),
        ("4k-to-1440p-sharp", 3840, 2160, 2560, 18_000_000),
    ];
    geometries
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
            results.push(run_host_benchmark(case.clone(), mode, &capabilities));
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

fn run_host_benchmark(
    case: HostBenchmarkCase,
    mode: EncoderMode,
    capabilities: &[EncoderCapability],
) -> HostBenchmarkResult {
    let codec = mode.codec().unwrap_or(VideoCodec::H264);
    let candidate = capabilities
        .iter()
        .find(|candidate| candidate.mode() == mode && candidate.state.is_usable());
    let Some(candidate) = candidate else {
        let reason = capabilities
            .iter()
            .find(|candidate| candidate.mode() == mode)
            .and_then(|candidate| candidate.failure_reason.clone())
            .unwrap_or_else(|| format!("{} is unavailable on this PC", mode.label()));
        return empty_result(case, mode, codec, "skipped", Some(reason));
    };

    match run_functional_case(&case, mode, codec, &candidate.encoder_name) {
        Ok(result) => result,
        Err(error) => empty_result(case, mode, codec, "failed", Some(error)),
    }
}

fn run_functional_case(
    case: &HostBenchmarkCase,
    mode: EncoderMode,
    codec: VideoCodec,
    encoder_name: &str,
) -> Result<HostBenchmarkResult, String> {
    let output_width = case.source_width.min(case.max_width) & !1;
    let output_height =
        (((u64::from(case.source_height) * u64::from(output_width)) / u64::from(case.source_width)) as u32).max(2) & !1;
    let mut encoder = create_video_encoder(VideoEncoderConfig {
        codec,
        mode,
        width: output_width,
        height: output_height,
        max_fps: case.requested_fps,
        bitrate_bps: case.bitrate_bps,
    })?;
    encoder.request_keyframe()?;

    let resize_options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));
    let mut resizer = Resizer::new();
    let mut gpu_pipeline = if mode.requires_hardware() {
        Some(GpuBenchmarkPipeline::new(
            case.source_width,
            case.source_height,
            output_width,
            output_height,
            case.requested_fps,
        )?)
    } else {
        None
    };
    let background = render_background(case.source_width, case.source_height);
    let process_start = process_sample();
    let started = Instant::now();
    let mut preprocess_ms = Vec::with_capacity(case.frames as usize);
    let mut encode_ms = Vec::with_capacity(case.frames as usize);
    let mut working_sets = Vec::with_capacity(case.frames as usize);
    let mut encoded_frames = 0_u32;
    let mut keyframes = 0_u32;
    let mut bytes_encoded = 0_u64;
    let mut startup_to_first_frame_ms = None;

    for frame_number in 0..case.frames {
        let preprocess_started = Instant::now();
        let bgra = render_workload_frame(&background, case, frame_number);
        let encoded = if let Some(gpu_pipeline) = gpu_pipeline.as_mut() {
            let surface = gpu_pipeline.process_bgra(&bgra)?;
            preprocess_ms.push(preprocess_started.elapsed().as_secs_f64() * 1000.0);
            let encode_started = Instant::now();
            let encoded = encoder.encode(VideoFrame {
                width: output_width,
                height: output_height,
                data: VideoFrameData::D3D11Nv12(&surface),
            })?;
            encode_ms.push(encode_started.elapsed().as_secs_f64() * 1000.0);
            encoded
        } else {
            let (bgra, width, height) = resize_bgra(
                bgra,
                case.source_width,
                case.source_height,
                case.max_width,
                &mut resizer,
                &resize_options,
            )?;
            let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&bgra, (width as usize, height as usize)));
            preprocess_ms.push(preprocess_started.elapsed().as_secs_f64() * 1000.0);
            let encode_started = Instant::now();
            let encoded = encoder.encode(VideoFrame {
                width,
                height,
                data: VideoFrameData::I420(&yuv),
            })?;
            encode_ms.push(encode_started.elapsed().as_secs_f64() * 1000.0);
            encoded
        };
        if let Some(encoded) = encoded {
            if startup_to_first_frame_ms.is_none() {
                startup_to_first_frame_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            }
            encoded_frames += 1;
            keyframes += u32::from(encoded.keyframe);
            bytes_encoded += encoded.data.len() as u64;
        }
        if let Some(sample) = process_sample() {
            working_sets.push(sample.working_set_mib);
        }
    }
    let pipeline_memory_mode = encoder.pipeline_memory_mode();
    encoder.shutdown();
    let elapsed = started.elapsed().as_secs_f64().max(0.000_001);
    let process_end = process_sample();
    let actual_fps = f64::from(encoded_frames) / elapsed;
    let actual_mbps = bytes_encoded as f64 * 8.0 / elapsed / 1_000_000.0;
    let cpu_percent = process_start.zip(process_end).map(|(start, end)| {
        let process_seconds = (end.process_100ns.saturating_sub(start.process_100ns)) as f64 / 10_000_000.0;
        process_seconds
            / elapsed
            / f64::from(std::thread::available_parallelism().map_or(1, std::num::NonZero::get) as u32)
            * 100.0
    });
    let encode_mean = mean(&encode_ms);
    let encode_p95 = percentile(&encode_ms, 0.95);
    let preprocess_mean = mean(&preprocess_ms);
    let preprocess_p95 = percentile(&preprocess_ms, 0.95);
    let working_set_mean = mean(&working_sets);
    let working_set_peak = working_sets.iter().copied().reduce(f64::max);
    let metrics = BenchmarkMetrics {
        requested_fps: f64::from(case.requested_fps),
        encoded_fps: actual_fps,
        presented_fps: None,
        encode_mean_ms: encode_mean.unwrap_or_default(),
        encode_p95_ms: encode_p95.unwrap_or_default(),
        preprocess_mean_ms: preprocess_mean.unwrap_or_default(),
        preprocess_p95_ms: preprocess_p95.unwrap_or_default(),
        actual_mbps,
        cpu_percent,
        working_set_mib: working_set_mean,
        drop_percent: f64::from(case.frames.saturating_sub(encoded_frames)) / f64::from(case.frames.max(1)) * 100.0,
        freeze_count: None,
        pipeline_p95_ms: None,
        quality_score: None,
    };
    Ok(HostBenchmarkResult {
        case: case.clone(),
        mode,
        codec,
        backend: encoder.backend().label().to_owned(),
        encoder_name: encoder_name.to_owned(),
        hardware: mode.requires_hardware(),
        pipeline_memory_mode,
        state: "completed".to_owned(),
        reason: None,
        output_width,
        output_height,
        input_frames: case.frames,
        encoded_frames,
        keyframes,
        bytes_encoded,
        average_bytes_per_frame: (encoded_frames > 0).then_some(bytes_encoded as f64 / f64::from(encoded_frames)),
        actual_fps: Some(actual_fps),
        actual_mbps: Some(actual_mbps),
        preprocess_mean_ms: preprocess_mean,
        preprocess_p50_ms: percentile(&preprocess_ms, 0.50),
        preprocess_p95_ms: preprocess_p95,
        preprocess_p99_ms: percentile(&preprocess_ms, 0.99),
        encode_mean_ms: encode_mean,
        encode_p50_ms: percentile(&encode_ms, 0.50),
        encode_p95_ms: encode_p95,
        encode_p99_ms: percentile(&encode_ms, 0.99),
        encode_max_ms: encode_ms.iter().copied().reduce(f64::max),
        startup_to_first_frame_ms,
        process_cpu_percent: cpu_percent,
        working_set_mean_mib: working_set_mean,
        working_set_peak_mib: working_set_peak,
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
            "Media Foundation hardware".to_owned()
        } else {
            "OpenH264 software".to_owned()
        },
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
    let environment = serde_json::json!({
        "nfidb_version": report.nfidb_version,
        "operating_system": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "logical_processors": std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        "adapters": report.capabilities.iter().filter_map(|capability| capability.adapter_name.as_ref()).collect::<std::collections::BTreeSet<_>>(),
        "measurement_scope": "host preprocessing and encoder; hardware modes use CPU pattern generation/upload followed by the live D3D11 video-processor and MF surface path; no receiver decode or presentation metrics",
        "generated_unix_ms": report.generated_unix_ms,
    });
    fs::write(
        directory.join("environment.json"),
        serde_json::to_vec_pretty(&environment).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
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
    let mut csv = "case,workload,mode,codec,backend,memory_path,state,source,output,requested_fps,actual_fps,encode_mean_ms,encode_p95_ms,preprocess_mean_ms,preprocess_p95_ms,cpu_percent,working_set_mean_mib,working_set_peak_mib,actual_mbps,auto_score,reason\n".to_owned();
    for result in &report.results {
        let field = |value: Option<f64>| value.map(|value| format!("{value:.4}")).unwrap_or_default();
        let row = [
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
            field(result.working_set_mean_mib),
            field(result.working_set_peak_mib),
            field(result.actual_mbps),
            field(result.auto_score.as_ref().and_then(|score| score.score)),
            result.reason.as_deref().unwrap_or("").replace(',', ";"),
        ];
        csv.push_str(&row.join(","));
        csv.push('\n');
    }
    csv
}

fn report_markdown(report: &HostBenchmarkReport) -> String {
    let mut markdown = "# NFiDB codec benchmark\n\nHost-only deterministic benchmark. Hardware rows upload the deterministic pattern, then use the same D3D11 GPU preprocessing and Media Foundation surface path as live capture. Missing end-to-end values are unavailable, not zero.\n\n| Case | Workload | Encoder | Memory path | State | FPS | Encode p95 | CPU | RAM peak | Mbps | Auto score |\n| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n".to_owned();
    for result in &report.results {
        let f = |value: Option<f64>, suffix: &str| {
            value
                .map(|value| format!("{value:.2}{suffix}"))
                .unwrap_or_else(|| "—".to_owned())
        };
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.case.name,
            result.case.workload.label(),
            result.mode.label(),
            result.pipeline_memory_mode.label(),
            result.state,
            f(result.actual_fps, ""),
            f(result.encode_p95_ms, " ms"),
            f(result.process_cpu_percent, "%"),
            f(result.working_set_peak_mib, " MiB"),
            f(result.actual_mbps, ""),
            f(result.auto_score.as_ref().and_then(|score| score.score), ""),
        ));
    }
    markdown
}

fn render_background(width: u32, height: u32) -> Vec<u8> {
    let mut frame = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            let grid = if x % 64 == 0 || y % 64 == 0 { 36 } else { 0 };
            frame[offset] = ((x * 160 / width) as u8).saturating_add(28 + grid);
            frame[offset + 1] = ((y * 150 / height) as u8).saturating_add(24 + grid);
            frame[offset + 2] = 28_u8.saturating_add(grid);
            frame[offset + 3] = 255;
        }
    }
    frame
}

fn render_workload_frame(background: &[u8], case: &HostBenchmarkCase, frame_number: u32) -> Vec<u8> {
    let mut frame = background.to_vec();
    match case.workload {
        BenchmarkWorkload::StaticDetail => {
            draw_marker(&mut frame, case.source_width, case.source_height, frame_number);
        }
        BenchmarkWorkload::Drawing => {
            let steps = (frame_number % 240) + 1;
            for step in 0..steps {
                let x = case.source_width / 12 + step * (case.source_width * 5 / 6) / 240;
                let wave = ((step as f64 / 17.0).sin() * f64::from(case.source_height) * 0.18) as i32;
                let y = (case.source_height as i32 / 2 + wave).clamp(2, case.source_height as i32 - 3) as u32;
                paint_block(
                    &mut frame,
                    case.source_width,
                    case.source_height,
                    x,
                    y,
                    4,
                    [245, 245, 245, 255],
                );
            }
            draw_marker(&mut frame, case.source_width, case.source_height, frame_number);
        }
        BenchmarkWorkload::HighMotion => {
            let shift = frame_number.wrapping_mul(23) % case.source_width.max(1);
            for y in 0..case.source_height {
                for x in 0..case.source_width {
                    let offset = (y as usize * case.source_width as usize + x as usize) * 4;
                    let value = (((x + shift) / 16 + y / 16) & 1) as u8 * 210;
                    frame[offset] = value;
                    frame[offset + 1] = value.wrapping_add((frame_number * 3) as u8);
                    frame[offset + 2] = 255_u8.saturating_sub(value);
                }
            }
            draw_marker(&mut frame, case.source_width, case.source_height, frame_number);
        }
    }
    frame
}

fn draw_marker(frame: &mut [u8], width: u32, height: u32, frame_number: u32) {
    let block = (width / 64).clamp(8, 64);
    for bit in 0..16 {
        let color = if frame_number & (1 << bit) != 0 {
            [245, 245, 245, 255]
        } else {
            [8, 8, 8, 255]
        };
        paint_block(frame, width, height, block + bit * block, block, block, color);
        paint_block(
            frame,
            width,
            height,
            width.saturating_sub(block + (bit + 1) * block),
            height.saturating_sub(block * 2),
            block,
            color,
        );
    }
}

fn paint_block(frame: &mut [u8], width: u32, height: u32, x: u32, y: u32, size: u32, color: [u8; 4]) {
    for row in y.saturating_sub(size / 2)..(y + size).min(height) {
        for column in x.saturating_sub(size / 2)..(x + size).min(width) {
            let offset = (row as usize * width as usize + column as usize) * 4;
            frame[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn resize_bgra(
    bgra: Vec<u8>,
    source_width: u32,
    source_height: u32,
    max_width: u32,
    resizer: &mut Resizer,
    options: &ResizeOptions,
) -> Result<(Vec<u8>, u32, u32), String> {
    if source_width <= max_width {
        return Ok((bgra, source_width, source_height));
    }
    let width = max_width & !1;
    let height = (((u64::from(source_height) * u64::from(width)) / u64::from(source_width)) as u32).max(2) & !1;
    let source =
        ImageRef::new(source_width, source_height, &bgra, PixelType::U8x4).map_err(|error| error.to_string())?;
    let mut output = Image::new(width, height, PixelType::U8x4);
    resizer
        .resize(&source, &mut output, Some(options))
        .map_err(|error| error.to_string())?;
    Ok((output.into_vec(), width, height))
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
    let index = ((sorted.len() - 1) as f64 * quantile).ceil() as usize;
    sorted.get(index).copied()
}

#[derive(Clone, Copy)]
struct ProcessSample {
    process_100ns: u64,
    working_set_mib: f64,
}

fn process_sample() -> Option<ProcessSample> {
    unsafe {
        let process = GetCurrentProcess();
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user).ok()?;
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        GetProcessMemoryInfo(process, &mut memory, memory.cb).ok()?;
        Some(ProcessSample {
            process_100ns: filetime_u64(kernel).saturating_add(filetime_u64(user)),
            working_set_mib: memory.WorkingSetSize as f64 / (1024.0 * 1024.0),
        })
    }
}

const fn filetime_u64(value: FILETIME) -> u64 {
    (value.dwHighDateTime as u64) << 32 | value.dwLowDateTime as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_percentiles_are_stable() {
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.5), Some(3.0));
        assert_eq!(percentile(&[], 0.95), None);
    }

    #[test]
    fn full_matrix_has_all_workloads_and_geometries() {
        let cases = full_benchmark_cases(3);
        assert_eq!(cases.len(), 12);
        assert!(cases.iter().all(|case| case.frames == 3));
    }
}
