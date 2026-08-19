# Known issues

This file is intentionally strict: “implemented” and “verified on available hardware” are different claims.

## MVP limitations

- The current encoder is isolated software OpenH264. Media Foundation hardware H.264 is not implemented. The measured release build sustains roughly 51–54 fps at 720p Fast, 29–32 fps at 1080p Balanced, and 14–16 fps at 1440p Sharp on the development PC; other CPUs and real screen content will differ.
- Physical iPad Safari pairing, touch transport, and video have begun field validation. Apple Pencil pressure/tilt/coalescing, rotation, background/resume, and repeated reconnect still require complete field results.
- Krita, Rebelle, and Photoshop compatibility has not yet been physically tested. The independent native sink proves Windows receives injected pressure and tilt, not that every application configures its brushes correctly.
- Only whole-monitor capture is available. Window capture is planned.
- Monitor mirroring is not a true Windows extended desktop. A virtual display driver is outside the MVP.
- Touch injection exists but is off by default. Two-finger canvas gestures need per-application validation before becoming a default.
- The portable package is unsigned, so SmartScreen may warn. No installer is shipped.
- A package extracted and run outside the repository serves its embedded client and encodes video with no non-system DLL dependency, but a separate clean Windows machine has not been tested yet.
- Edge headless with a 4K viewport can present fewer frames than it successfully decodes. Receiver diagnostics distinguish browser presentation drops from RTP loss and decoder drops; physical iPad presentation remains to be measured.
- The first pairing/signaling exchange is local HTTP. Do not use NFiDB on hostile or public networks.
- mDNS `.local` discovery depends on the router and iPad network. The numeric IPv4 URL is the fallback.
- Some VPN and virtual adapters may add unusable WebRTC ICE candidates. Local ICE can still select the working LAN candidate, but logs may contain harmless bind warnings.
- Changing the capture quality profile in the desktop UI is saved for the next launch; it does not rebuild the encoder mid-session.
- Safari may omit WebRTC `captureTime`/`receiveTime` frame metadata. In that case the live/report pipeline value is explicitly a component estimate; exact Pencil-to-photon latency requires a synchronized high-speed camera.
- Diagnostic recording is sampled once per second and retains approximately six hours in memory. It is intended for distributions and trend diagnosis, not packet-level capture.
- iPadOS/Safari can reserve OS-level keyboard shortcuts before the page receives them. NFiDB forwards every supported event it receives and provides on-screen Alt+Tab, Alt+Shift+Tab, Task View, minimize, and special-key fallbacks.
- NFiDB transmits Control+Option+Delete as Ctrl+Alt+Delete, but Windows intentionally rejects synthetic input at the secure-attention screen. A portable, unsigned user-mode app cannot safely bypass that boundary.
- Physical iPad trackpad, hardware-keyboard layout/shortcut coverage, software-keyboard IME, and three-finger gesture behavior still need field validation across iPadOS versions.
- The file-transfer protocol, large-file streaming, chunk verification, cancellation, ranges, multi-file queueing, arrival notice, and completed-stream auto-clear are automated, but the iPad Files/Photos picker, Safari download manager, background/sleep behavior, and very large physical-device transfers still need real-iPad validation. Safari must remain in the foreground for reliable uploads; native downloads use Safari's own progress UI.
- File transfer handles files, not folders, clipboard contents, or automatic synchronization. The Windows Outbox is intentionally process-local and exposes only explicitly selected regular files.
- There is no audio, clipboard synchronization, cloud relay, multi-client support, or Linux/macOS host.

## Validation still required for a public stable release

- Clean-machine install/run and Private-firewall prompt.
- Thirty-minute stability and repeated reconnect tests (the automated 10-minute active soak passes).
- Physical iPadOS/Safari test across at least two iPad generations.
- Real Krita, Rebelle, and Photoshop pressure/tilt matrix.
- Release-mode capture/encode/latency measurements on Intel, AMD, and NVIDIA systems.
- Internet-disconnected LAN session.
