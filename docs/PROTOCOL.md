# Pointer and signaling protocol

Protocol version `1` uses little-endian binary messages. One pointer batch consists of a 16-byte header followed by 44-byte samples. The maximum batch size is 512 samples.

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
| 0 | `u8` | device (`1` pen, `2` touch) |
| 1 | `u8` | action (`1` down, `2` move, `3` up, `4` cancel) |
| 2 | `u16` | browser button flags |
| 4 | `u32` | pointer ID |
| 8 | `u32` | monotonically wrapping sample sequence |
| 12 | `f32` | normalized X |
| 16 | `f32` | normalized Y |
| 20 | `f32` | pressure, clamped to 0…1 |
| 24 | `f32` | X tilt, clamped to −90…90 degrees |
| 28 | `f32` | Y tilt, clamped to −90…90 degrees |
| 32 | `f32` | twist, normalized to 0…359 degrees |
| 36 | `f64` | browser event epoch timestamp, ms (converted from the Event Timing timebase when necessary) |

Rust and TypeScript have matching golden-vector tests. A packet with unknown version/kind/device/action, a non-finite float, the wrong exact byte length, or too many samples is rejected before reaching the native sink.

## Coordinate mapping

The client computes the displayed video rectangle for fit, fill, or 1:1 mode, then normalizes input to that rectangle. In fit mode, points in letterboxing are ignored. Fill mode permits cropped source coordinates. The host maps normalized coordinates into the selected monitor rectangle, including negative virtual-desktop origins, and clamps the final pixel to the target edge.

## Signaling endpoints

- `GET /api/status`: public per-run metadata; no secret is returned.
- `POST /api/pair`: accepts `{pin}` or `{qr_secret}` and returns an access token while also setting the WebSocket HttpOnly cookie.
- `POST /api/webrtc/offer`: accepts browser SDP plus the token and returns the complete host SDP answer after ICE gathering.
- `GET /api/ws`: authenticated WebSocket control/input fallback using the pairing cookie.
- `GET /api/metrics`: authenticated current host counters.
- `GET /api/diagnostics`: authenticated processed summary of the bounded diagnostic recording.
- `POST /api/disconnect`: closes the peer, resets native input, and invalidates the token.

WebRTC uses local host candidates only—there is no STUN, TURN, or relay service. Video is H.264; input uses a reliable ordered DataChannel named `nfidb-input`.

## Diagnostic control messages

The authenticated WebSocket carries JSON control/telemetry in addition to binary fallback input. The browser sends a `client-diagnostics` sample at most once per second and skips it rather than growing a queue when the socket has more than 256 KiB buffered. Each sample includes device/viewport state, connection states, video frame counters and dimensions, RTP loss/jitter/throughput, selected-candidate properties, decoder/jitter-buffer costs, animation-frame timing, video-frame callback timing, browser buffer levels, and the relevant raw WebRTC statistics.

Ping/pong messages contain four epoch timestamps. The browser calculates NTP-style round-trip time and server/client clock offset; the host uses that offset to estimate input arrival age. These are engineering estimates on a LAN, not a substitute for an externally synchronized glass-to-glass measurement.

The host synchronizes each browser sample with its own capture, encode, transport, input continuity, and native injection metrics. It retains 21,600 samples (approximately six hours at one hertz) in memory. Resetting the recording clears this bounded buffer; exporting writes both raw samples and count/min/mean/p50/p95/p99/max summaries to a local JSON file.
