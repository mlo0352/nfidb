# Test matrix

Last updated: 2026-08-28. Windows development host: Windows 11 Pro x64 build 22631, Intel Core i9-13900K, NVIDIA GeForce RTX 4090. macOS development host: 2021 MacBook Pro, Apple M1 Pro arm64, macOS 27.0 beta build 26A5416b, Xcode 27.0 beta 6. Historical OpenH264 profile/soak results are release `0.2.0`; Windows multi-codec/D3D11 evidence is `0.6.0`–`0.7.0`; native Mac work begins in `0.8.0`.

| Layer | Test | Result | Evidence |
| --- | --- | --- | --- |
| Protocol | Rust packet round trips, rejection, pressure/tilt clamps | PASS | `cargo test --workspace` |
| Remote-input protocol | Wheel, key-down/up, Unicode text, semantic command round trips plus malformed/oversize rejection | PASS | 4 Rust remote-message tests and TypeScript binary-layout vectors |
| Mapping | fit/fill/1:1 and negative-origin target coordinates | PASS | Rust plus 3 TypeScript geometry tests |
| Sessions/config | PIN, QR, invalid credential, disconnect, rotation/invalidation, TOML round trip | PASS | Core unit tests plus live reset/reconnect path |
| Video config migration | Old `[video] profile/max_fps/cursor` config, new preset round trip, bounds, reset defaults | PASS | Core migration/validation tests |
| Capability matrix | Host/browser H.264-only, H.264+HEVC, all-codec common-mode calculation | PASS | Core matrix tests |
| Auto selection | Unsupported AV1, failed HEVC FPS gate, lower-bandwidth healthy HEVC, hardware fallback, stale encoder identity | PASS | Core and capture unit tests |
| Video setting synchronization | Host/iPad revision update, stale revision rejection, invalid remote values | PASS | Authenticated control tests and live Auto E2E |
| Input setting synchronization | iPad Touch/Gesture edits update the host-authoritative native injector gate; stale revisions reject | PASS | Browser control, server revision, and native injector-gate tests; user-run release build and native smoke passed |
| PIN entry | Six digits auto-submit once; paste is normalized; partial entry stays local | PASS | PIN normalization unit tests plus live-host E2E with no Connect click |
| Browser coalescing | Use coalesced samples exactly once and in chronological order | PASS | Exact binary packet test with pressure, tilt, twist, coordinates, and sequences |
| Browser lifecycle | Preserve exactly one primary Down/Up across fit letterboxes and unusual coalesced/duplicate events | PASS | Deterministic edge-entry, edge-exit, duplicate-Down, and coalesced-lifecycle tests |
| Browser volume | Ten-minute 240 Hz stroke encoding | PASS | 144,002 samples generated in 0.55 s by deterministic unit test; continuous batch/sample sequences |
| Native input quick | Inject realistic primary-contact `PT_PEN`; receive `WM_POINTER` pressure/tilt/button state | PASS | 4/4 exact events; pressure `[102, 512, 1024, 0]`; exact tilt/lifecycle; zero received barrel-flag samples |
| Native input sustained | User32 injection, transient queue backpressure, and reverse-chronological history recovery | PASS | Exact packaged v0.3.3 primary-tip run: 14,400/14,400 samples over 59.83 s at 240.66 Hz; 5,375 coalesced samples recovered; zero missing/excess/value/barrel error; full pressure and ±60° tilt ranges; 50 ms bounded `ERROR_NOT_READY` retry |
| Browser remote input | Option+Tab ordering, Delete/Backspace and Ctrl+Option+Delete distinction, Unicode text, normalized wheel, three-finger command, mouse hover/click | PASS | deterministic browser event tests |
| Native remote mapping | DOM key rows/modifiers/navigation/F1–F24/numpad, virtual-desktop mouse buttons, fractional wheel accumulation, held-state reset construction | PASS | Rust host unit tests |
| Browser build | strict TypeScript + Safari 16.4 Vite target | PASS | 39 Vitest tests across 7 files; typecheck and production build |
| Hardware encoder discovery | Enumerated candidate is activated, initialized, and must return encoded bytes before use | PASS | NVIDIA H.264/HEVC/AV1 MFT functional probes; Microsoft H264 MFT remains initializeable only |
| GPU video pipeline | Bounded WGC D3D texture copy, GPU BGRA→NV12 scale/conversion, direct DXGI MFT input with assisted fallback | PASS (NVIDIA) | Live monitor reported `gpu-zero-copy` with 184 captured/180 encoded frames; hardware-conditional test passed H.264/HEVC/AV1; optimized 1080p Quick rows all reported `gpu-zero-copy` |
| macOS build/tests | Apple-silicon compile, 11 native host tests, cross-platform app link, Swift runtime rpath, ad-hoc bundle signature | PASS | M1 Pro: 11/11 host tests plus strict Clippy; package extracted, signature verified, Info.plist/icon present, executable reports its version |
| macOS encoder discovery | VideoToolbox list plus real-time session initialization and active hardware-property verification | PASS (M1 Pro) | Apple H.264 and Apple HEVC report functional hardware; AV1 reports unavailable; OpenH264 remains functional fallback |
| macOS host Quick benchmark | Optimized deterministic 1080p drawing, H.264 HW / HEVC HW / OpenH264, corrected pipeline timing | PASS | H.264 101.58 fps capacity / 8.42 ms encode p95; HEVC 93.08 / 9.37 ms; OpenH264 60.26 / 15.14 ms; AV1 skipped as unavailable |
| macOS generated server smoke | Start embedded browser server and hardware encoder without Screen Recording access | PASS | 5 s: 196 generated frames, 195 hardware H.264 frames, zero stale-frame drops; status/runtime JSON written; LAN URL `192.168.1.209` |
| macOS permission/onboarding | First-run defaults, live Screen Recording/Accessibility checks, request/deep-link controls, missing-app recovery | PASS | The physical Mac user approved both permissions. A LaunchServices-launched capture process proved the ScreenCaptureKit grant; the app reports Accessibility from `AXIsProcessTrusted` rather than trusting the settings-list appearance. |
| macOS real monitor capture | ScreenCaptureKit IOSurface → VideoToolbox → WebRTC | PASS (H.264) | Physical M1 Pro run captured and hardware-H.264 encoded 557/557 real 1920×1080 frames in 15 seconds with zero newest-frame replacements or transport drops; the signed app subsequently presented that path in physical iPad Safari. HEVC presentation remains separate. |
| macOS static-screen RTP timing | Simulated capture output pause, then motion resumes after 3 s | PASS (automated) | The resumed H.264 frame itself advances exactly 270,000 ticks on the 90 kHz RTP clock; 60 fractional frame intervals total exactly 90,000 ticks without cumulative drift. Physical Safari idle/resume observation remains the field confirmation. |
| macOS Quartz remote input | Pencil pressure/tilt/twist, mouse/buttons/wheel, keyboard/text, Command+Tab, Mission Control, minimize | PARTIAL PASS | Accessibility is physically approved and physical iPad mouse/trackpad plus keyboard control passed. Pencil pressure/tilt/twist and drawing-app behavior remain; event construction, key maps, modifiers, state release, and semantic commands are automated. |
| macOS iPad Safari E2E | Real monitor presentation, codec verification, pointer/keyboard/Pencil/files/diagnostics | PARTIAL PASS | Signed app permissions, real-monitor H.264 presentation, mouse/trackpad, and keyboard passed on a physical iPad; Pencil pressure, HEVC, files, reconnect/sleep, and the long diagnostic run remain |
| Host codec matrix | Four modes × four geometries × three deterministic workloads | PASS | 48/48 rows completed in optimized mode; JSON/CSV/Markdown exports |
| Live codec switching | H.264 HW → AV1 HW → H.264 SW → Auto while capture stays alive | PASS | Edge Quick Auto E2E passed in 18.1 s; capture advanced throughout; authenticated session retained |
| HEVC receiver negotiation | Functional host HEVC plus receiver report/SDP/presentation | PARTIAL | Host encode passes; Edge 151 did not report H.265, so it was correctly excluded; physical Safari pending |
| AV1 receiver negotiation | Functional host AV1 plus receiver SDP and presentation | PASS (EDGE) | Edge negotiated AV1 profile 0 and presented frames; physical Safari pending |
| HTTP/control | status, pairing, embedded assets, authenticated metrics/diagnostics, stats WebSocket | PASS | live and portable smoke tests |
| File upload core | leaf-name confinement, reserved names, chunk SHA-256, stale offsets, lost-response idempotency, collision-safe finalization, cancellation | PASS | Rust manager tests plus browser SHA/chunk/retry/cancel tests |
| File download core | Explicit canonical Outbox file, no path serialization, complete and suffix/open/fixed byte ranges, concurrent per-file auto-clear | PASS | Rust queue/range tests prove completed streams clear independently while partial/interrupted streams remain |
| File HTTP/auth | Paired listing, mutation origin, unauthorized rejection, idempotent upload/finalize, cancel/download/auto-clear APIs | PASS | Packaged-release `scripts/file-transfer-smoke.ps1`; 3,146,237-byte upload plus two downloads totaling 7,340,252 bytes, exact hashes, partial retention, empty final Outbox, zero failures |
| File queue UI | Background paired-session polling, multi-file arrival notice/badge, Download all, default-on persisted cleanup preference, ASCII history labels | PASS | deterministic browser DOM/storage/URL tests plus strict production build |
| File QoS | Configured rate ceiling and drawing-contact pause | PASS | Default-limited real-host throughput plus deterministic active-Pen pacing test |
| File transfer on iPad Safari | Files picker, foreground/background behavior, Safari native download, cancel/retry | NOT RUN | Browser logic is deterministic; physical iPad Files/Photos and Safari download-manager check remains |
| Diagnostic recorder | One-hertz raw capture, host synchronization, bounded retention, processed distributions | PASS | Unit tests plus 11/11 live samples over 10.003 s, zero discarded |
| Browser upgrades | A new EXE cannot reuse a prior immutable JavaScript/CSS payload | PASS | `index.html` is no-store and references content-hashed assets; extracted-archive smoke discovers and loads the hash |
| Desktop navigation | Session, Source, Input, Files, Diagnostics, and App Setup sidebar pages | PASS | Each button selects a distinct rendered page; release GUI smoke |
| Windows GUI startup | Encoder discovery preserves the caller's COM apartment; the `wgpu` desktop creates its titled window and serves status from its actual bound port | PASS | Apartment regression unit test plus exact-archive `scripts/gui-smoke.ps1`; forced 49121 collision correctly discovered fallback 49122; hosted OpenGL 1.1 failure retained as evidence; published v0.6.1 is a required negative fixture |
| Credential lifetime/reset | Paired code remains valid, ordinary disconnect gets a fresh same-code grace period, manual/focused expiry rotation invalidates the old session | PASS | Session unit tests plus physical reset: session identity and QR bitmap changed, prior peer disconnected, and the iPad re-paired |
| WebRTC input | Reliable ordered DataChannel under simultaneous video | PASS | Every live scenario received/injected exact 240 Hz input with zero gaps, reordering, lifecycle errors, or buffered bytes |
| WebRTC mixed input | Mouse pointer, high-resolution wheel, Option+Tab key transitions, 17-byte Unicode commit, and three-finger semantic command beside a 240 Hz Pencil stream | PASS | Both `0.4.0` live comparisons observed exact counter deltas, zero input errors/backlog, and 1,202/1,202 sustained Pencil samples |
| WebRTC 4K→720p | 4K source and 4K receiver viewport, Fast profile | PASS | 10 s: ~54 encoded/53 decoded fps; zero RTP loss, decoder drops, freezes, marker mismatches, or transport drops |
| WebRTC 1080p | 1080p source/receiver, Balanced profile | PASS | 10 s: ~32 encoded/33 decoded fps; all integrity/input/network checks pass |
| WebRTC 4K→1080p | 4K source and receiver viewport, Balanced profile | PASS | 10 s: ~29 encoded/29 decoded fps; all integrity/input/network checks pass |
| WebRTC 4K→1440p | 4K source and receiver viewport, Sharp profile | PASS | 10 s: ~14 encoded/15 decoded fps; all integrity/input/network checks pass; CPU-limited |
| Active soak | One continuous pressure/angle stroke plus 4K-source video | PASS | 600.005 s; 144,002/144,002 samples at 240.001 Hz; 30,508 decoded frames; 728 integrity checks; all gap/loss/freeze/backlog/error counters zero |
| RTP/media clock | Frame shedding does not accumulate playback lag | PASS | Soak media time advanced 599.997 s; zero media-time regressions; max presented-frame gap 81.1 ms in headless Edge |
| Recovery IDR under load | Bound corruption recovery without allowing keyframes to dominate an overloaded encoder | PASS | Five-second recovery cadence; repeated 4K→1080p run ~27 encoded/~25 mean decoded fps, four interval keyframes, zero freezes/loss/gaps/skips |
| WebRTC startup | Join an already-running H.264 stream at an arbitrary frame | PASS | First pre-IDR delta rejected; requested IDR in 65.091 ms; first browser frame in 99.3 ms; automated limit 5 s |
| Safari first-frame recovery | Connected peer/video track without a presented frame requests fresh IDRs until presentation | PASS | Final packaged real-monitor test: 144.7 ms startup; forced request counter 0→1; host keyframe and browser decoded-keyframe counts each advanced; browser watchdog and DataChannel fallback covered |
| Safari post-start stall recovery | Unchanged media time, missing telemetry/control channel, or failed peer escalates from bounded IDR requests to a video-only peer rebuild | AUTOMATED PASS / FIELD RUN PENDING | Browser unit tests prove unchanged callbacks do not mask a stall and that reconnect occurs even with both control channels unavailable; repeated physical recovery remains a release gate |
| Portable runtime | Extract ZIP and run with no repo assets or Node runtime | PASS | User-run release validation: embedded HTML/JS served and video encoded from extracted EXE; direct dependency audit found Windows system DLLs only; post-build resume repeated packaging/smokes without recompilation |
| WGC monitor capture | Real desktop produces capture/encoded/decoded frames | PASS | Isolated 3840×2160 → 1920×1080 production path: first browser frame 96.4 ms, host IDR wait 62.549 ms, zero RTP loss/decoder drops/freezes/transport drops |
| Workspace | fmt, check all targets, Clippy `-D warnings`, all Rust tests | PASS | local Windows build and GitHub Actions |
| iPad Safari video/touch | Real LAN pairing, touch transport, initial video | PASS | Cache-safe v0.3.2 client identified itself, recorded at 1 Hz, and started physical video in 71 ms after rejecting one pre-IDR delta |
| iPad Safari Pencil | Real pressure/tilt/coalescing and primary-tip semantics | IN PROGRESS | 10,358 real samples carried pressure/tilt with zero packet gaps; barrel mapping and the observed fit-edge lifecycle defect are fixed; Paint/Rebelle behavior still needs user confirmation on v0.3.3 |
| iPad trackpad/keyboard | Pointer/buttons/wheel, hardware shortcuts, software keyboard/IME, reconnect reset | PASS | Physical iPad: responsive pointer and typing; Caps Lock, Shift, shifted symbols, Tab, Option+Tab, and minimize controls passed; Ctrl+Option+Delete reached the documented Windows secure-attention boundary |
| iPad three-finger gestures | next/previous app, Task View, minimize, Pencil suppression | PASS | Physical iPad app switching and minimize passed; deterministic recognizer covers all four directions and Pencil arbitration |
| Krita | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Rebelle | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Photoshop | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Stability | 30-minute active session | NOT RUN | 10-minute active soak passes; 30 minutes remains a stable-release gate |
| WAN-offline | Continue after disconnecting internet | NOT RUN | Both peers configure an empty ICE-server list and use no cloud service; physical cable/WAN-disconnect test remains |

