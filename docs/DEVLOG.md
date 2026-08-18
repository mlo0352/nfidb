# Engineering devlog

## 2026-08-17 — Synthetic pen injection spike

### Goal

Prove that the documented Windows synthetic pointer API produces real `PT_PEN` messages with pressure and tilt before coupling it to Safari or WebRTC.

### Experiment

Built `pointer-sink`, a native `WM_POINTER` window using `GetPointerPenInfo`, and a self-test that injects down/move/move/up through the production `PointerInjector`.

### Result

**PASS.** The receiver observed four messages and the synthetic pen ended released.

### Measurements

- Pressure: `102, 512, 1024, 0` (expected 10%, 50%, 100%, release)
- Tilt X: `0, 30, 0, 30`
- Tilt Y: `0, 0, -30, -30`

### Decision

Use `CreateSyntheticPointerDevice(PT_PEN)` and `InjectSyntheticPointerInput` as the production Windows input path. Keep the sink as an independent regression tool.

## 2026-08-17 — Pointer packet and mapping contract

### Goal

Prevent silent Rust/TypeScript drift and verify mapping on multi-monitor virtual desktops.

### Experiment

Implemented the same fixed header/sample layout and golden values in Rust and TypeScript. Added fit/fill/1:1 tests plus a target at a negative X origin.

### Result

**PASS.** Six Rust protocol tests and five browser tests pass. Invalid lengths, versions, enums, excessive sample counts, and non-finite fields are rejected.

### Decision

Keep protocol versioning at the packet boundary and reject rather than guess. Predicted browser samples remain local-only.

## 2026-08-17 — Capture backpressure and encoder fallback

### Goal

Build a bounded capture pipeline that can evolve independently of transport.

### Experiment

Connected Windows Graphics Capture to a one-frame replacement slot and then to profile-aware scaling and OpenH264 Baseline screen-content encoding. Added a deterministic 1280×720 moving test pattern.

### Result

**PARTIAL.** WGC and the newest-frame design compile and exercise the transport, but debug software encoding measured roughly 120 ms per 720p frame (~7 encoded FPS) on the development PC.

### Decision

Ship the software path only as the functional MVP fallback and keep its limitation prominent. The next performance layer is a Media Foundation hardware encoder, without replacing WGC, packet mapping, or WebRTC.

## 2026-08-17 — Live browser/WebRTC integration

### Goal

Prove that signaling, local ICE, H.264 packetization/decoding, and browser input work together—not merely as isolated unit tests.

### Experiment

Started the real host with the generated video source and logging input sink. Playwright drove Microsoft Edge through PIN pairing, waited for `RTCPeerConnection` connected, required non-zero `videoWidth`, and dispatched pen down/move/up.

### Result

**PASS.** The run completed in 5.6 seconds. The host reached connected, the browser decoded video, all three pointer packets arrived in order, and disconnect reset input state.

### Decision

Keep this as an opt-in live integration test. CI runs deterministic unit/build checks; the live test runs when a host URL and PIN are supplied.

## 2026-08-17 — Token URL leakage review

### Goal

Ensure normal request logs do not reveal session credentials.

### Experiment

The first WebSocket implementation placed the token in a query parameter. A debug integration run showed the full URL in HTTP tracing.

### Result

**FAIL, THEN FIXED.** The WebSocket now authenticates with an HttpOnly same-site cookie set by the pairing response. The URL contains no credential.

### Decision

Never put PINs, QR secrets, or access tokens in a request URI. Headless test output is the sole deliberate PIN disclosure.

## 2026-08-17 — Windows Graphics Capture compatibility smoke

### Goal

Prove capture against a real monitor on the development Windows 11 build, rather than only the generated frame source.

### Experiment

Started the headless host against the 3840×2160 primary monitor, paired an authenticated metrics WebSocket, and required non-zero capture and encode rates.

### Result

**FAIL, THEN PASS.** The initial run requested WGC's optional minimum-update-interval feature, which build 22631 rejected. NFiDB already rate-limits in the frame callback, so the WGC setting was changed to platform default. The rerun produced real frames.

### Measurements

- 3840×2160 source
- 28 captured FPS
- 3 encoded FPS at 257.2 ms/frame in a debug build
- 27 stale frames dropped by the one-frame slot

### Decision

Keep callback rate limiting and avoid the newer optional WGC interval API so the host supports older Windows 11 builds. The measurement reinforces the Media Foundation encoder priority.

## 2026-08-17 — Sustained input and 4K receiver validation

### Goal

Detect silent line holes, pressure/angle flattening, transport switching, media-clock drift, tearing, and unbounded video queues at realistic Pencil sample rates.

### Experiment

Added independent continuity/lifecycle metrics, sticky per-contact transport, bounded browser buffers, User32 retry and pointer-history recovery, a deterministic native stress receiver, duplicated video integrity markers, authenticated browser/host diagnostics, and a LAN Playwright benchmark. The live soak generated 60 parent pointer events per second with four chronological coalesced samples per event while decoding a 4K-source H.264 stream.

### Result

**PASS within the tested hardware envelope.** The native receiver accepted all 14,400 exact samples over one minute, including 4,846 recovered through `GetPointerPenInfoHistory`. The ten-minute WebRTC run delivered and accepted all 144,002 samples at 240.001 Hz with full pressure and ±60° tilt ranges. It decoded 30,508 video frames with zero RTP loss, decoder drops, freezes, media regressions, transport drops, or integrity-marker mismatches.

### Decision

Use actual encoded-frame intervals for RTP timestamps and keep newest-frame replacement as the explicit low-latency policy. Publish software profile throughput rather than imply 60 fps at every resolution. Keep physical iPad, art-application, WAN-disconnect, and glass-to-glass latency checks as explicit unverified gates.
