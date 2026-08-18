# Test matrix

Last updated: 2026-08-17. Development host: Windows 11 Pro x64 build 22631, Intel Core i9-13900K, Rust 1.97.1 (`x86_64-pc-windows-msvc`), Node 22.22.3, npm 10.9.8, Microsoft Edge headless. All results below are release `0.2.0` unless stated otherwise.

| Layer | Test | Result | Evidence |
| --- | --- | --- | --- |
| Protocol | Rust packet round trips, rejection, pressure/tilt clamps | PASS | `cargo test --workspace` |
| Mapping | fit/fill/1:1 and negative-origin target coordinates | PASS | Rust plus 3 TypeScript geometry tests |
| Sessions/config | PIN, QR, invalid credential, disconnect, TOML round trip | PASS | Core unit tests |
| Browser coalescing | Use coalesced samples exactly once and in chronological order | PASS | Exact binary packet test with pressure, tilt, twist, coordinates, and sequences |
| Browser volume | Ten-minute 240 Hz stroke encoding | PASS | 144,002 samples generated in 0.55 s by deterministic unit test; continuous batch/sample sequences |
| Native input quick | Inject synthetic `PT_PEN`; receive `WM_POINTER` pressure/tilt | PASS | 4/4 exact events; pressure `[102, 512, 1024, 0]`; exact tilt and lifecycle |
| Native input sustained | User32 injection and reverse-chronological history recovery | PASS | 14,400/14,400 exact samples over 59.93 s at 240.27 Hz; 4,846 coalesced samples recovered; zero missing/excess/value error; full pressure and ±60° tilt ranges |
| Browser build | strict TypeScript + Safari 16.4 Vite target | PASS | 7 Vitest tests; typecheck and production build |
| HTTP/control | status, pairing, embedded assets, authenticated metrics, stats WebSocket | PASS | live and portable smoke tests |
| WebRTC input | Reliable ordered DataChannel under simultaneous video | PASS | Every live scenario received/injected exact 240 Hz input with zero gaps, reordering, lifecycle errors, or buffered bytes |
| WebRTC 4K→720p | 4K source and 4K receiver viewport, Fast profile | PASS | 10 s: ~54 encoded/53 decoded fps; zero RTP loss, decoder drops, freezes, marker mismatches, or transport drops |
| WebRTC 1080p | 1080p source/receiver, Balanced profile | PASS | 10 s: ~32 encoded/33 decoded fps; all integrity/input/network checks pass |
| WebRTC 4K→1080p | 4K source and receiver viewport, Balanced profile | PASS | 10 s: ~29 encoded/29 decoded fps; all integrity/input/network checks pass |
| WebRTC 4K→1440p | 4K source and receiver viewport, Sharp profile | PASS | 10 s: ~14 encoded/15 decoded fps; all integrity/input/network checks pass; CPU-limited |
| Active soak | One continuous pressure/angle stroke plus 4K-source video | PASS | 600.005 s; 144,002/144,002 samples at 240.001 Hz; 30,508 decoded frames; 728 integrity checks; all gap/loss/freeze/backlog/error counters zero |
| RTP/media clock | Frame shedding does not accumulate playback lag | PASS | Soak media time advanced 599.997 s; zero media-time regressions; max presented-frame gap 81.1 ms in headless Edge |
| Portable runtime | Extract ZIP and run with no repo assets or Node runtime | PASS | Embedded HTML/JS served and video encoded from extracted EXE; direct dependency audit found Windows system DLLs only |
| WGC monitor capture | Real desktop produces capture/encoded frames | PASS | Earlier debug 3840×2160 source smoke on Windows build 22631 |
| Workspace | fmt, check all targets, Clippy `-D warnings`, all Rust tests | PASS | local Windows build and GitHub Actions |
| iPad Safari | Real Pencil pressure/tilt/coalescing/video | NOT RUN | Physical iPad unavailable |
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

The benchmark starts a real host, pairs through its LAN IPv4 URL, negotiates WebRTC, decodes H.264, sends synthetic coalesced Pointer Events, reads authenticated host metrics, and writes JSON under ignored `build/benchmarks/`.
