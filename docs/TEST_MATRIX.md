# Test matrix

Last updated: 2026-08-18. Development host: Windows 11 Pro x64 build 22631, Intel Core i9-13900K, Rust 1.97.1 (`x86_64-pc-windows-msvc`), Node 22.22.3, npm 10.9.8, Microsoft Edge headless. Profile/soak results are release `0.2.0`; startup, diagnostics, credential rotation, navigation, browser-upgrade, and physical regressions are the `0.3.1`–`0.3.4` candidates. Remote-input rows and the five-second comparison runs are the `0.4.0` alpha.

| Layer | Test | Result | Evidence |
| --- | --- | --- | --- |
| Protocol | Rust packet round trips, rejection, pressure/tilt clamps | PASS | `cargo test --workspace` |
| Remote-input protocol | Wheel, key-down/up, Unicode text, semantic command round trips plus malformed/oversize rejection | PASS | 4 Rust remote-message tests and TypeScript binary-layout vectors |
| Mapping | fit/fill/1:1 and negative-origin target coordinates | PASS | Rust plus 3 TypeScript geometry tests |
| Sessions/config | PIN, QR, invalid credential, disconnect, rotation/invalidation, TOML round trip | PASS | Core unit tests plus live reset/reconnect path |
| PIN entry | Six digits auto-submit once; paste is normalized; partial entry stays local | PASS | PIN normalization unit tests plus live-host E2E with no Connect click |
| Browser coalescing | Use coalesced samples exactly once and in chronological order | PASS | Exact binary packet test with pressure, tilt, twist, coordinates, and sequences |
| Browser lifecycle | Preserve exactly one primary Down/Up across fit letterboxes and unusual coalesced/duplicate events | PASS | Deterministic edge-entry, edge-exit, duplicate-Down, and coalesced-lifecycle tests |
| Browser volume | Ten-minute 240 Hz stroke encoding | PASS | 144,002 samples generated in 0.55 s by deterministic unit test; continuous batch/sample sequences |
| Native input quick | Inject realistic primary-contact `PT_PEN`; receive `WM_POINTER` pressure/tilt/button state | PASS | 4/4 exact events; pressure `[102, 512, 1024, 0]`; exact tilt/lifecycle; zero received barrel-flag samples |
| Native input sustained | User32 injection, transient queue backpressure, and reverse-chronological history recovery | PASS | Exact packaged v0.3.3 primary-tip run: 14,400/14,400 samples over 59.83 s at 240.66 Hz; 5,375 coalesced samples recovered; zero missing/excess/value/barrel error; full pressure and ±60° tilt ranges; 50 ms bounded `ERROR_NOT_READY` retry |
| Browser remote input | Option+Tab ordering, Delete/Backspace and Ctrl+Option+Delete distinction, Unicode text, normalized wheel, three-finger command, mouse hover/click | PASS | deterministic browser event tests |
| Native remote mapping | DOM key rows/modifiers/navigation/F1–F24/numpad, virtual-desktop mouse buttons, fractional wheel accumulation, held-state reset construction | PASS | Rust host unit tests |
| Browser build | strict TypeScript + Safari 16.4 Vite target | PASS | 24 Vitest tests across 6 files; typecheck and production build |
| HTTP/control | status, pairing, embedded assets, authenticated metrics/diagnostics, stats WebSocket | PASS | live and portable smoke tests |
| Diagnostic recorder | One-hertz raw capture, host synchronization, bounded retention, processed distributions | PASS | Unit tests plus 11/11 live samples over 10.003 s, zero discarded |
| Browser upgrades | A new EXE cannot reuse a prior immutable JavaScript/CSS payload | PASS | `index.html` is no-store and references content-hashed assets; extracted-archive smoke discovers and loads the hash |
| Desktop navigation | Session, Source, Input, Diagnostics, and App Setup sidebar pages | PASS | Each button selects a distinct rendered page; release GUI smoke |
| Credential reset | Manual and focused-window expiry rotation refresh PIN/QR and invalidate old session | PASS | Expiry unit test plus physical reset: session identity and QR bitmap changed, prior peer disconnected, and the iPad re-paired |
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
| Portable runtime | Extract ZIP and run with no repo assets or Node runtime | PASS | Embedded HTML/JS served and video encoded from extracted EXE; direct dependency audit found Windows system DLLs only |
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

The benchmark starts a real host, pairs through its LAN IPv4 URL, negotiates WebRTC, decodes H.264, sends synthetic coalesced Pointer Events plus mixed mouse/keyboard/text/gesture probes, reads authenticated host metrics, and writes JSON under ignored `build/benchmarks/`.
