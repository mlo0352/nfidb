# Architecture

NFiDB is a single-host, single-browser, trusted-LAN bridge. The Windows or macOS executable owns capture, capability discovery, codec-neutral encoding, signaling, WebRTC, native input injection, configuration, and the desktop UI. The iPad side is a small TypeScript application embedded in that executable.

The Windows desktop shell is rendered by eframe through `wgpu`, allowing the native Windows graphics backend or its software adapter to be used without requiring a vendor OpenGL installation. This renderer is separate from monitor capture and video encoding.

```text
Windows monitor -> WGC newest-frame slot -> scale/YUV newest-frame slot -> H.264/HEVC/AV1 -> WebRTC -> Safari <video>
macOS monitor -> ScreenCaptureKit IOSurface newest-frame slot -> VideoToolbox H.264/HEVC -> WebRTC -> Safari <video>
Windows PT_PEN <- native injector <- binary batches <- DataChannel <- Safari Pointer Events
macOS Quartz tablet/mouse events <- binary batches <- DataChannel <- Safari Pointer Events
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
- `crates/host-macos`: CoreGraphics/ScreenCaptureKit display enumeration and capture, IOSurface/VideoToolbox encoding, OpenH264 fallback, host benchmarks, and Quartz input injection.
- `crates/transport`: embedded static assets, HTTP pairing/file transfer, WebSocket control, mDNS, and WebRTC signaling/media.
- `apps/windows-host`: CLI, egui shell, mode selection, QR display, and subsystem lifecycle.
- `apps/ipad-web`: Safari UI, Pointer Events sampling, coordinate mapping, diagnostics, and the WebRTC initiator.
- `tools/pointer-sink`: an independent `WM_POINTER` receiver and deterministic synthetic-pen self-test.

## Capture and backpressure

Windows Graphics Capture copies the newest monitor frame into an owned D3D11 texture from a four-surface pool, then puts that reference in a one-element slot. A preprocessing worker uses the D3D11 video processor to scale and convert BGRA to NV12 into another four-surface pool and one-element slot. Pool exhaustion or stale replacement drops that picture and increments the dropped-frame counter. No stage can allocate or queue without a bound.

The encoder worker owns a codec-neutral `VideoEncoder`. A D3D11-aware Media Foundation MFT receives the NV12 texture through `MFCreateDXGISurfaceBuffer` and an `IMFDXGIDeviceManager`; diagnostics call this `gpu-zero-copy`, meaning no full-frame CPU transfer (there is one bounded GPU-local copy out of WGC's reusable frame pool). If a vendor MFT rejects direct surfaces, NFiDB reads back the already resized NV12 texture and submits a memory buffer as `gpu-assisted`. OpenH264 and generated CPU test patterns use BGRA resize, I420 conversion, and the `cpu-preprocessing` path. Encoded frames carry their codec identity. The transport registers the matching H.264, H.265, or AV1 RTP codec/packetizer and refuses mismatched frames. RTP durations come from actual encoded-frame intervals so shedding cannot make media time fall behind wall time.

A newly connected peer starts at the broadcast tail, requests a keyframe, and discards delta frames until it receives one. Hardware keyframe requests rebuild only the encoder for a reliable fresh parameter-set/IDR boundary; software H.264 can force an intra frame directly. Codec changes close and recreate the WebRTC peer while authenticated WebSocket input stays available and held contacts are released. Width, FPS, bitrate, and preset changes update the running encoder worker without restarting capture, pairing, HTTP, or the application.

Windows enumerates hardware MFTs with `MFTEnumEx`, reads the associated DXGI adapter where exposed, initializes media types, and requires an actual encoded output sample before a mode becomes functional. Safari reports receiver codecs, SDP inclusion/negotiation is tracked separately, and a mode becomes playback-verified only after a browser presents its first frame. Auto combines those facts with locally cached benchmark gates and scores. Cache keys include NFiDB version, receiver runtime, encoder identity, profile, width, and FPS, so material changes cause a retest.

Media Foundation discovery runs on a named MTA worker and returns data-only capabilities to the GUI. COM apartment state is initialized and balanced per encoder/discovery worker, while `MFStartup` remains process-wide. The egui/winit main thread is never claimed by encoder discovery, and the unused Windows file-drop hook is disabled as a second boundary against OLE apartment conflicts.

GPU processing is tied to the WGC device so multi-adapter systems do not guess which GPU owns the captured surface. D3D11 multithread protection covers the capture, preprocess, and encoder workers. Direct-input refusal is remembered for that adapter/encoder instance so fallback happens once rather than introducing a repeated per-frame stall. A codec or quality change still reconstructs only the active video processor/encoder state and preserves the authenticated session.

### macOS capture and encoding

ScreenCaptureKit is configured at the active preset's even output dimensions and FPS, with a queue depth of two. Its IOSurface-backed `CVPixelBuffer` enters the same one-frame replacement slot used by the encoder worker; a new callback replaces a stale frame instead of extending latency. VideoToolbox consumes the IOSurface directly. Generated patterns and OpenH264 remain explicit CPU paths.

VideoToolbox candidates come from `VTCopyVideoEncoderList`. A session is configured for real-time operation, no frame reordering, a bounded frame-delay count, the requested bitrate/FPS/profile, and speed priority where the encoder accepts it. NFiDB then reads `kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder`; a session is rejected from hardware modes if VideoToolbox reports software or withholds the result. H.264 AVCC and HEVC HVCC output is converted to Annex B and its VPS/SPS/PPS headers are prepended to fresh keyframes for the existing RTP packetizers.

Screen capture permission is not required merely to open the app. If ScreenCaptureKit content enumeration is denied, CoreGraphics supplies display geometry so input-only and generated diagnostic modes remain available while the Source/App Setup pages explain Screen Recording approval. Accessibility is separately required before Quartz may post remote input.

The first-run shell persists default configuration immediately, opens App Setup when a required Mac permission is missing, probes Screen Recording and Accessibility every repaint, and provides constant deep links to the matching System Settings panes. OS consent remains user-controlled; the app never modifies the TCC permission database.

## Input path

The drawing engine listens to `pointerdown`, `pointermove`, `pointerup`, and `pointercancel`. It sends all actual coalesced samples in chronological order, never predicted points. A predicted point may be drawn locally as transient feedback. Pen, touch, and mouse have distinct device tags; only pen data contributes to pressure/tilt ranges. A contact stays on the transport selected at pointer-down; switching DataChannel/WebSocket during a stroke is forbidden. Interrupted contacts are cancelled and blocked until lifted rather than silently split across transports.

A separate remote-input engine forwards mouse wheel deltas, hardware-key transitions, committed Unicode/IME text, and bounded semantic commands. Printable text uses the host's Unicode event path instead of assuming a US keyboard layout. Shortcut keys use physical DOM codes so modifier chords retain key-down/key-up order. Three-finger recognition runs only when touch forwarding is off and no Pencil is active. Blur, backgrounding, disconnect, peer replacement, credential rotation, and process shutdown release every held mouse button, key, pen, and touch contact.

The host validates packet version, message kind, exact/minimum length, enum values, finite numbers, pressure, tilt, UTF-8, field-size limits, and sample count before injection. Normalized points are mapped against the selected monitor's desktop rectangle, including negative origins. Windows uses synthetic-pointer APIs for pen/touch and `SendInput` for mouse, wheel, keyboard, and text. macOS posts Quartz tablet-subtype mouse events with `0–1` pressure, normalized tilt, rotation, and device identity, plus normal Quartz mouse/key/scroll/text events. Semantic commands become Alt+Tab/Task View on Windows or Command+Tab/Mission Control on macOS. The authenticated WebSocket accepts the same binary messages as the preferred reliable/ordered DataChannel.

## Session lifecycle

Each process run or credential rotation creates a UUID, random six-digit PIN, random 256-bit QR secret, and no access token. A correct PIN or QR secret issues a random 256-bit token, stored server-side only as SHA-256. The WebSocket receives it through an HttpOnly same-site cookie so access tokens do not appear in URLs or request logs. One `ActivePeer` replaces and closes any prior WebRTC peer. Manual reset and focused-window expiry rotation invalidate the old token and signal every active transport to close.

## File-transfer path

Bulk files deliberately do not use the reliable real-time DataChannel. The authenticated HTTP path keeps large transfers out of the input queue, supports browser-native download streaming/ranges, and bounds upload memory to one 1 MiB chunk per request. The file manager owns no general filesystem browser: the host exposes only canonical regular files explicitly chosen into an in-memory Outbox, while iPad uploads can end only in a host-selected Inbox.

Safari creates an upload ticket containing a sanitized leaf name and declared size, then sends sequential chunks with an exact offset and a SHA-256 digest. The host verifies each digest before writing, rejects stale/out-of-order offsets with the authoritative resume point, and holds partial data in its private NFiDB configuration directory. Completion requires the declared byte count, hashes the complete staged file, selects a collision-free Inbox name, then atomically renames it. Cancel, disconnect, credential rotation, or the next process start removes owned `.part` files.

Downloads reopen and revalidate the queued file's size/modification timestamp, support one byte range, emit `Content-Disposition` and `Accept-Ranges`, and stream fixed 64 KiB buffers. A client cleanup request is attached to that stream's guard: only complete whole-file delivery removes the matching Outbox ID, while range and interrupted bodies remain retryable. Each file in a batch has an independent guard. Queue metadata contains no source path. A single checksum worker prevents a large selection from spawning unbounded hashing threads. Transfer rate/history state is bounded, and each streamed block checks that the paired session is still current.

The transfer pacer is independent of WebRTC. It uses a configurable throughput ceiling and, by default, waits while host metrics report any active pointer contact. This is application-level prioritization rather than network QoS, but prevents the file path from intentionally filling the real-time input channel or consuming unbounded RAM.

## Diagnostic recorder

Safari sends one authenticated structured diagnostic sample per second. The transport pairs it with a contemporaneous immutable host metrics snapshot and appends it to a six-hour bounded in-memory ring. The desktop Diagnostics page renders the latest raw evidence and aggregate percentile distributions; JSON export serializes configuration, capture/encoder identity, current host counters, limitations, all retained samples, and processed statistics. The recorder never writes unless the user selects export and never sends data off the LAN.

The HTTP bootstrap exists because iPad Safari must first reach an ordinary LAN address. NFiDB does not claim hostile-network confidentiality; use it only on a trusted private network. Details are in `SECURITY.md`.

## Failure containment

Capture failure leaves input transport available. WebRTC video failure falls back to authenticated WebSocket input. mDNS failure leaves the numeric IPv4 URL. Input is cancelled on disconnect rather than attempting to resume a possibly stale pointer lifecycle.