Host `dropped_frames` means a stale capture/preprocess frame was intentionally replaced by a newer one. This is the bounded-latency policy and is distinct from `video_transport_drops`, RTP `packetsLost`, browser `framesDropped`, and compositor presentation drops. The latter categories are reported independently.

## Reproduce deterministic and native tests

```powershell
.\scripts\test.ps1
.\scripts\file-transfer-smoke.ps1
.\target\release\pointer-sink.exe --self-test
.\target\release\pointer-sink.exe --stress-test --samples 14400 --rate 240 --batch-size 4 --json-output .\build\pointer-stress-60s.json
```

The native sink must be the unobscured foreground target; Windows credential/security overlays deliberately intercept synthetic input.

## Reproduce live video/input benchmarks

```powershell
# All four 1080p/4K scenarios, ten seconds each
.\scripts\benchmark.ps1 -DurationSeconds 10

# Ten-minute maximum-throughput soak
.\scripts\benchmark.ps1 -DurationSeconds 600 -SkipBuild -ScenarioName 4k-to-720p-fast

# Extracted-archive/no-runtime-install check
.\scripts\build-release.ps1
.\scripts\portable-smoke.ps1
```

The end-to-end benchmark starts a real host, pairs through its LAN IPv4 URL, negotiates the selected mutually supported codec, sends synthetic coalesced Pointer Events plus mixed mouse/keyboard/text/gesture probes, reads authenticated host metrics, and writes labeled JSON/CSV/Markdown under ignored `build/benchmarks/`. Host-only mode compares every functional encoder without a browser.
