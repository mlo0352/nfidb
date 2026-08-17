//! LAN-only HTTP pairing, WebSocket diagnostics, and WebRTC transport.

mod server;
mod webrtc_session;

pub use server::{ServerHandle, ServerInfo, ServerOptions};
