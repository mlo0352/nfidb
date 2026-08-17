# Pointer and signaling protocol

Protocol version `1` uses little-endian binary messages. One pointer batch consists of a 16-byte header followed by 44-byte samples. The maximum batch size is 512 samples.

## Batch header

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | `u8` | version (`1`) |
| 1 | `u8` | message kind (`1` = pointer batch) |
| 2 | `u16` | sample count |
| 4 | `u32` | batch sequence |
| 8 | `f64` | browser `performance.now()` send time, ms |

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
| 36 | `f64` | browser event timestamp, ms |

Rust and TypeScript have matching golden-vector tests. A packet with unknown version/kind/device/action, a non-finite float, the wrong exact byte length, or too many samples is rejected before reaching the native sink.

## Coordinate mapping

The client computes the displayed video rectangle for fit, fill, or 1:1 mode, then normalizes input to that rectangle. In fit mode, points in letterboxing are ignored. Fill mode permits cropped source coordinates. The host maps normalized coordinates into the selected monitor rectangle, including negative virtual-desktop origins, and clamps the final pixel to the target edge.

## Signaling endpoints

- `GET /api/status`: public per-run metadata; no secret is returned.
- `POST /api/pair`: accepts `{pin}` or `{qr_secret}` and returns an access token while also setting the WebSocket HttpOnly cookie.
- `POST /api/webrtc/offer`: accepts browser SDP plus the token and returns the complete host SDP answer after ICE gathering.
- `GET /api/ws`: authenticated WebSocket control/input fallback using the pairing cookie.
- `POST /api/disconnect`: closes the peer, resets native input, and invalidates the token.

WebRTC uses local host candidates only—there is no STUN, TURN, or relay service. Video is H.264; input uses a reliable ordered DataChannel named `nfidb-input`.
