# Changelog

NFiDB is pre-release software. Releases remain GitHub prereleases until the physical drawing-app/multi-codec matrix, longer stability run, and broader hardware coverage are complete.

## Unreleased

## 0.6.2 — 2026-08-20 — Alpha

### Fixed

- Fixed the release GUI aborting before its first window with `RPC_E_CHANGED_MODE`. Media Foundation discovery now runs on a dedicated MTA worker instead of changing the GUI thread's COM apartment.
- Media Foundation COM initialization is now tracked per worker thread and balanced when that thread exits; the process-wide Media Foundation runtime remains separately initialized once.
- Disabled winit's unused Windows file-drop hook so an unrelated COM user cannot reintroduce an OLE apartment conflict at window creation.
- GUI startup errors and panics now write `%APPDATA%\NFiDB\startup-error.log` and display a native error dialog instead of disappearing without an explanation.

### Validation

- Added a real Windows GUI startup smoke that requires both an NFiDB window and a responsive local status API. It runs during user handoff validation, CI, and release publication.
- Made that smoke's Unicode title check reliable in Windows PowerShell 5.1 and PowerShell 7, and replaced `Compress-Archive` with a scanner-tolerant ZIP writer so temporary antivirus/indexer locks no longer print false-looking packaging errors.
- Fixed the GUI smoke's hosted-Windows port race: it no longer reserves and immediately releases the requested port, discovers the listener actually owned by NFiDB when the server selects a fallback port, bypasses loopback proxies, and retains actionable listener/stdout/stderr diagnostics in CI artifacts.
- Moved the Windows desktop shell from the OpenGL-only `glow` renderer to `wgpu`. NFiDB now uses the native Windows graphics stack or its software adapter instead of aborting on systems—such as clean hosted-Windows runners—that expose only OpenGL 1.1.
- Confirmed the regression test rejects the published v0.6.1 binary with the captured `OleInitialize` panic before accepting a replacement build.
- Validation now disables Cargo's transient progress renderer, prints the concrete failure reason, and can resume after source validation or after release compilation without repeating completed stages.

## 0.6.1 — 2026-08-20 — Withdrawn Alpha

The portable v0.6.1 GUI aborts during window creation because hardware discovery initializes the main thread as MTA before winit requests an OLE STA for file dropping. Headless mode is unaffected, but the release should not be used as a desktop build.

### Fixed

- The iPad Touch toggle now changes the host's real native-touch injector gate through an authenticated, revision-checked control instead of changing only a browser flag.
- Drawing near the remote screen's top edge no longer reopens the controls. The toolbar opens only from the compact bottom-left Controls button and can be closed immediately.

### Added

- Added branded Windows executable icon resources generated from the existing NFiDB mark.
- Added a user-run validation handoff that records the tested commit/tree, commands, results, package checksum, and log for a later Codex resume turn. Its post-build resume path skips npm and Cargo; unique staging and bounded scanner-lock retries keep packaging resilient while an older portable copy remains open.

## 0.6.0 — 2026-08-19 — Alpha

### Video pipeline

- Replaced the fixed OpenH264 assumption with a codec-neutral encoder interface and explicit H.264, HEVC/H.265, and AV1 identities.
- Added real Media Foundation hardware-MFT enumeration, DXGI adapter identification when exposed, media-type/low-latency probing, and an encoded-frame functional test. A transform is never labeled usable hardware merely because it was enumerated.
- Added working NVIDIA Media Foundation H.264, HEVC, and AV1 encoders while retaining OpenH264 as the compatibility fallback.
- Made WebRTC codec-aware with matching MIME/SDP/RTP packetization, keyframe gating, and controlled peer replacement on codec changes.
- Preserved bounded newest-frame replacement; live width/FPS/bitrate/preset changes rebuild only the encoder worker, not capture, pairing, HTTP, or the process.

### Auto, settings, and UI

- Added receiver capability discovery, codec preferences, SDP/negotiation tracking, and first-presented-frame verification. Browser-reported support alone is not treated as proof.
- Added transparent hard gates and a 100-point Auto score weighted toward latency and frame stability before bandwidth, CPU, memory, and quality.
- Added locally cached results keyed by build, receiver runtime, encoder identity, profile, width, and FPS, plus rerun/clear controls.
- Converted Fast/Balanced/Sharp into editable, validated preset data with optional per-codec bitrates and migration for existing `config.toml` files.
- Added synchronized, revision-checked, host-authoritative video controls to Windows and iPad. Both show availability reasons and the actual codec/backend/memory path; changes apply without restarting NFiDB.

### Benchmarking and diagnostics

- Added deterministic static-detail, drawing, and high-motion host workloads; Quick and Full suites; CPU/RAM/latency/throughput metrics; and JSON/CSV/Markdown exports.
- Extended `scripts/benchmark.ps1` with codec/profile/workload filters and explicitly labeled Microsoft Edge end-to-end results.
- Added codec/backend/adapter/memory-mode/restart/receiver/Auto fields to raw diagnostics and compact benchmark comparisons to both UIs.
- Verified H.264, HEVC, and AV1 hardware output on the development RTX 4090. A 48-row optimized host run and a live repeated-codec Edge Auto run completed; physical iPad Safari multi-codec validation remains pending.

### Known boundary

- Hardware encode is active, but WGC resize/color conversion and NV12 upload still use CPU memory. Diagnostics truthfully report `cpu-preprocessing`; a D3D surface/zero-copy path is not claimed.

## 0.5.1 — 2026-08-19 — Alpha

### Fixed

- Replaced unsupported arrow glyphs in transfer history with plain labels so the iPad and Windows no longer show square placeholder characters.
- The iPad now watches the Windows Outbox throughout the paired session, shows an arrival notice, and keeps a live queue-count badge even when the Files panel is closed.
- Completed Safari downloads can remove their Windows queue entries automatically. Cleanup is enabled by default, persists across page loads, and occurs only after a full stream; partial or interrupted downloads stay queued.

### Added

- A single-tap **Download all** action for batches queued from Windows, with independent server-side completion and cleanup for every file.
- A clearly labeled **Open received files folder** action on the Windows Files page.
- Concurrent, partial, interrupted, persisted-preference, notification, multi-file, and real-host auto-clear coverage.

### Automated evidence

- The real-host smoke uploaded 3,146,237 bytes and downloaded two files totaling 7,340,252 bytes with exact SHA-256 matches, retained a partial range, then auto-cleared both completed files with zero failures.
- Browser coverage now totals 32 tests across seven files; the Rust workspace totals 34 tests, including concurrent full-download cleanup and interrupted/ranged retention.

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
