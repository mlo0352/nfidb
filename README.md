# NFiDB

**No Frills iPad Drawing Bridge** turns an iPad and Apple Pencil into a local-network pen display for Windows 11 or Apple-silicon macOS. Run the host app, open the address it shows in Safari, enter the six-digit PIN, and draw. There is no iPad app, account, subscription, cloud relay, or internet service involved in a session.

> NFiDB 0.8.0 is a solid alpha. Windows remains the most field-tested host. On the M1 Pro test Mac, Screen Recording and Accessibility approval, real-monitor VideoToolbox H.264 playback in iPad Safari, and remote mouse/keyboard control have passed. Pencil behavior in drawing apps, HEVC, files, reconnect/sleep, and the longer physical-device matrix remain documented follow-ups in [the test matrix](docs/TEST_MATRIX.md).

## What works

- Windows 11 capture through Windows Graphics Capture, or macOS capture through ScreenCaptureKit.
- Capability-aware H.264, HEVC, or AV1 video over peer-to-peer WebRTC, with Media Foundation hardware encoding and OpenH264 fallback.
- Benchmark-driven Auto selection, receiver playback verification, editable Fast/Balanced/Sharp presets, and live changes from Windows or iPad.
- Apple Pencil pressure, tilt, twist, buttons, and lifecycle samples from Safari Pointer Events.
- Native Windows `PT_PEN` injection, or macOS Quartz tablet events carrying pressure and tilt.
- iPad trackpad/mouse movement, buttons, and high-resolution horizontal/vertical scrolling.
- Hardware-keyboard shortcuts plus Unicode text entry from the hardware or iPad software keyboard.
- Three-finger app switching, Task View, and foreground-window minimize gestures while touch forwarding is off.
- Bidirectional, paired-session file transfer with an explicit Windows Outbox, confined Inbox, verified resumable uploads, ranged downloads, progress, cancellation, rate limiting, and transfer diagnostics.
- Fit, fill, and 1:1 coordinate mapping, including monitors with negative desktop coordinates.
- Six-digit PIN or QR pairing, focus-aware credential rotation, one active browser, and input reset on disconnect.
- mDNS-friendly and numeric-IP URLs, with automatic port fallback.
- Pen-display, input-only, and display-only modes.
- A standalone browser pointer diagnostic and a native `WM_POINTER` validation sink.
- Live Session, Source, Input, Files, Diagnostics, and App Setup pages in either desktop host.
- Authenticated raw and percentile-processed latency, bandwidth, frame-timing, input, encoder, and WebRTC diagnostics with local JSON export.

The alpha mirrors one monitor. It is not an extended-desktop display driver, remote-desktop product, or internet relay. Window-only capture, audio, multi-client sessions, Linux hosts, and an installer are not in this release.

## Run the portable build

