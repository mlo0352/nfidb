//! macOS capture, VideoToolbox encoding, and Quartz input adapters.

#[cfg(target_os = "macos")]
mod benchmark;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod hardware;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod monitors;
#[cfg(target_os = "macos")]
mod resource_monitor;
#[cfg(target_os = "macos")]
mod videotoolbox_encoder;

#[cfg(target_os = "macos")]
pub use benchmark::{
    BenchmarkWorkload, HostBenchmarkCase, HostBenchmarkReport, HostBenchmarkResult,
    full_benchmark_cases, quick_benchmark_cases, run_host_benchmark_suite,
    write_benchmark_exports,
};
#[cfg(target_os = "macos")]
pub use capture::{CaptureManager, CaptureStatus};
#[cfg(target_os = "macos")]
pub use hardware::discover_video_encoders;
#[cfg(target_os = "macos")]
pub use input::{PointerInjector, PointerInjectorOptions, set_per_monitor_dpi_awareness};
#[cfg(target_os = "macos")]
pub use monitors::{MonitorDescriptor, enumerate_monitors};
#[cfg(target_os = "macos")]
pub use resource_monitor::ProcessResourceMonitor;
