# NFiDB

**No Frills iPad Drawing Bridge** turns an iPad and Apple Pencil into a local-network pen display for a Windows 11 PC. Run one portable Windows app, open the address it shows in Safari, enter the six-digit PIN, and draw. There is no iPad app, account, subscription, cloud relay, or internet dependency after installation.

> NFiDB is an early MVP. Its protocol, native pen injection, Windows capture, browser pairing, and WebRTC video path have automated coverage. Physical iPad/Apple Pencil and individual art-app compatibility still need field validation; see [the test matrix](docs/TEST_MATRIX.md) before relying on it for production work.

## What works

- Windows 11 monitor capture through Windows Graphics Capture.
- Local H.264 video over a peer-to-peer WebRTC connection.
- Apple Pencil pressure, tilt, twist, buttons, and lifecycle samples from Safari Pointer Events.
- Native Windows `PT_PEN` injection; optional `PT_TOUCH` injection is off by default.
- Fit, fill, and 1:1 coordinate mapping, including monitors with negative desktop coordinates.
- Six-digit PIN or QR pairing, random per-run credentials, one active browser, and input reset on disconnect.
- mDNS-friendly and numeric-IP URLs, with automatic port fallback.
- Pen-display, input-only, and display-only modes.
- A standalone browser pointer diagnostic and a native `WM_POINTER` validation sink.

The first MVP mirrors one Windows monitor. It is not an extended-desktop display driver, remote-desktop product, or internet relay. Window-only capture, hardware Media Foundation encoding, audio, multi-client sessions, and an installer are not in this release.

## Run the portable build

1. Download the newest `NFiDB-windows-x64.zip` from [GitHub Releases](https://github.com/mlo0352/nfidb/releases).
2. Extract it and run `nfidb.exe` on a Windows 11 PC.
3. If Windows Firewall asks, allow NFiDB on **Private networks only**.
4. Put the PC and iPad on the same normal LAN. Guest Wi-Fi/client isolation will prevent a connection.
5. Open the shown `.local` or numeric address in iPad Safari and enter the PIN (or scan the QR code).
6. Open the drawing app on the selected Windows monitor and draw on the iPad.

Touch is deliberately disabled until enabled in both the host and browser. Use input-only mode when another screen-sharing system handles the picture, or display-only mode when you do not want remote input.

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
```

The host stores user settings in `%APPDATA%\NFiDB\config.toml`. Full flags are available with `nfidb --help` in a debug/console build; release builds normally launch as a GUI application.

## Documentation

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