1. Download the newest `NFiDB-windows-x64.zip` from [GitHub Releases](https://github.com/mlo0352/nfidb/releases).
2. On Windows 11 x64, extract it and run `nfidb.exe`. On an Apple-silicon Mac running macOS 13 or newer, extract `NFiDB-macos-arm64.zip`, move NFiDB to Applications, and open it.
3. Follow the host's **App Setup** page. NFiDB creates its configuration, safe defaults, Inbox, encoder selection, server, PIN, and QR automatically. On Windows, allow its one-time Firewall prompt on **Private networks only**. On macOS, use the live permission buttons for **Screen & System Audio Recording** and **Accessibility**; Apple requires the user to approve those switches.
4. Put the PC and iPad on the same normal LAN. Guest Wi-Fi/client isolation will prevent a connection.
5. Open the shown `.local` or numeric address in iPad Safari and enter the PIN (or scan the QR code).
6. Open the drawing app on the selected host monitor and draw on the iPad.

Nothing is installed on the iPad: the desktop host serves the complete browser client. Both desktop artifacts are currently unsigned alpha builds. Windows SmartScreen or macOS Gatekeeper may warn; use the GitHub release checksum and the normal **More info / Run anyway** or control-click **Open** flow. The Mac build is an application bundle with an ad-hoc local signature, not an Apple-notarized release.

On a first run, NFiDB opens **App Setup** automatically. That page remains in the sidebar and includes live permission state, direct settings links, a four-step session check, the pointer diagnostic, full help, and practical troubleshooting. If NFiDB does not appear in macOS Accessibility, click `+`, press `Command-Shift-G`, enter `~/Applications/NFiDB.app` (or `/Applications/NFiDB.app`), and add it. Permission state refreshes automatically; newly granted Screen Recording can start capture without relaunching when macOS allows it. Current Tahoe releases can ignore a Screen Recording request after that permission entry has been removed or reset. In that state, leave the same signed NFiDB app installed, restart the Mac once, reopen NFiDB, and use **Repair access** again before rebuilding or reinstalling it.

Touch is disabled by default. The paired iPad's **Touch** button changes the authoritative host input setting directly, and the desktop Input page stays synchronized with it. Windows forwards native touch contacts; macOS maps the first finger to its pointer. Use input-only mode when another screen-sharing system handles the picture, or display-only mode when you do not want remote input.

## iPad mouse and keyboard

Connect a trackpad/mouse and keyboard to the iPad, keep Safari in the foreground, and use the remote surface normally. Pointer motion is absolute to the displayed Windows monitor, mouse buttons and wheel gestures are forwarded, and Pencil remains a separate native pen device. With the keyboard panel closed, physical key-down/key-up events are sent to Windows so drawing-app hotkeys and held keys work. Open **Keyboard** on the iPad toolbar for layout-independent Unicode typing through the hardware or software keyboard, plus Esc/Tab/Backspace/Enter and Windows shortcut buttons.

On macOS the same browser inputs are posted through Quartz: Command stays Command, Option stays Option, app switching uses Command+Tab, the upward three-finger command opens Mission Control, and minimize uses Command+M. macOS does not expose a public general-purpose synthetic multitouch API, so **Touch on** maps the first finger to the Mac pointer; **Touch off / Gestures on** reserves fingers for NFiDB's semantic shortcuts. The Mac requires Accessibility permission before it accepts remote input.

The hardware mappings are **Option → Alt**, **Control → Ctrl**, **Shift → Shift**, **Return → Enter**, and an unmodified **Delete → Backspace**. Option+Tab therefore sends Alt+Tab. Control+Option+Delete sends the Ctrl+Alt+Delete key sequence, but normal unsigned applications cannot invoke the protected Windows secure-attention screen; Windows discards synthetic Ctrl+Alt+Delete there.

With **Touch off** and **Gestures on**, swipe three fingers right/left to move to the next/previous Windows app, up for Task View, and down to minimize the foreground window. Pencil contact temporarily suppresses finger shortcuts. Turning Touch on updates Windows and sends fingers as native Windows touch instead. The controls open only from the small bottom-left **NFi Controls** button, so drawing across Windows menu bars does not summon the toolbar. iPadOS can consume system-reserved shortcuts before Safari receives them, so the on-screen shortcut buttons remain the deterministic fallback.

If a PIN/QR has expired or Safari shows a stale-session error, use **Session → Reset PIN + QR**. This immediately invalidates the old browser session, releases injected contacts, closes its peer connection, and redraws both credentials. An expired unpaired code also rotates automatically while the desktop app is focused.

## File transfer

Open **Files** on the desktop host and choose **Add files for iPad** to place one or more explicit files in the temporary Outbox. The paired iPad shows an in-page notice and queue-count badge even when its Files panel is closed. Open that panel to download one item or the full batch through Safari’s download manager. **Clear each queue item after download** is on by default, persists across page loads, and asks the host to remove each item only after its complete stream is delivered; interrupted and partial-range downloads remain queued. Manual removal affects only the NFiDB queue entry, never the original file.

To send files to the host, open the iPad Files panel, choose one or more files, and leave Safari in the foreground until the queue completes. Incoming files are written through a private staging directory and moved into the host's `Downloads/NFiDB Inbox` only after every 1 MiB chunk and the completed file have been SHA-256 processed. Duplicate names become `name (1).ext` rather than overwriting an existing file. Use **Open received files folder** on the desktop Files page to open the configured Inbox directly.

Transfers are capped at 10 GiB per file and 32 Mbps by default. The Windows Files page can change those limits or disable transfer. With **Pause bulk traffic while Pencil or touch is down** enabled, a long transfer yields whenever a drawing contact is active. Pairing reset/disconnect invalidates transfer access and removes unfinished uploads.

## Drawing-app setup

- On Windows, select the app's Windows Ink/Pointer input path when it offers a choice. On macOS, use the app's normal tablet/pressure path.
- Start with touch disabled to avoid accidental canvas gestures.
- If a program ignores pressure, first run `pointer-sink.exe --self-test`. A passing sink proves the Windows pen path independently of the art app.
- Krita, Rebelle, and Photoshop are explicit validation targets, but no physical-app result is claimed yet.

## Build from source

Windows prerequisites are Windows 11 x64, Rust stable with the MSVC target, Visual Studio 2022 Build Tools with “Desktop development with C++,” and Node.js 22+. macOS prerequisites are an Apple-silicon Mac, macOS 13+, full Xcode/Command Line Tools, Rust stable, and Node.js 22+.

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
.\scripts\gui-smoke.ps1
```

On macOS:

```bash
npm --prefix apps/ipad-web ci
./scripts/build-macos.sh
```

This creates `build/packages/NFiDB-macos-arm64.zip` and its SHA-256 file.

Mac developers with a signing identity should use a stable certificate so Screen Recording and Accessibility approvals survive rebuilds:

```bash
NFIDB_CODESIGN_IDENTITY="Apple Development: Your Name (TEAMID)" ./scripts/build-macos.sh
```

Public releases should use a Developer ID Application identity and Apple notarization. Without one, the script deliberately labels its signature ad-hoc and a changed build may require macOS permission approval again.

For a single long-running handoff that another Codex turn can verify without spending the turn waiting on compilation, run:

```powershell
.\scripts\validate-for-codex.ps1
```

It writes `build\user-validation\latest.json` plus a full transcript. After it finishes, tell Codex **resume**; the report identifies the exact commit/tree and every validation stage.

If compilation completed but a later packaging or smoke step failed, Codex can resume from the existing release binaries with `-ResumeAfterBuild`. That path skips npm, Cargo, and the source suite instead of burning another full compile.

If source validation is already complete but the optimized release binary has not been built, `-ResumeAfterSourceValidation` skips the repeated source suite and frontend build while still compiling and testing the final release artifact.

The browser client is built into the desktop executable; GitHub Pages is the public download/documentation site, not the drawing client. The actual iPad page is served directly by the paired host on the trusted LAN.

## Diagnostics and CLI

```text
nfidb --input-only
nfidb --display-only
nfidb --diagnostics
nfidb --capture test-pattern --input-sink log
pointer-sink --self-test
pointer-sink --stress-test --samples 14400 --rate 240 --batch-size 4
./scripts/benchmark.ps1 -Quick
./scripts/benchmark.ps1 -Full -HostOnly
./scripts/file-transfer-smoke.ps1
```

Run `nfidb --diagnostics` to open the diagnostic page directly, or select **DIAGNOSTICS** in the left sidebar. For a real-iPad test, reset the recording, connect in Safari, enable **Stats**, draw and move desktop content for at least 60 seconds, then export the detailed JSON. The report contains the raw one-second samples and processed count/min/mean/p50/p95/p99/max distributions; it is written under `%APPDATA%\NFiDB\diagnostics\` and never uploaded.

The live iPad panel separates network RTT/bandwidth, decoder and presentation rate, RTP loss, jitter-buffer/decode cost, frame gaps, startup-to-first-frame, estimated pipeline delay, input continuity, pressure/tilt, mouse/wheel/key/text/gesture counters, and browser/host buffering. It also shows the transport-selected ICE path (iPad and host addresses/ports, candidate type, protocol, network type, and VPN flag when Safari exposes them), per-interval packet loss, and NACK/PLI/FIR recovery activity. Safari supplies exact capture-to-presentation timing only when its WebRTC frame metadata exposes it; otherwise NFiDB labels and uses a component estimate. Exact Pencil-contact-to-host-photon latency still requires a synchronized high-speed camera.

The host stores user settings in `%APPDATA%\NFiDB\config.toml` on Windows or `~/Library/Application Support/NFiDB/config.toml` on macOS. Full flags are available with `nfidb --help`; release builds normally launch as a GUI application. The measured release-mode resolution and frame-rate envelope is published in [Performance notes](docs/PERFORMANCE.md).

For hardware modes, real monitor frames use the capture GPU for resize and BGRA-to-NV12 conversion, then feed a DXGI texture directly to a D3D11-aware Media Foundation encoder. The Source and Diagnostics pages show `GPU zero-copy` when no full frame enters CPU memory, `GPU assisted` when a vendor encoder requires compact NV12 readback, and `CPU preprocess` for OpenH264 or a compatibility fallback.

On macOS, ScreenCaptureKit produces a bounded IOSurface-backed pixel buffer at the requested output size and VideoToolbox consumes that surface directly. NFiDB verifies the active compression session actually reports hardware acceleration before labeling it hardware. The tested M1 Pro exposes hardware H.264 and HEVC; it exposes no AV1 encoder, so AV1 remains visibly unavailable and Auto will not select it.

If the Windows GUI cannot initialize, NFiDB displays the startup error and saves the same details to `%APPDATA%\NFiDB\startup-error.log` for diagnosis.

## Documentation

- [Changelog](CHANGELOG.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Pointer and signaling protocol](docs/PROTOCOL.md)
- [Security model](docs/SECURITY.md)
- [Performance notes](docs/PERFORMANCE.md)
- [Codec benchmarks](docs/CODEC_BENCHMARKS.md)
- [Known issues](docs/KNOWN_ISSUES.md)
- [Test matrix](docs/TEST_MATRIX.md)
- [Release process](docs/RELEASE.md)
- [Engineering decisions and evidence](docs/DEVLOG.md)

## License

NFiDB is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Direct dependency licenses are summarized in [THIRD_PARTY.md](THIRD_PARTY.md).
