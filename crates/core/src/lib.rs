//! Platform-neutral NFiDB session, configuration, and metrics core.

mod config;
mod metrics;
mod session;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use config::{AppConfig, CaptureMode, InputConfig, NetworkConfig, UiConfig, VideoConfig, VideoProfile};
pub use metrics::{Metrics, MetricsSnapshot};
use nfidb_protocol::{CommandInput, KeyboardInput, PointerBatch, TextInput, WheelInput};
pub use session::{PairMethod, PairResult, PublicSession, SessionError, SessionManager};

#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub data: Arc<[u8]>,
    pub duration: std::time::Duration,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
}

/// A shared, edge-triggered request for the encoder's next frame to be an IDR.
///
/// The transport raises this after a receiver is actually connected. The capture
/// thread consumes it immediately before encoding, avoiding a dependency from the
/// platform encoder back into WebRTC.
#[derive(Debug, Clone, Default)]
pub struct KeyframeRequest {
    pending: Arc<AtomicBool>,
}

impl KeyframeRequest {
    pub fn request(&self) {
        self.pending.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
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
    fn inject_wheel(&self, input: &WheelInput) -> Result<(), InputError>;
    fn inject_keyboard(&self, input: &KeyboardInput) -> Result<(), InputError>;
    fn inject_text(&self, input: &TextInput) -> Result<(), InputError>;
    fn inject_command(&self, input: &CommandInput) -> Result<(), InputError>;
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

    fn inject_wheel(&self, input: &WheelInput) -> Result<(), InputError> {
        tracing_fallback(&format!("wheel input {}", input.sequence));
        Ok(())
    }

    fn inject_keyboard(&self, input: &KeyboardInput) -> Result<(), InputError> {
        tracing_fallback(&format!("keyboard input {}: {}", input.sequence, input.code));
        Ok(())
    }

    fn inject_text(&self, input: &TextInput) -> Result<(), InputError> {
        tracing_fallback(&format!(
            "text input {}: {} UTF-8 bytes",
            input.sequence,
            input.text.len()
        ));
        Ok(())
    }

    fn inject_command(&self, input: &CommandInput) -> Result<(), InputError> {
        tracing_fallback(&format!("command input {}: {:?}", input.sequence, input.command));
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

#[cfg(test)]
mod tests {
    use super::KeyframeRequest;

    #[test]
    fn keyframe_requests_are_shared_and_edge_triggered() {
        let producer = KeyframeRequest::default();
        let consumer = producer.clone();
        assert!(!consumer.take());
        producer.request();
        assert!(consumer.take());
        assert!(!producer.take());
    }
}
