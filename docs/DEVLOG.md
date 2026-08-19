# Engineering devlog

## 2026-08-19 — Download queue completion and arrival feedback

### Goal

Make Windows-to-iPad delivery visible without leaving the Files panel open, prevent completed downloads from lingering in the Outbox, and keep multi-file behavior safe under concurrent streams.

### Implementation

The paired page now polls the small authenticated file listing throughout the session, announces newly queued IDs, and badges the Files control. Safari download URLs opt into cleanup through a persisted default-on preference. The host owns the completion decision: only a stream covering the entire file can remove its matching Outbox ID after all bytes are produced. Partial ranges, canceled clients, session changes, and read failures retain the source queue entry. Download all starts each normal Safari download independently. Transfer labels use plain text to avoid missing-glyph boxes, and the Windows Files page exposes its received-files folder directly.

### Evidence

**PASS IN AUTOMATION; PHYSICAL SAFARI BATCH PROMPT PENDING.** Concurrent manager tests consume two complete bodies and prove each queue ID disappears, then prove partial and interrupted bodies remain. Browser tests cover default and persisted preference state, cleanup URLs, two-file arrival notice/badge, batch control, and glyph-safe history. The packaged-release smoke transferred two outbound files totaling 7,340,252 bytes with exact hashes, retained a 1,024-byte partial response, then completed and auto-cleared both IDs with zero failures.

### Decision

Ship as v0.5.1. Keep the cleanup signal opt-in at the HTTP URL and default it on in the UI; never infer completion from a browser click or remove a source queue entry before the host finishes its body.

## 2026-08-19 — Paired bidirectional file transfer

### Goal

Move explicitly selected files in either direction without an iPad app, cloud relay, arbitrary filesystem access, or interference with the drawing input queue.

### Implementation

Added a session-authenticated HTTP transfer manager beside WebRTC rather than putting bulk bytes on the reliable input DataChannel. iPad uploads use sequential 1 MiB chunks, per-chunk SHA-256, authoritative offsets, bounded retries, idempotent browser-generated tickets/finalization, private `.part` staging, full-file hashing, collision-safe final names, progress, cancellation, and cleanup. Cross-volume Inbox moves copy to an owned temporary leaf and rename locally before exposing the final name. Windows downloads expose only regular files chosen into an in-memory Outbox, revalidate the source before streaming, support one byte range, and use Safari's native download manager. Windows source paths never enter the browser response.

The Files pages on both devices show queues, rates, outcomes, checksums, and bounded history. Default bulk throughput is capped at 32 Mbps and the pacer waits during an active pointer contact. Pairing reset/disconnect removes unfinished uploads and invalidates in-flight streaming at the next block. Limits bound individual files, active uploads, Outbox entries, recent records, upload request memory, and download buffers.

### Evidence

**PASS IN AUTOMATION; PHYSICAL IPAD FILE UI PENDING.** Manager and browser tests cover SHA vectors, sequential/corrupt/stale chunks, lost-response recovery, retry/cancel behavior, leaf/reserved-name handling, duplicate names, path privacy, ranges, and drawing-contact pacing. The packaged-release smoke rejected an unauthenticated listing, uploaded 3,146,237 bytes with matching source/server/destination SHA-256, repeated creation/completion without a duplicate, canceled and removed a partial upload, downloaded 5,243,017 bytes with a matching SHA-256, and matched a separate 1,024-byte range. Final server counters reported zero failures or active transfers.

### Decision

Ship the pipeline in v0.5.0 alpha, while explicitly keeping physical iPad Files/Photos selection, Safari downloads, foreground/background interruption, and very-large-file behavior as field-validation items. Preserve HTTP as the bulk path and real-time channels as bounded control/input paths.

## 2026-08-18 — Physical iPad remote-control acceptance

### Goal

Confirm that the new mouse, keyboard, and three-finger controls survive real iPadOS/Safari event handling rather than only browser automation.

### Experiment

Paired a physical iPad to the release host and exercised trackpad/mouse movement, typing, Caps Lock, Shift, numbers and shifted symbols, Tab, Option+Tab, three-finger app switching, and three-finger minimize.

### Result

