//! Platform-neutral NFiDB session, configuration, and metrics core.

mod config;
mod metrics;
mod session;

use std::fmt;
use std::sync::Arc;

pub use config::{AppConfig, CaptureMode, InputConfig, NetworkConfig, UiConfig, VideoConfig, VideoProfile};
pub use metrics::{Metrics, MetricsSnapshot};
use nfidb_protocol::PointerBatch;
pub use session::{PairMethod, PairResult, PublicSession, SessionError, SessionManager};

#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub data: Arc<[u8]>,
    pub duration: std::time::Duration,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct InputError(pub String);

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InputError {}

pub trait InputSink: Send + Sync + 'static {
    fn inject_batch(&self, batch: &PointerBatch) -> Result<(), InputError>;
    fn reset_all(&self) -> Result<(), InputError>;
}

#[derive(Default)]
pub struct LoggingInputSink;

impl InputSink for LoggingInputSink {
    fn inject_batch(&self, batch: &PointerBatch) -> Result<(), InputError> {
        tracing_fallback(&format!(
            "input batch {}: {} samples",
            batch.batch_sequence,
            batch.samples.len()
        ));
        Ok(())
    }

    fn reset_all(&self) -> Result<(), InputError> {
        tracing_fallback("input state reset");
        Ok(())
    }
}

fn tracing_fallback(_message: &str) {
    #[cfg(debug_assertions)]
    eprintln!("{_message}");
}
