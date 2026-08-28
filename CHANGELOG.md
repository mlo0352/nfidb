# Changelog

NFiDB is pre-release software. Releases remain GitHub prereleases until the physical drawing-app/multi-codec matrix, longer stability run, and broader hardware coverage are complete.

## 0.8.0 — 2026-08-28 — Alpha

### Native macOS host

- Added an Apple-silicon macOS application using ScreenCaptureKit, IOSurface-backed newest-frame capture, VideoToolbox H.264/HEVC hardware encoding, and the existing OpenH264 compatibility path.
- Added runtime VideoToolbox enumeration and functional session probes. NFiDB verifies `UsingHardwareAcceleratedVideoEncoder` before claiming a hardware path; the tested M1 Pro exposes Apple H.264 and HEVC hardware and no AV1 encoder.
- Configured real-time compression, disabled frame reordering, bounded delayed frames, prioritized encoding speed where supported, preserved periodic parameter-set/keyframe recovery, and kept all capture/encoder queues bounded.
- Added Quartz Pencil/tablet, mouse, wheel, keyboard, Unicode text, Command+Tab, Mission Control, and minimize injection behind macOS Accessibility permission. Touch-on uses the first finger as the Mac pointer because macOS has no public general-purpose synthetic multitouch API.
- Added Screen Recording permission fallback so the app UI, input-only mode, and generated diagnostics can start before capture permission is granted.
- Added a first-run Setup and Help page on both hosts. Defaults and the configuration file are created automatically; macOS permission state is checked live with direct Request/Open Settings controls, and Windows explains its one-time Private-network firewall prompt.
- Added in-app first-session steps and platform-specific troubleshooting for LAN discovery, first frame, Pencil mode, touch/gestures, stale pairing, and macOS manual permission recovery.
- Corrected macOS permission reporting: an active ScreenCaptureKit stream is authoritative even if the legacy CoreGraphics preflight result is stale, and Accessibility now uses Apple's `AXIsProcessTrustedWithOptions` prompt so NFiDB registers in the correct list.
- Reworked the native desktop theme for accessibility with near-white secondary text, larger default and diagnostic type, persistent navigation fills, stronger card/control outlines, and larger Input controls.
- Replaced the macOS permission button cluster with two focused status cards. Each reports the permission NFiDB can actually use, explains stale approvals from earlier builds, and exposes one clear repair/settings action.
- Unified every desktop tab on a single high-contrast control system: pure-white button labels, 40-point action targets, readable sentence-case navigation, bright section labels, larger help text, and no remaining 8–11 point UI overrides.
- Removed the final platform-inherited desktop colors: all action and navigation buttons now use the same explicit dark-teal fill, bright-teal two-pixel border, white label, and readable sizing as the permission repair control. Page headings, setup titles, help text, subtitles, and secondary labels are explicitly white rather than gray.
- Locked the native wordmark to the established teal `NFi` / white `DB` brand colors instead of allowing macOS to inherit the `DB` foreground from its window theme.
- Ignore ScreenCaptureKit's normal status-only startup/change/shutdown samples instead of reporting a false missing-pixel-buffer capture error after an otherwise healthy run.
- Accept valid ScreenCaptureKit IOSurface frames when macOS 27 omits the optional frame-status attachment or labels the initial/static image Started or Idle. Explicit blank, suspended, and stopped samples remain excluded, preventing a paired iPad from waiting forever for its first decodable frame.
- Added `NFIDB_CODESIGN_IDENTITY` support to the Mac packager. Stable Apple signing prevents development rebuilds from appearing as unrelated apps to macOS privacy controls; CI retains an explicit ad-hoc fallback until Developer ID/notarization credentials are configured.
- Local Mac packaging now automatically reuses the installed NFiDB Apple identity, or another available stable Apple code-signing identity, when `NFIDB_CODESIGN_IDENTITY` is unset. Setup and release guidance also document Tahoe's required one-time reboot after a Screen Recording permission entry is removed or reset.
- Prefer physical macOS, Windows, and Linux LAN interfaces over VPN, overlay, and virtual adapters when choosing the Session URL and QR address. This prevents a Mac `utun` address from displacing its reachable `en0` Wi-Fi address.
- Added macOS-specific host and iPad labels, keyboard shortcuts, permission guidance, application icon, `.app` packaging, ad-hoc signing, checksum generation, CI, and GitHub prerelease artifacts.

### macOS evidence

- Native macOS crate: 10/10 tests pass on an M1 Pro running macOS 27.0 beta; the full cross-platform application links and headless test-pattern/server smoke passes.
- Corrected the Mac benchmark clock so deterministic source generation is excluded from encoder/preprocessor throughput and bitrate follows the media timeline rather than test-run wall time.
- Optimized Quick 1080p drawing results: H.264 hardware 101.58 fps capacity / 8.42 ms encode p95; HEVC hardware 93.08 fps / 9.37 ms; OpenH264 60.26 fps / 15.14 ms encode p95. Hardware used about 47 MiB peak RAM and 26–27% of one core during the measured pipeline; OpenH264 used 83 MiB and one full core. These are host-only measurements, not iPad presentation results.
- Added an arm64 app-bundle build that verifies its signature and runs from a downloaded-style ZIP without depending on an Xcode-only Swift runtime path.

### Still required before stable

- Physical Mac Screen Recording and Accessibility approval, real monitor capture, iPad Safari presentation, pressure-sensitive drawing-app behavior, and longer stability remain explicit field tests. No unperformed iPad or app result is claimed.

## 0.7.0 — 2026-08-22 — Alpha

### GPU video path

- Monitor capture now remains in D3D11: WGC frames are copied into a four-surface bounded GPU pool, scaled and converted from BGRA to NV12 by the D3D11 video processor, then submitted to D3D11-aware Media Foundation encoders as DXGI surface buffers.
- Added a driver-compatible `gpu-assisted` fallback. If an otherwise functional hardware MFT rejects direct DXGI input, NFiDB reads back the already resized/converted NV12 surface and keeps hardware encoding active. OpenH264 and all GPU failures retain the established CPU path.
- Preserved newest-frame replacement at both live pipeline boundaries. GPU surface exhaustion drops stale video instead of allocating without a bound or growing latency.
- Runtime diagnostics and benchmark rows now report the path actually used: `gpu-zero-copy`, `gpu-assisted`, or `cpu-preprocessing`.
- Hardware host benchmarks now exercise the D3D11 upload, GPU video processor, and Media Foundation surface path; software H.264 continues to measure the compatibility CPU path.
- Added a hardware-conditional test that feeds one GPU-preprocessed NV12 surface through every locally available H.264, HEVC, and AV1 hardware encoder.
- Validated the live monitor path as `gpu-zero-copy` on the development RTX 4090 and fed GPU NV12 surfaces successfully to its NVIDIA H.264, HEVC, and AV1 Media Foundation encoders. The 1080p drawing Quick run sustained well above the 60 fps gate for all three hardware paths; OpenH264 remained below it.

### Validation

- Preserved native Cargo and Clippy diagnostics in Windows PowerShell 5.1 validation transcripts instead of collapsing failures into a generic exit message.
- Made the bidirectional transfer smoke compatible with the .NET Framework runtime included with Windows PowerShell 5.1; its SHA-256 checks no longer require modern static .NET APIs.

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
- Updated the portable dependency audit for `setupapi.dll`, the signed Windows system component used by the new graphics adapter discovery path; third-party runtime DLLs remain rejected.
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
