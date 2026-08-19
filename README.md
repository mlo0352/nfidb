# NFiDB

**No Frills iPad Drawing Bridge** turns an iPad and Apple Pencil into a local-network pen display for a Windows 11 PC. Extract one portable Windows app, open the address it shows in Safari, enter the six-digit PIN, and draw. There is no iPad app, account, subscription, cloud relay, or internet service involved in a session.

> NFiDB 0.5.1 is a solid alpha. Its protocol, native input, Windows capture, browser pairing, remote-control, WebRTC, and bidirectional file-transfer paths have automated coverage, and the iPad mouse, keyboard, and three-finger controls have passed a physical-device check. Apple Pencil behavior in individual art apps and the Files panel still need physical field validation; see [the test matrix](docs/TEST_MATRIX.md) before relying on it for production work.

## What works

- Windows 11 monitor capture through Windows Graphics Capture.
- Local H.264 video over a peer-to-peer WebRTC connection.
- Apple Pencil pressure, tilt, twist, buttons, and lifecycle samples from Safari Pointer Events.
- Native Windows `PT_PEN` injection; optional `PT_TOUCH` injection is off by default.
- iPad trackpad/mouse movement, buttons, and high-resolution horizontal/vertical scrolling.
- Hardware-keyboard shortcuts plus Unicode text entry from the hardware or iPad software keyboard.
- Three-finger app switching, Task View, and foreground-window minimize gestures while touch forwarding is off.
- Bidirectional, paired-session file transfer with an explicit Windows Outbox, confined Inbox, verified resumable uploads, ranged downloads, progress, cancellation, rate limiting, and transfer diagnostics.
- Fit, fill, and 1:1 coordinate mapping, including monitors with negative desktop coordinates.
- Six-digit PIN or QR pairing, focus-aware credential rotation, one active browser, and input reset on disconnect.
- mDNS-friendly and numeric-IP URLs, with automatic port fallback.
- Pen-display, input-only, and display-only modes.
- A standalone browser pointer diagnostic and a native `WM_POINTER` validation sink.
- Live Session, Source, Input, Files, Diagnostics, and App Setup pages in the Windows host.
- Authenticated raw and percentile-processed latency, bandwidth, frame-timing, input, encoder, and WebRTC diagnostics with local JSON export.

The first MVP mirrors one Windows monitor. It is not an extended-desktop display driver, remote-desktop product, or internet relay. Window-only capture, hardware Media Foundation encoding, audio, multi-client sessions, and an installer are not in this release.

## Run the portable build

