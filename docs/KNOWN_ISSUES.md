# Known issues

This file is intentionally strict: “implemented” and “verified on available hardware” are different claims.

## MVP limitations

- The current encoder is isolated software OpenH264. Media Foundation hardware H.264—the release performance objective—is not implemented yet. Debug builds do not sustain 720p60 on the development machine.
- No physical iPad/Apple Pencil was available for this build. iPad Safari pressure, tilt, coalesced-event behavior, rotation, background/resume, and reconnect still require field validation.
- Krita, Rebelle, and Photoshop compatibility has not yet been physically tested. The independent native sink proves Windows receives injected pressure and tilt, not that every application configures its brushes correctly.
- Only whole-monitor capture is available. Window capture is planned.
- Monitor mirroring is not a true Windows extended desktop. A virtual display driver is outside the MVP.
- Touch injection exists but is off by default. Two-finger canvas gestures need per-application validation before becoming a default.
- The portable package is unsigned, so SmartScreen may warn. No installer is shipped.
- The first pairing/signaling exchange is local HTTP. Do not use NFiDB on hostile or public networks.
- mDNS `.local` discovery depends on the router and iPad network. The numeric IPv4 URL is the fallback.
- Some VPN and virtual adapters may add unusable WebRTC ICE candidates. Local ICE can still select the working LAN candidate, but logs may contain harmless bind warnings.
- Changing the capture quality profile in the desktop UI is saved for the next launch; it does not rebuild the encoder mid-session.
- There is no audio, clipboard, keyboard forwarding, file transfer, cloud relay, multi-client support, or Linux/macOS host.

## Validation still required for a public stable release

- Clean-machine install/run and Private-firewall prompt.
- Thirty-minute stability and repeated reconnect tests.
- Physical iPadOS/Safari test across at least two iPad generations.
- Real Krita, Rebelle, and Photoshop pressure/tilt matrix.
- Release-mode capture/encode/latency measurements on Intel, AMD, and NVIDIA systems.
- Internet-disconnected LAN session.