**PASS.** Pointer control and typing were responsive; keyboard state and punctuation arrived correctly; Option+Tab switched Windows apps; mouse, minimize, and swipe app switching worked. Control+Option+Delete did not open the secure-attention screen, matching the documented Windows restriction on synthetic Ctrl+Alt+Delete from an unsigned user-mode process.

### Decision

Promote v0.4.0 as a solid alpha, mark physical iPad trackpad/keyboard and gesture rows passed, keep the secure-attention limitation explicit, and retain the physical per-art-app Pencil matrix as a stable-release gate.

## 2026-08-18 — Browser first-frame recovery

### Goal

Recover automatically when WebRTC is connected but Safari has not presented a decodable H.264 frame.

### Experiment

Reproduced the visible symptom, then removed a conflicting second capture session and isolated one real 3840×2160 Windows monitor through the production capture, Balanced 1920×1080 encode, WebRTC, and browser decode path. Added a presented-frame watchdog that requests a fresh IDR over the authenticated WebSocket, with DataChannel fallback, until the browser presents a frame. Added a host counter and live control-path regression.

### Result

**PASS.** The isolated real-monitor run presented its first frame in 96.4 ms after a 62.549 ms host IDR wait, decoded at 1920×1080, advanced continuously, and reported zero RTP loss, decoder drops, freezes, or transport drops. The final packaged regression started in 144.7 ms; its authenticated recovery request advanced the host recovery counter `0→1`, generated a new keyframe, and advanced browser keyframes decoded `1→2`. The intermittent physical-Safari stall was not reproduced after restarting the single capture session, so the precise cause of the originally missed first IDR remains unproven; the recovery handshake directly covers that failure mode.

### Decision

Do not treat `RTCPeerConnection.connected` or receipt of a video track as proof that the user can see a picture. Continue requesting bounded fresh IDRs until `requestVideoFrameCallback`/playback confirms presentation, and surface the recovery count in diagnostics.

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

## 2026-08-18 — Physical iPad startup-delay finding

### Goal

Validate the browser client on real iPad Safari and distinguish input latency from video latency.

### Experiment

Paired a physical iPad over the local Wi-Fi and used touch while observing the Windows host counters and remote picture. Input counters reacted immediately, but the first decodable picture arrived roughly 30 seconds later; after catching up, playback was close to real time. Inspection showed that a new peer joined the already-running H.264 broadcast on an arbitrary delta frame, with no connection-time IDR request or first-frame keyframe gate. The encoder also scheduled IDRs by requested frame count, which can become a long wall-clock interval when software encoding is overloaded.

### Result

**FAIL, THEN FIXED IN AUTOMATION; PHYSICAL RETEST PENDING.** The receiver now starts at the broadcast tail only after WebRTC reaches connected, requests a fresh IDR, rejects every delta frame until that IDR, and requests recovery IDRs at a one-second wall-clock interval. The client and host report browser first-frame time, IDR wait, and skipped pre-IDR frames.

The release-mode 1080p Balanced regression joined an already-running encoder, skipped one pre-IDR delta frame, received the requested IDR in 69.3 ms, and rendered the first browser frame in 464.5 ms. It then completed ten seconds with 2,402/2,402 exact input samples, zero RTP loss, decoder drops, freezes, sequence gaps, or integrity mismatches.

### Decision

Treat first-frame latency as an explicit release gate. A receiver must never be handed an H.264 delta frame before its first IDR, and keyframe recovery must be bounded by elapsed time rather than nominal frame count.

## 2026-08-18 — Real-device diagnostic console and renewable pairing

### Goal

Turn physical-iPad testing into captured evidence instead of transient counters, make stale QR/PIN recovery immediate, and make every desktop sidebar destination functional.

### Experiment

Added one-hertz authenticated browser samples covering device/viewport/orientation state, connection state, decoder and presentation counters, RTP bandwidth/loss/jitter, jitter-buffer and decode timing, selected candidate properties, frame callback percentiles, Safari frame metadata when available, input buffers, and raw RTC fields. Each sample is synchronized with host capture, encode, transport, continuity, input-arrival, and native-injection counters. The host keeps a six-hour bounded recording, renders live and percentile-processed views, and exports raw plus processed JSON locally.

Moved session credentials into renewable state. Manual reset and focused-window expiry rotation now create a new session/PIN/QR secret, invalidate the old token, close the peer/socket, and release input. Replaced the decorative sidebar labels with functional Session, Source, Input, Diagnostics, and App Setup pages.

