# Architecture

NFiDB is a single-PC, single-browser, trusted-LAN bridge. The Windows executable owns capture, H.264 encoding, signaling, WebRTC, native pointer injection, configuration, and the desktop UI. The iPad side is a small TypeScript application embedded in that executable.

```text
Windows monitor -> WGC newest-frame slot -> scale -> H.264 -> WebRTC video -> Safari <video>
Windows PT_PEN <- native injector <- binary batches <- DataChannel <- Safari Pointer Events
                                      ^ WebSocket fallback/control/stats
```

## Workspace boundaries

- `crates/protocol`: fixed binary pointer packets and source-to-target mapping. It has no UI or transport knowledge.
- `crates/core`: configuration, session credentials, metrics, and the abstract `InputSink`.
- `crates/host-windows`: monitor enumeration, Windows Graphics Capture, H.264 encoding, and `PT_PEN`/`PT_TOUCH` injection.
- `crates/transport`: embedded static assets, HTTP pairing, WebSocket control, mDNS, and WebRTC signaling/media.
- `apps/windows-host`: CLI, egui shell, mode selection, QR display, and subsystem lifecycle.
- `apps/ipad-web`: Safari UI, Pointer Events sampling, coordinate mapping, diagnostics, and the WebRTC initiator.
- `tools/pointer-sink`: an independent `WM_POINTER` receiver and deterministic synthetic-pen self-test.

## Capture and backpressure

Windows Graphics Capture copies the newest BGRA frame into a one-element slot. Replacing an unencoded frame increments the dropped-frame counter. This deliberately bounds memory and latency: an overloaded encoder discards stale pictures instead of building a queue.

The encoder scales down only when the selected profile requires it, converts BGRA to YUV, and emits an Annex-B H.264 access unit. Keyframes are requested once per target frame-rate interval. The current portable MVP uses the isolated OpenH264 software implementation; the intended replacement boundary is the encoder inside `host-windows`, leaving capture and WebRTC unchanged.

## Input path

The browser listens only to `pointerdown`, `pointermove`, `pointerup`, and `pointercancel`. It sends all actual coalesced samples, never predicted points. A predicted point may be drawn locally as transient feedback. Packets are sent over the WebRTC DataChannel when open and the authenticated WebSocket otherwise.

The host validates packet version, exact length, enum values, finite numbers, pressure, tilt, and sample count before injection. Normalized points are mapped against the selected monitor's physical desktop rectangle. The injector maintains pointer lifecycle state and releases every active pen/touch contact on channel close, peer failure, and application shutdown.

## Session lifecycle

Each process run creates a UUID, random six-digit PIN, random 256-bit QR secret, and no access token. A correct PIN or QR secret issues a random 256-bit token, stored server-side only as SHA-256. The WebSocket receives it through an HttpOnly same-site cookie so access tokens do not appear in URLs or request logs. One `ActivePeer` replaces and closes any prior WebRTC peer.

The HTTP bootstrap exists because iPad Safari must first reach an ordinary LAN address. NFiDB does not claim hostile-network confidentiality; use it only on a trusted private network. Details are in `SECURITY.md`.

## Failure containment

Capture failure leaves input transport available. WebRTC video failure falls back to authenticated WebSocket input. mDNS failure leaves the numeric IPv4 URL. Input is cancelled on disconnect rather than attempting to resume a possibly stale pointer lifecycle.
