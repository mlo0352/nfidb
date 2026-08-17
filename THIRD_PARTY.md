# Third-party software

NFiDB is distributed under `MIT OR Apache-2.0`. Its exact resolved Rust and npm dependency graphs are recorded in `Cargo.lock` and `apps/ipad-web/package-lock.json`.

Direct shipped dependencies were selected from permissive-license projects:

| Component | Purpose | License |
| --- | --- | --- |
| axum, tokio, tower-http | HTTP/WebSocket runtime | MIT |
| webrtc-rs | WebRTC/ICE/DTLS/SRTP/DataChannel | MIT OR Apache-2.0 |
| windows-capture | Windows Graphics Capture integration | MIT |
| windows-sys | Windows API bindings | MIT OR Apache-2.0 |
| OpenH264 / openh264-rs | H.264 software encoder fallback | BSD-2-Clause; Cisco OpenH264 binary/patent terms may also apply to separately supplied binaries |
| eframe/egui | Windows host interface | MIT OR Apache-2.0 |
| fast_image_resize | Frame scaling | MIT OR Apache-2.0 |
| rust-embed | Embedded browser assets | MIT |
| mdns-sd | LAN service discovery | MIT OR Apache-2.0 |
| qrcode | Pairing QR generation | MIT OR Apache-2.0 |
| serde, serde_json, toml | Configuration and messages | MIT OR Apache-2.0 |
| clap | Command-line parsing | MIT OR Apache-2.0 |
| sha2, subtle, rand, base64, uuid | Session credentials and identifiers | MIT OR Apache-2.0 (some transitive packages also ISC/BSD) |
| Vite, TypeScript, Vitest | Browser build and tests | MIT |
| Playwright Test | Live browser integration test | Apache-2.0 |

No FFmpeg executable, GStreamer runtime, cloud SDK, analytics SDK, or GPL dependency is intentionally shipped in the core application.

This summary is not a replacement for the license files included in dependency source distributions. Before a public binary release, review the generated dependency graph and include any attribution files required by the exact resolved versions.
