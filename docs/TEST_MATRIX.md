# Test matrix

Last updated: 2026-08-17. Development host: Windows 11 Pro x64 build 22631, Rust 1.97.1 (`x86_64-pc-windows-msvc`), Node 22.22.3, npm 10.9.8, Microsoft Edge headless.

| Layer | Test | Result | Evidence |
| --- | --- | --- | --- |
| Protocol | Rust packet round trips, rejection, pressure/tilt clamps | PASS | 6 protocol tests |
| Mapping | fit/fill/1:1 and negative-origin target coordinates | PASS | Rust + 3 TypeScript geometry tests |
| Sessions/config | PIN, QR, invalid credential, disconnect, TOML round trip | PASS | 3 core tests |
| Native input | Inject synthetic `PT_PEN`; receive `WM_POINTER` pressure/tilt | PASS | 4 events; pressure `[102, 512, 1024, 0]`; X tilt `[0, 30, 0, 30]`; Y tilt `[0, 0, -30, -30]`; pen released |
| Browser unit | Binary golden values and mapping | PASS | Vitest: 5 tests |
| Browser build | strict TypeScript + Safari 16.4 Vite target | PASS | `npm run typecheck`, `npm run build` |
| HTTP/control | status, pairing, embedded assets, diagnostic route, stats WebSocket | PASS | local smoke run |
| WebRTC | Edge pairs, reaches connected, decodes H.264 video | PASS | Playwright live-host run: 5.6 s |
| Browser input | Pointer down/move/up reaches logging host | PASS | three ordered one-sample batches and disconnect reset |
| Workspace | fmt, check all targets, Clippy `-D warnings`, all Rust tests | PASS | local Windows build |
| WGC monitor capture | Real desktop produces capture/encoded frames | PASS | 3840×2160 source; debug smoke: 28 captured FPS, 3 encoded FPS, 257.2 ms software encode, 27 dropped |
| iPad Safari | Real Pencil pressure/tilt/coalescing/video | NOT RUN | Physical iPad unavailable |
| Krita | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Rebelle | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Photoshop | pressure/tilt/undo/reconnect | NOT RUN | Application/hardware unavailable |
| Stability | 30-minute active session | NOT RUN | Required before stable tag |
| WAN-offline | Continue after disconnecting internet | NOT RUN | Required before stable tag |

## Reproduce the deterministic tests

```powershell
.\scripts\test.ps1
cargo run -p pointer-sink -- --self-test
```

The Playwright live-host test is opt-in because it needs a running host and PIN:

```powershell
$env:NFIDB_E2E_URL = 'http://127.0.0.1:47831/'
$env:NFIDB_E2E_PIN = '123456'
Push-Location apps\ipad-web
npm run test:e2e
Pop-Location
```
