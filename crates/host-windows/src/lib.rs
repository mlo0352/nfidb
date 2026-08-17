//! Windows capture and native synthetic pointer adapters.

mod capture;
mod input;
mod monitors;

pub use capture::{CaptureManager, CaptureStatus};
pub use input::{PointerInjector, PointerInjectorOptions, set_per_monitor_dpi_awareness};
pub use monitors::{MonitorDescriptor, enumerate_monitors};
