# Architecture

NFiDB is a single-PC, single-browser, trusted-LAN bridge. The Windows executable owns capture, capability discovery, codec-neutral encoding, signaling, WebRTC, native pointer injection, configuration, and the desktop UI. The iPad side is a small TypeScript application embedded in that executable.

```text
Windows monitor -> WGC newest-frame slot -> scale/YUV newest-frame slot -> H.264/HEVC/AV1 -> WebRTC -> Safari <video>
Windows PT_PEN <- native injector <- binary batches <- DataChannel <- Safari Pointer Events
Windows SendInput <- mouse/key/text/wheel messages <- DataChannel <- iPad trackpad/keyboard/IME
Windows commands <- semantic gesture messages <- DataChannel <- three-finger recognizer
                                      ^ WebSocket fallback/control/1 Hz diagnostics
iPad Files picker -> verified HTTP chunks -> private staging -> NFiDB Inbox on Windows
iPad Files/Safari <- ranged HTTP stream <- explicit NFiDB Outbox <- Windows file picker
```

## Workspace boundaries

- `crates/protocol`: fixed binary pointer packets and source-to-target mapping. It has no UI or transport knowledge.
- `crates/core`: versioned video configuration/presets, capability and Auto-score models, session credentials, metrics, and the abstract `InputSink`.
- `crates/host-windows`: monitor enumeration, Windows Graphics Capture, encoder abstraction, Media Foundation discovery/encoding, OpenH264 fallback, benchmarks, and `PT_PEN`/`PT_TOUCH` injection.
- `crates/transport`: embedded static assets, HTTP pairing/file transfer, WebSocket control, mDNS, and WebRTC signaling/media.
- `apps/windows-host`: CLI, egui shell, mode selection, QR display, and subsystem lifecycle.
- `apps/ipad-web`: Safari UI, Pointer Events sampling, coordinate mapping, diagnostics, and the WebRTC initiator.
- `tools/pointer-sink`: an independent `WM_POINTER` receiver and deterministic synthetic-pen self-test.

## Capture and backpressure

Windows Graphics Capture copies the newest BGRA frame into a one-element slot. A preprocessing worker scales and converts that frame to YUV into a second one-element slot, independently of the encoder. Replacing stale work at either boundary increments the dropped-frame counter. This deliberately bounds memory and latency: an overloaded stage discards stale pictures instead of building a queue.

The encoder worker owns a codec-neutral `VideoEncoder`. OpenH264 consumes I420; the Media Foundation path converts the same CPU YUV frame to NV12 and submits it to an asynchronously driven hardware MFT configured for low latency. Encoded frames carry their codec identity. The transport registers the matching H.264, H.265, or AV1 RTP codec/packetizer and refuses mismatched frames. RTP durations come from actual encoded-frame intervals so shedding cannot make media time fall behind wall time.

A newly connected peer starts at the broadcast tail, requests a keyframe, and discards delta frames until it receives one. Hardware keyframe requests rebuild only the encoder for a reliable fresh parameter-set/IDR boundary; software H.264 can force an intra frame directly. Codec changes close and recreate the WebRTC peer while authenticated WebSocket input stays available and held contacts are released. Width, FPS, bitrate, and preset changes update the running encoder worker without restarting capture, pairing, HTTP, or the application.

Windows enumerates hardware MFTs with `MFTEnumEx`, reads the associated DXGI adapter where exposed, initializes media types, and requires an actual encoded output sample before a mode becomes functional. Safari reports receiver codecs, SDP inclusion/negotiation is tracked separately, and a mode becomes playback-verified only after a browser presents its first frame. Auto combines those facts with locally cached benchmark gates and scores. Cache keys include NFiDB version, receiver runtime, encoder identity, profile, width, and FPS, so material changes cause a retest.

The present hardware path is CPU preprocessing: WGC copies BGRA to a bounded CPU frame, resize/BGRA-to-I420 runs on CPU, and I420 is interleaved to an NV12 memory buffer for the hardware MFT. It is accurately reported as `cpu-preprocessing`, not zero-copy. The capture crate exposes a D3D texture, but GPU resize/color conversion and D3D-surface encoder input remain a future optimization.

## Input path

