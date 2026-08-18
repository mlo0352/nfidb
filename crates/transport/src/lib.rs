//! LAN-only HTTP pairing, WebSocket diagnostics, and WebRTC transport.

mod diagnostics;
mod server;
mod webrtc_session;

pub use diagnostics::{
    ClientDiagnosticSample, DiagnosticReport, DiagnosticSummary, Distribution, RecordedDiagnosticSample,
};
pub use server::{ServerHandle, ServerInfo, ServerOptions};