1. Download the newest `NFiDB-windows-x64.zip` from [GitHub Releases](https://github.com/mlo0352/nfidb/releases).
2. Extract it and run `nfidb.exe` on a Windows 11 PC.
3. If Windows Firewall asks, allow NFiDB on **Private networks only**.
4. Put the PC and iPad on the same normal LAN. Guest Wi-Fi/client isolation will prevent a connection.
5. Open the shown `.local` or numeric address in iPad Safari and enter the PIN (or scan the QR code).
6. Open the drawing app on the selected Windows monitor and draw on the iPad.

Nothing is installed on the iPad: the Windows host serves the complete browser client from the EXE. The Windows ZIP is portable and has only standard Windows DLL dependencies in the tested build. It is unsigned, so SmartScreen can warn, and Windows Firewall may ask once for Private-network access.

Touch is deliberately disabled until enabled in both the host and browser. Use input-only mode when another screen-sharing system handles the picture, or display-only mode when you do not want remote input.

## iPad mouse and keyboard

Connect a trackpad/mouse and keyboard to the iPad, keep Safari in the foreground, and use the remote surface normally. Pointer motion is absolute to the displayed Windows monitor, mouse buttons and wheel gestures are forwarded, and Pencil remains a separate native pen device. With the keyboard panel closed, physical key-down/key-up events are sent to Windows so drawing-app hotkeys and held keys work. Open **Keyboard** on the iPad toolbar for layout-independent Unicode typing through the hardware or software keyboard, plus Esc/Tab/Backspace/Enter and Windows shortcut buttons.

The hardware mappings are **Option → Alt**, **Control → Ctrl**, **Shift → Shift**, **Return → Enter**, and an unmodified **Delete → Backspace**. Option+Tab therefore sends Alt+Tab. Control+Option+Delete sends the Ctrl+Alt+Delete key sequence, but normal unsigned applications cannot invoke the protected Windows secure-attention screen; Windows discards synthetic Ctrl+Alt+Delete there.

With browser **Touch off** and **Gestures on**, swipe three fingers right/left to move to the next/previous Windows app, up for Task View, and down to minimize the foreground window. Pencil contact temporarily suppresses finger shortcuts. Turning Touch on sends fingers as native Windows touch instead. iPadOS can consume system-reserved shortcuts before Safari receives them, so the on-screen shortcut buttons remain the deterministic fallback.

If a PIN/QR has expired or Safari shows a stale-session error, use **Session → Reset PIN + QR**. This immediately invalidates the old browser session, releases injected contacts, closes its peer connection, and redraws both credentials. An expired unpaired code also rotates automatically while the desktop app is focused.

## File transfer

Open **Files** on Windows and choose **Add files for iPad** to place one or more explicit files in the temporary Outbox. The paired iPad shows an in-page notice and queue-count badge even when its Files panel is closed. Open that panel to download one item or the full batch through Safari’s download manager. **Clear each queue item after download** is on by default, persists across page loads, and asks the host to remove each item only after its complete stream is delivered; interrupted and partial-range downloads remain queued. Manual removal affects only the NFiDB queue entry, never the original Windows file.

To send files to Windows, open the iPad Files panel, choose one or more files, and leave Safari in the foreground until the queue completes. Incoming files are written through a private staging directory and moved into `Downloads\NFiDB Inbox` only after every 1 MiB chunk and the completed file have been SHA-256 processed. Duplicate names become `name (1).ext` rather than overwriting an existing file. Use **Open received files folder** on the Windows Files page to open the configured Inbox directly.

Transfers are capped at 10 GiB per file and 32 Mbps by default. The Windows Files page can change those limits or disable transfer. With **Pause bulk traffic while Pencil or touch is down** enabled, a long transfer yields whenever a drawing contact is active. Pairing reset/disconnect invalidates transfer access and removes unfinished uploads.

## Drawing-app setup

- Select the app's Windows Ink/Pointer input path when it offers a choice.
- Start with touch disabled to avoid accidental canvas gestures.
- If a program ignores pressure, first run `pointer-sink.exe --self-test`. A passing sink proves the Windows pen path independently of the art app.
- Krita, Rebelle, and Photoshop are explicit validation targets, but no physical-app result is claimed yet.

## Build from source

Prerequisites are Windows 11 x64, Rust stable with the MSVC target, Visual Studio 2022 Build Tools with “Desktop development with C++,” and Node.js 22+.

```powershell
git clone https://github.com/mlo0352/nfidb.git
cd nfidb
.\scripts\dev.ps1
```

Run all static checks and tests with:

```powershell
.\scripts\test.ps1
```

Create the portable ZIP and SHA-256 file with:

```powershell
.\scripts\build-release.ps1
```

The browser client is built into `nfidb.exe`; GitHub Pages is the public download/documentation site, not the drawing client. The actual iPad page is served directly by the Windows host on the trusted LAN.

## Diagnostics and CLI

```text
nfidb --input-only
nfidb --display-only
nfidb --diagnostics
nfidb --capture test-pattern --input-sink log
pointer-sink --self-test
pointer-sink --stress-test --samples 14400 --rate 240 --batch-size 4
./scripts/benchmark.ps1 -DurationSeconds 10
./scripts/file-transfer-smoke.ps1
```

Run `nfidb --diagnostics` to open the diagnostic page directly, or select **DIAGNOSTICS** in the left sidebar. For a real-iPad test, reset the recording, connect in Safari, enable **Stats**, draw and move desktop content for at least 60 seconds, then export the detailed JSON. The report contains the raw one-second samples and processed count/min/mean/p50/p95/p99/max distributions; it is written under `%APPDATA%\NFiDB\diagnostics\` and never uploaded.

The live iPad panel separates network RTT/bandwidth, decoder and presentation rate, RTP loss, jitter-buffer/decode cost, frame gaps, startup-to-first-frame, estimated pipeline delay, input continuity, pressure/tilt, mouse/wheel/key/text/gesture counters, and browser/host buffering. Safari supplies exact capture-to-presentation timing only when its WebRTC frame metadata exposes it; otherwise NFiDB labels and uses a component estimate. Exact Pencil-contact-to-Windows-photon latency still requires a synchronized high-speed camera.

The host stores user settings in `%APPDATA%\NFiDB\config.toml`. Full flags are available with `nfidb --help`; release builds normally launch as a GUI application. The measured release-mode resolution and frame-rate envelope is published in [Performance notes](docs/PERFORMANCE.md).

## Documentation

- [Changelog](CHANGELOG.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Pointer and signaling protocol](docs/PROTOCOL.md)
- [Security model](docs/SECURITY.md)
- [Performance notes](docs/PERFORMANCE.md)
- [Known issues](docs/KNOWN_ISSUES.md)
- [Test matrix](docs/TEST_MATRIX.md)
- [Release process](docs/RELEASE.md)
- [Engineering decisions and evidence](docs/DEVLOG.md)

## License

NFiDB is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Direct dependency licenses are summarized in [THIRD_PARTY.md](THIRD_PARTY.md).