### Result

**PASS IN AUTOMATION; PHYSICAL DETAIL RUN IN PROGRESS.** The final v0.3.0 candidate regression retained 11/11 diagnostic samples over 10.002 seconds, with zero discarded samples, and preserved 2,402/2,402 simultaneous input samples. The new join gate produced an IDR in 63.856 ms and browser video in 92.4 ms. The run reported zero RTP loss, decode drops, freezes, input gaps, transport skips, or marker mismatches. Its p95 LAN RTT was 1.999 ms; Edge did not expose capture-time metadata, so the 92.814 ms p95 pipeline result was correctly labeled as a component estimate.

### Decision

Ship diagnostic reports with explicit measurement limitations and never present an estimate as physical glass-to-glass latency. Exclude startup/uninitialized zeroes from processed rate/latency distributions. Keep diagnostic data memory-bounded, local-only, and user-exported.

## 2026-08-18 — Physical Safari diagnostic bootstrap

### Finding

The first real-iPad diagnostic run paired and delivered pressure/tilt input, but the desktop recorder received no client samples. The browser assets had stable filenames combined with a one-year immutable cache header, so Safari could legitimately reuse the v0.2 client after the host executable changed. Safari RTC stats iteration also needed a compatibility path that did not depend on `RTCStatsReport.values()`.

### Fix

The production browser bundle now uses content-hashed JavaScript/CSS filenames while `index.html` remains `no-store`, guaranteeing a new executable points Safari at a new asset URL. RTC reports are collected through the broadly supported `forEach` interface, unavailable stats degrade to zero/fallback fields rather than aborting the one-second sample, and video playback-quality access is feature-tested. If any later browser API still throws, the client sends a minimal diagnostic sample containing its version and the bounded error string, so the recorder remains useful and exposes the incompatible operation. The live Stats panel identifies its client version and fallback count. The portable smoke discovers the hashed script URL from the embedded HTML instead of assuming a filename.

### Decision

Release the physical-Safari bootstrap correction as v0.3.1 and require a real-device sample to appear before considering the recorder validated.

## 2026-08-18 — Physical iPad real-content encoder feedback

### Finding

The first clean v0.3.1 physical run proved the diagnostic recorder, DataChannel input, UDP host candidate, and zero-loss transport, but exposed severe video pacing under real 4K desktop content. When OpenH264 fell below ten frames per second, the one-second wall-clock recovery interval forced a full IDR every few encoded frames. Those expensive frames reduced throughput further, which made the keyframe ratio still worse. The run retained 111/111 samples and reported zero RTP packet loss, input gaps, input errors, buffered bytes, or transport skips, isolating the encoder rather than the LAN or input path.

### Fix

Connection startup continues to issue an immediate edge-triggered IDR and reject undecodable deltas. Periodic corruption recovery now forces an IDR every five seconds instead of every second. This bounds recovery without letting recovery frames dominate an overloaded software encoder.

### Decision

Re-run the same physical foreground workload before release and keep the hardware Media Foundation encoder as the principal path to true high-frame-rate 1080p/4K output.

## 2026-08-18 — Pencil tip was mapped as a barrel click

### Finding

On the physical iPad, ordinary Pencil contact selected/resized in Paint and panned the canvas in Rebelle instead of drawing. Browser Pointer Events defines `buttons` bit 0 as the primary pen tip and bit 1 as the secondary/barrel button. The Windows injector incorrectly interpreted bit 0 as `PEN_FLAG_BARREL`, advertising every normal stroke as a right-click/barrel gesture.

### Fix

The host now maps only browser secondary-button bit 1 to `PEN_FLAG_BARREL`; primary tip contact continues to use `POINTER_CHANGE_FIRSTBUTTON_DOWN/UP` with no pen barrel flag. A native unit regression asserts none/primary/secondary/combined button mappings explicitly.

### Decision

Treat working primary Pencil ink in physical drawing applications as a release gate. Transport-only pressure/tilt tests are necessary but cannot substitute for application behavior.

## 2026-08-18 — Bursty physical Pencil injection

### Finding

