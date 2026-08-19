//! LAN-only HTTP pairing, WebSocket diagnostics, and WebRTC transport.

use std::time::Instant;

use nfidb_core::{InputSink, Metrics};
use nfidb_protocol::InputMessage;

mod diagnostics;
mod file_transfer;
mod server;
mod webrtc_session;

pub use diagnostics::{
    ClientDiagnosticSample, DiagnosticReport, DiagnosticSummary, Distribution, RecordedDiagnosticSample,
};
pub use file_transfer::{
    ActiveUpload, BrowserFileListing, CompletedTransfer, FileTransferManager, FileTransferOptions,
    FileTransferSnapshot, OutgoingFile, TransferDirection, TransferStats,
};
pub use server::{ServerHandle, ServerInfo, ServerOptions};

pub(crate) fn process_input_packet(input: &dyn InputSink, metrics: &Metrics, bytes: &[u8], source: &str) {
    let message = match InputMessage::decode(bytes) {
        Ok(message) => message,
        Err(error) => {
            tracing::warn!(%error, %source, "discarded invalid input packet");
            return;
        }
    };
    let inject_started = Instant::now();
    let (result, count) = match &message {
        InputMessage::Pointer(batch) => {
            metrics.input_batch(batch);
            (input.inject_batch(batch), batch.samples.len())
        }
        InputMessage::Wheel(wheel) => {
            metrics.wheel_input();
            (input.inject_wheel(wheel), 1)
        }
        InputMessage::Keyboard(keyboard) => {
            metrics.keyboard_input();
            (input.inject_keyboard(keyboard), 1)
        }
        InputMessage::Text(text) => {
            metrics.text_input(text);
            (input.inject_text(text), 1)
        }
        InputMessage::Command(command) => {
            metrics.command_input();
            (input.inject_command(command), 1)
        }
    };
    match result {
        Ok(()) => metrics.input_injected(count, inject_started.elapsed()),
        Err(error) => {
            metrics.input_error();
            tracing::warn!(%error, %source, "remote input injection failed");
        }
    }
}
