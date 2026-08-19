# Changelog

NFiDB is pre-release software. Releases remain GitHub prereleases until the physical drawing-app matrix, longer stability run, and hardware-encoding work are complete.

## 0.5.0 — 2026-08-19 — Alpha

### Added

- Bidirectional file transfer between the paired iPad and Windows without an iPad app, cloud service, or shared filesystem.
- A Windows Files page with an explicit outbound queue, Inbox access, enable/rate/size controls, live bandwidth, progress, outcome counters, and bounded recent history.
- An iPad Files panel with multi-file upload queueing, progress, cancel/retry, native Safari downloads, queue removal, checksums, and recent activity.
- One-MiB upload chunks with independent SHA-256 verification, offset reconciliation, retry, temporary staging, full-file hashing, and collision-safe final names.
- Streaming Windows-to-iPad downloads with single-range resume support and no whole-file browser or host buffering.
- `scripts/file-transfer-smoke.ps1`, covering authenticated listing, upload, cancellation cleanup, full download, byte-range download, and end-to-end checksums against a real headless host.

### Safety and performance

- Only explicitly queued Windows files are downloadable; Safari cannot browse the Windows filesystem. Incoming files are confined to the configured NFiDB Inbox with path and Windows reserved-name sanitization.
- Pairing rotation or disconnect cancels partial uploads and prevents an active download from continuing under the old session.
- Bulk traffic is rate-limited separately from input/video and pauses by default while a Pencil or touch contact is active.
- Transfer history is bounded to 100 records, active uploads to 32, the outbound queue to 1,000 entries, and the default per-file limit to 10 GiB.

### Automated evidence

- The packaged-release smoke transferred a 3,146,237-byte upload at 29.170 Mbps and a 5,243,017-byte download at 16.747 Mbps under the default 32 Mbps limiter; idempotent retry, complete/ranged SHA-256, cancellation, and authentication checks passed with zero failed transfers.
- Strict browser tests now total 30 across seven files. Rust coverage includes verified/out-of-order chunks, idempotent recovery, filename confinement, collision handling, ranges, queue path privacy, and origin enforcement.

## 0.4.0 — 2026-08-18 — Alpha

### Added

- iPad trackpad and mouse forwarding for cursor movement, buttons, and horizontal or vertical scrolling.
- Hardware-keyboard forwarding with held-key support, drawing-app shortcuts, Caps Lock, shifted symbols, and the mappings Option → Alt, Control → Ctrl, Return → Enter, and Delete → Backspace.
- A compact iPad keyboard panel for Unicode text, Esc, Tab, Backspace, Enter, app switching, Task View, and minimize.
- Three-finger gestures while touch forwarding is off: right/left switch Windows apps, up opens Task View, and down minimizes the foreground window.
- One-hertz client diagnostics for latency, bandwidth, decode/presentation timing, buffering, loss, input continuity, pressure, and tilt.
- Functional Session, Source, Input, Diagnostics, and App Setup pages in the Windows host.

### Fixed

- Apple Pencil primary contact is injected as the pen tip instead of a barrel/right-click action.
- Long strokes remain continuous across the visible video edge and preserve coalesced pressure/tilt samples.
- Six-digit PINs submit automatically; stale PIN and QR credentials can be rotated from the desktop app.
- Safari now requests fresh H.264 keyframes until it actually presents a frame, recovering from a connected session that is still waiting for its first decodable picture.
- The embedded browser entry point uses no-store HTML and content-hashed assets so a new EXE cannot load an old client bundle.

### Physical iPad check

- Confirmed responsive typing, Caps Lock, Shift, symbols, Tab, Option+Tab, trackpad/mouse control, three-finger app switching, and three-finger minimize.
- Ctrl+Option+Delete is forwarded as Ctrl+Alt+Delete, but Windows deliberately blocks synthetic input at the secure-attention screen. NFiDB does not bypass that security boundary.

### Release status

- Solid alpha. The transport, browser, Windows input, 1080p/4K video, recovery, package, and physical remote-control paths have broad coverage.
- Physical Apple Pencil behavior in individual drawing applications, a 30-minute active session, WAN-offline operation, and hardware video encoding remain release gates for a stable build.

## 0.3.4 — 2026-08-17

- Added automatic PIN submission after the sixth digit.

## 0.3.3 — 2026-08-17

- Corrected primary Pencil-tip injection and stroke lifecycle behavior at fitted-video edges.

## 0.3.2 — 2026-08-17

- Corrected Safari client bootstrap and made client diagnostics resilient to incomplete browser APIs.

## 0.3.1 — 2026-08-17

- Reduced video startup time and hardened keyframe recovery without flooding the software encoder.

## 0.3.0 — 2026-08-17

- Added detailed local diagnostics, credential rotation, functional desktop navigation, and browser cache protection.