The first v0.3.2 physical report carried 10,731 Pencil samples with zero incoming sequence gaps, lifecycle errors, reordering, buffered bytes, or transport skips, but recorded four native injection errors. Each occurred during a 160–236-sample/s interval, and the recorded maximum injection call was 6.003 ms—direct evidence that the five-millisecond transient `ERROR_NOT_READY` retry expired while Windows was still draining a prior synthetic pen update.

### Fix

Transient `ERROR_NOT_READY` responses now retry for up to 50 ms with a 100-microsecond backoff instead of busy-spinning for five milliseconds. Other Windows errors still fail immediately. This favors line continuity during a short OS input-queue stall while preserving a finite upper latency bound.

### Decision

Require both the sustained 240-sample/s native stress test and a second physical recorder pass to report zero injection errors before release.

## 2026-08-18 — Pencil lifecycle at fit-mode edges

### Finding

A post-fix physical run delivered 10,358 real Pencil samples with zero sequence gaps or reordering and full pressure/tilt variation, but accumulated 12 lifecycle errors. The client rejected coordinates in the narrow letterbox outside the fitted monitor image. A stroke whose down or up occurred there could therefore send movement without its matching lifecycle boundary.

### Fix

An active stroke now clamps movement and its terminal sample to the nearest monitor edge instead of dropping them. Contact that begins outside the fitted image and then enters it is recovered with exactly one primary Down. Duplicate or browser-specific coalesced lifecycle samples are normalized to one Down, zero or more Moves, and one Up/Cancel. Deterministic regressions cover edge exit, edge entry, duplicate Down, and coalesced lifecycle lists.

### Decision

Treat both sequence continuity and lifecycle continuity as release gates. A zero packet-gap count alone is insufficient if coordinate filtering can remove Down or Up.

## 2026-08-18 — Automatic six-digit pairing

### Goal

Remove the extra Connect tap from the normal iPad pairing path without making partial or repeated requests.

### Result

Entering or pasting the sixth PIN digit now submits immediately. The client keeps the Connect button and form submission as fallbacks, rejects partial PINs locally, formats pasted input consistently, disables duplicate requests while pairing, and restores an editable form after an error. The live-host browser regression now reaches the drawing surface by filling the PIN alone.

### Decision

Keep PIN entry one-step: the sixth digit is explicit enough to begin a local pairing check, while QR pairing remains zero-entry.

## 2026-08-18 — iPad trackpad, keyboard, and finger shortcuts

### Goal

Let an iPad trackpad/keyboard control the Windows target without compromising Pencil semantics, and turn otherwise-idle finger input into deliberate Windows app gestures.

### Implementation

Extended protocol version 1 with strictly bounded wheel, key-transition, committed UTF-8 text, and semantic-command messages. Safari forwards physical trackpad hover/drag/buttons and wheel deltas; hardware shortcuts retain down/up ordering; printable hardware, software-keyboard, paste, and IME commits use Windows Unicode injection. Option maps to Alt, Control to Ctrl, Shift to Shift, Return to Enter, and unmodified Delete to Backspace. Control+Option+Delete sends the Windows Delete chord while retaining Windows' protected secure-attention boundary.

With native touch forwarding off, a dedicated three-finger recognizer sends next/previous app, Task View, or minimize commands. Active Pencil contact suppresses these gestures. Browser blur/background, transport replacement, disconnect, credential rotation, and host shutdown release held buttons, keys, and contacts. Diagnostics count message categories and text bytes without recording typed content.

### Evidence

Strict TypeScript, 22 browser tests, Rust formatting/check/Clippy, and 25 Rust tests pass. Two real-host WebRTC comparisons each delivered 1,202/1,202 sustained Pencil samples near 240 samples/s while independently verifying three mouse samples, one wheel event, four Option+Tab transitions, one 17-byte Unicode commit, and one semantic command. Both had zero input errors, sequence/lifecycle gaps, ordering faults, buffer growth, RTP loss, decoder drops, freezes, integrity mismatches, or transport drops.

### Decision

Keep secure-attention behavior explicit rather than claiming synthetic Ctrl+Alt+Delete can cross Windows' security desktop. Retain on-screen shortcut buttons because iPadOS can reserve hardware shortcuts before Safari. Physical iPad trackpad, keyboard-layout/IME, and gesture arbitration remain required field tests before declaring the feature stable.
