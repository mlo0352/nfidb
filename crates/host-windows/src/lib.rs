//! Windows capture and native synthetic pointer adapters.

mod benchmark;
mod capture;
mod encoder;
mod hardware;
mod input;
mod learned;
mod mf_encoder;
mod monitors;
mod resource_monitor;

pub use benchmark::{
    BenchmarkWorkload, HostBenchmarkCase, HostBenchmarkReport, HostBenchmarkResult, full_benchmark_cases,
    quick_benchmark_cases, run_host_benchmark_suite, write_benchmark_exports,
};
pub use capture::{CaptureManager, CaptureStatus};
pub use encoder::{EncodedPacket, VideoEncoder, VideoEncoderConfig, VideoFrame, create_video_encoder};
pub use hardware::discover_video_encoders;
pub use input::{PointerInjector, PointerInjectorOptions, set_per_monitor_dpi_awareness};
pub use mf_encoder::{HardwareEncodedFrame, MediaFoundationEncoder, functional_probe};
pub use monitors::{MonitorDescriptor, enumerate_monitors};
pub use resource_monitor::ProcessResourceMonitor;