The drawing engine listens to `pointerdown`, `pointermove`, `pointerup`, and `pointercancel`. It sends all actual coalesced samples in chronological order, never predicted points. A predicted point may be drawn locally as transient feedback. Pen, touch, and mouse have distinct device tags; only pen data contributes to pressure/tilt ranges. A contact stays on the transport selected at pointer-down; switching DataChannel/WebSocket during a stroke is forbidden. Interrupted contacts are cancelled and blocked until lifted rather than silently split across transports.

A separate remote-input engine forwards mouse wheel deltas, hardware-key transitions, committed Unicode/IME text, and bounded semantic commands. Printable text uses Windows Unicode injection instead of assuming a US keyboard layout. Shortcut keys use physical DOM codes so Control/Option/Shift chords retain key-down/key-up order. Three-finger recognition runs only when native touch forwarding is off and no Pencil is active. Blur, backgrounding, disconnect, peer replacement, credential rotation, and process shutdown release every held mouse button, key, pen, and touch contact.

The host validates packet version, message kind, exact/minimum length, enum values, finite numbers, pressure, tilt, UTF-8, field-size limits, and sample count before injection. Normalized points are mapped against the selected monitor's physical desktop rectangle, including negative virtual-desktop origins. Pen/touch use Windows synthetic-pointer APIs; mouse, wheel, keyboard, and text use `SendInput`; app-switch/Task View are atomic Windows chords and minimize targets the foreground window. The authenticated WebSocket accepts the same binary messages as the preferred reliable/ordered DataChannel.

## Session lifecycle

Each process run or credential rotation creates a UUID, random six-digit PIN, random 256-bit QR secret, and no access token. A correct PIN or QR secret issues a random 256-bit token, stored server-side only as SHA-256. The WebSocket receives it through an HttpOnly same-site cookie so access tokens do not appear in URLs or request logs. One `ActivePeer` replaces and closes any prior WebRTC peer. Manual reset and focused-window expiry rotation invalidate the old token and signal every active transport to close.

## File-transfer path

Bulk files deliberately do not use the reliable real-time DataChannel. The authenticated HTTP path keeps large transfers out of the input queue, supports browser-native download streaming/ranges, and bounds upload memory to one 1 MiB chunk per request. The file manager owns no general filesystem browser: Windows exposes only canonical regular files explicitly chosen into an in-memory Outbox, while iPad uploads can end only in a host-selected Inbox.

Safari creates an upload ticket containing a sanitized leaf name and declared size, then sends sequential chunks with an exact offset and a SHA-256 digest. The host verifies each digest before writing, rejects stale/out-of-order offsets with the authoritative resume point, and holds partial data in `%APPDATA%\NFiDB\transfer-staging`. Completion requires the declared byte count, hashes the complete staged file, selects a collision-free Inbox name, then atomically renames it. Cancel, disconnect, credential rotation, or the next process start removes owned `.part` files.

Downloads reopen and revalidate the queued file's size/modification timestamp, support one byte range, emit `Content-Disposition` and `Accept-Ranges`, and stream fixed 64 KiB buffers. A client cleanup request is attached to that stream's guard: only complete whole-file delivery removes the matching Outbox ID, while range and interrupted bodies remain retryable. Each file in a batch has an independent guard. Queue metadata contains no source path. A single checksum worker prevents a large selection from spawning unbounded hashing threads. Transfer rate/history state is bounded, and each streamed block checks that the paired session is still current.

The transfer pacer is independent of WebRTC. It uses a configurable throughput ceiling and, by default, waits while host metrics report any active pointer contact. This is application-level prioritization rather than network QoS, but prevents the file path from intentionally filling the real-time input channel or consuming unbounded RAM.

## Diagnostic recorder

Safari sends one authenticated structured diagnostic sample per second. The transport pairs it with a contemporaneous immutable host metrics snapshot and appends it to a six-hour bounded in-memory ring. The Windows Diagnostics page renders the latest raw evidence and aggregate percentile distributions; JSON export serializes configuration, capture/encoder identity, current host counters, limitations, all retained samples, and processed statistics. The recorder never writes unless the user selects export and never sends data off the LAN.

The HTTP bootstrap exists because iPad Safari must first reach an ordinary LAN address. NFiDB does not claim hostile-network confidentiality; use it only on a trusted private network. Details are in `SECURITY.md`.

## Failure containment

Capture failure leaves input transport available. WebRTC video failure falls back to authenticated WebSocket input. mDNS failure leaves the numeric IPv4 URL. Input is cancelled on disconnect rather than attempting to resume a possibly stale pointer lifecycle.
