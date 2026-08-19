# Pointer and signaling protocol

Protocol version `1` uses little-endian binary messages. Every message starts with the version byte and a message-kind byte. Pointer batches remain kind `1`; version 1 receivers can now also accept wheel, keyboard, committed text, and semantic-command kinds. Input normally travels through the reliable ordered WebRTC DataChannel and uses the authenticated WebSocket as its fallback.

## Batch header

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | version (`1`) |
| 1 | `u8` | message kind (`1` = pointer batch) |
| 2 | `u16` | sample count |
| 4 | `u32` | batch sequence |
| 8 | `f64` | browser epoch send time, ms (`Date.now()` timebase) |

## Sample

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | device (`1` pen, `2` touch, `3` mouse) |
| 1 | `u8` | action (`1` down, `2` move, `3` up, `4` cancel, `5` hover) |
| 2 | `u16` | browser Pointer Events `buttons` flags: bit 0 primary tip, bit 1 secondary/barrel |
| 4 | `u32` | pointer ID |
| 8 | `u32` | monotonically wrapping sample sequence |
| 12 | `f32` | normalized X |
| 16 | `f32` | normalized Y |
| 20 | `f32` | pressure, clamped to 0…1 |
| 24 | `f32` | X tilt, clamped to −90…90 degrees |
| 28 | `f32` | Y tilt, clamped to −90…90 degrees |
| 32 | `f32` | twist, normalized to 0…359 degrees |
| 36 | `f64` | browser event epoch timestamp, ms (converted from the Event Timing timebase when necessary) |

One pointer batch consists of the 16-byte header followed by zero or more 44-byte samples. The maximum batch size is 512 samples.

## Wheel message (kind 2)

The wheel message is exactly 32 bytes. Modifier bits are Shift `1`, Control `2`, Alt `4`, and Meta/Windows `8`.

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | version (`1`) |
| 1 | `u8` | message kind (`2`) |
| 2 | `u16` | modifier bits |
| 4 | `u32` | sequence |
| 8 | `f32` | normalized X |
| 12 | `f32` | normalized Y |
| 16 | `f32` | horizontal pixel delta |
| 20 | `f32` | vertical pixel delta |
| 24 | `f64` | browser event epoch timestamp, ms |

Safari line/page deltas are converted to pixels before transmission. The Windows sink retains fractional high-resolution wheel remainders and maps the position through the selected monitor and full virtual desktop.

## Keyboard message (kind 3)

The keyboard message has a 24-byte header followed by the UTF-8 DOM `code` bytes and UTF-8 DOM `key` bytes. The physical `code` is required, ASCII-only, and 1–64 bytes; `key` is 0–64 bytes. Action is `1` down or `2` up. Client time is rounded to a non-negative epoch millisecond `u64`.

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | version (`1`) |
| 1 | `u8` | message kind (`3`) |
| 2 | `u8` | key action |
| 3 | `u8` | DOM location |
| 4 | `u16` | modifier bits |
| 6 | `u8` | repeat flag |
| 7 | `u8` | reserved (`0`) |
| 8 | `u32` | sequence |
| 12 | `u64` | browser event epoch timestamp, ms |
| 20 | `u16` | code byte length |
| 22 | `u16` | key byte length |
| 24 | bytes | code, then key |

The host maps standard DOM physical codes for modifiers, navigation, letters, digits, punctuation, function keys F1–F24, and numpad keys to Windows virtual keys. Physical keys outside the text-entry field retain down/up/repeat behavior for held controls and drawing-app hotkeys. Option becomes Alt, Control becomes Ctrl, Return becomes Enter, and unmodified iPad Delete becomes Backspace. Key state is tracked and released on every reset/disconnect.

## Text message (kind 4)

Committed hardware/software-keyboard, paste, and IME text uses Unicode injection and is independent of the Windows keyboard layout. The message is a 20-byte header followed by 1–4096 valid UTF-8 bytes. Long text is split only at Unicode character boundaries.

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | version (`1`) |
| 1 | `u8` | message kind (`4`) |
| 2 | `u16` | reserved (`0`) |
| 4 | `u32` | sequence |
| 8 | `u64` | browser event epoch timestamp, ms |
| 16 | `u32` | UTF-8 byte length |
| 20 | bytes | committed text |

## Command message (kind 5)

The command message is exactly 16 bytes: version, kind, command, reserved byte, `u32` sequence, and `u64` client epoch milliseconds. Commands are `1` next app (Alt+Tab), `2` previous app (Alt+Shift+Tab), `3` minimize foreground, `4` Task View (Win+Tab), and `5` reset all held remote input. The browser produces commands through on-screen controls and three-finger gestures rather than sending platform-specific implementation details.

Rust and TypeScript have matching golden-vector tests. A packet with unknown version/kind/device/action/command, malformed UTF-8, an invalid key field, a non-finite float, the wrong exact byte length, or too many samples is rejected before reaching the native sink.

## Coordinate mapping

The client computes the displayed video rectangle for fit, fill, or 1:1 mode, then normalizes input to that rectangle. In fit mode, points in letterboxing are ignored. Fill mode permits cropped source coordinates. The host maps normalized coordinates into the selected monitor rectangle, including negative virtual-desktop origins, and clamps the final pixel to the target edge.

## Signaling endpoints

- `GET /api/status`: public per-run metadata; no secret is returned.
- `POST /api/pair`: accepts `{pin}` or `{qr_secret}` and returns an access token while also setting the WebSocket HttpOnly cookie.
- `POST /api/webrtc/offer`: accepts browser SDP plus the token and returns the complete host SDP answer after ICE gathering.
- `GET /api/ws`: authenticated WebSocket control/input fallback using the pairing cookie.
- `GET /api/metrics`: authenticated current host counters.
- `GET /api/diagnostics`: authenticated processed summary of the bounded diagnostic recording.
- `GET /api/files`: authenticated Outbox, current-session uploads, bounded recent history, limits, rates, and counters. Windows source paths are never serialized.
- `POST /api/files/uploads`: creates a current-session staged upload from `{upload_id, name, mime, size}` and returns its UUID, verified offset, and 1 MiB chunk size. Retrying the same browser-generated UUID returns the same ticket.
- `GET /api/files/uploads/{id}`: returns the authoritative received offset for interruption recovery.
- `PUT /api/files/uploads/{id}`: accepts one raw chunk with `x-nfidb-offset` and `x-nfidb-chunk-sha256`; stale or corrupt chunks return `409` and `expected_offset`.
- `POST /api/files/uploads/{id}/complete`: requires the declared size, computes the complete SHA-256, and atomically publishes the file in the Inbox. Repeating completion after a lost response returns the same result without creating a duplicate.
- `DELETE /api/files/uploads/{id}`: cancels the upload and removes its owned partial file.
- `GET /api/files/outbox/{id}/download`: streams an explicitly queued Windows file with attachment headers and single-range support.
- `DELETE /api/files/outbox/{id}`: removes the queue entry without deleting the Windows source file.
- `POST /api/disconnect`: closes the peer, resets native input, and invalidates the token.

File mutations additionally reject a present `Origin` header unless it exactly matches the request host. Upload names are leaf-only, Windows-reserved/invalid characters are neutralized, files never overwrite an Inbox entry, and active transfer requests recheck the session during pacing. Bulk payloads are HTTP-only; WebRTC DataChannel and WebSocket queues remain reserved for bounded input/control messages.

WebRTC uses local host candidates only—there is no STUN, TURN, or relay service. Video is H.264; input uses a reliable ordered DataChannel named `nfidb-input`.

## Diagnostic control messages

The authenticated WebSocket carries JSON control/telemetry in addition to binary fallback input. The browser sends a `client-diagnostics` sample at most once per second and skips it rather than growing a queue when the socket has more than 256 KiB buffered. Each sample includes client version, device/viewport state, connection states, video frame counters and dimensions, RTP loss/jitter/throughput, selected-candidate properties, decoder/jitter-buffer costs, animation-frame timing, video-frame callback timing, browser buffer levels, and the relevant raw WebRTC statistics. If a Safari API throws, a minimal sample marks `diagnosticFallback` and includes a bounded error string rather than silently stopping the recorder.

Ping/pong messages contain four epoch timestamps. The browser calculates NTP-style round-trip time and server/client clock offset; the host uses that offset to estimate input arrival age. These are engineering estimates on a LAN, not a substitute for an externally synchronized glass-to-glass measurement.

The host synchronizes each browser sample with its own capture, encode, transport, input continuity, and native injection metrics. It retains 21,600 samples (approximately six hours at one hertz) in memory. Resetting the recording clears this bounded buffer; exporting writes both raw samples and count/min/mean/p50/p95/p99/max summaries to a local JSON file.
