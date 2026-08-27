# Performance

NFiDB favors fresh input and pictures over complete frame delivery. Both the capture-to-encoder boundary and video broadcast are bounded; an overloaded stage drops stale frames.

## Profiles

| Profile | Maximum width | Target bitrate | Target frame rate |
| --- | ---: | ---: | ---: |
| Fast | 1280 | 5 Mbps | configuration, default 60 |
| Balanced | 1920 | 10 Mbps | configuration, default 60 |
| Sharp | 2560 | 18 Mbps | configuration, default 60 |

Aspect ratio is preserved and encoder dimensions are even. The browser requests fit by default and keeps predicted Pointer Events local so prediction cannot corrupt the remote ink stream.

All three presets are configuration data in 0.6.0: maximum width, FPS, and bitrate can be edited from Windows or the paired iPad and applied live. Each preset has an H.264 base bitrate plus optional HEVC and AV1 overrides. An unset codec override inherits the H.264 target; NFiDB does not pretend that an unmeasured lower number is automatically equal quality.

## Metrics and recording

The host and browser expose capture/encode/decode/presentation FPS; source and output dimensions; dropped, skipped, lost, and frozen frames by stage; encoded and received bandwidth; encode/decode/jitter-buffer cost; LAN RTT and clock offset; startup IDR/first-frame timing; frame-gap percentiles; DataChannel/WebSocket buffers; input rate, pressure, angles, continuity, estimated arrival age, and native injection cost. The generated test pattern provides a repeatable video path without depending on desktop activity.

The iPad sends a structured sample once per second. The host retains approximately six hours, joins each sample to host counters, and processes count/min/mean/p50/p95/p99/max distributions. A report can be reset and exported from the desktop Diagnostics page. Direct browser capture-to-presentation values are included when Safari exposes frame metadata; otherwise the report labels a component-derived pipeline estimate. Neither method measures physical Pencil-contact-to-photon latency.

## Multi-codec release measurements

The full matrix below predates the Unreleased D3D11 preprocessing change and is preserved as the CPU-preprocessing baseline. Current benchmark exports include a `memory_path` column and hardware rows exercise deterministic BGRA upload, D3D11 scale/conversion, and direct-or-assisted Media Foundation input.

The optimized 2026-08-22 GPU validation ran 30 deterministic drawing frames at 1920×1080. All three NVIDIA Media Foundation encoders used direct DXGI surfaces and reported `gpu-zero-copy`; the live WGC monitor check independently captured 184 and encoded 180 frames through the same path. Host throughput is intentionally unpaced, so values above 60 fps indicate headroom rather than transmitted frame rate. Bitrates describe this short synthetic sequence and are not equal-quality codec comparisons.

| Encoder | Memory path | Host throughput | Preprocess p95 | Encode p95 | Process CPU | Peak RAM | Actual Mbps | Auto score |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| NVIDIA H.264 | `gpu-zero-copy` | 121.05 fps | 3.48 ms | 3.71 ms | 1.18% | 103.83 MiB | 3.14 | 85.97 |
| NVIDIA HEVC | `gpu-zero-copy` | 111.98 fps | 2.89 ms | 4.30 ms | 0.91% | 127.61 MiB | 8.13 | 80.19 |
| NVIDIA AV1 | `gpu-zero-copy` | 121.68 fps | 2.87 ms | 3.36 ms | 0.40% | 137.51 MiB | 1.66 | 88.01 |
| OpenH264 | `cpu-preprocessing` | 29.30 fps | 2.26 ms | 34.96 ms | 2.72% | 151.32 MiB | 0.22 | rejected |

The 0.6.0 development host exposes functional NVIDIA Media Foundation H.264, HEVC, and AV1 encoders on an RTX 4090. The 60-frame full host matrix covered four source/output geometries and three deterministic workloads for all four usable modes. At 1080p Balanced with the drawing workload, steady encode p95 was 4.94 ms H.264 hardware, 5.54 ms HEVC hardware, 4.76 ms AV1 hardware, and 34.60 ms OpenH264. Unpaced complete host-pipeline throughput was 109.59, 102.12, 110.87, and 28.97 fps respectively. See [Codec benchmarks](CODEC_BENCHMARKS.md) for the complete method, representative tables, Auto formula, browser evidence, and limitations.

The paired packaged Edge Quick Auto Test verified actual presentation for H.264 hardware, AV1 hardware, and OpenH264. Edge did not report HEVC and it was therefore excluded rather than forced. The final run measured 56.25/53.5 encoded/presented fps for hardware H.264, 57.25/57.25 for AV1, and 16.25/16.25 for OpenH264. Edge headless also reported 47.6–50% compositor presentation drops on the hardware paths, so every observation failed at least one strict end-to-end gate and Auto retained the conservative H.264 hardware choice. No physical iPad Safari multi-codec number is claimed.

## Release-mode measurements

These profile results are from the development Windows 11 PC (Intel Core i9-13900K), release `0.2.0`, Microsoft Edge headless, a real LAN-address WebRTC connection, a generated 60 fps 4K/1080p integrity pattern, and a simultaneous 240-sample/s pen stream. They measure software encoding and end-to-end decode, not physical Pencil glass-to-glass latency.

| Source → encoded output | Profile | Host encoded fps | Mean encode | Decoded fps at end | RTP loss / decoder drops / freezes |
| --- | --- | ---: | ---: | ---: | --- |
| 3840×2160 → 1280×720 | Fast | 53.8 | 19.2 ms | 53 | 0 / 0 / 0 |
| 1920×1080 → 1920×1080 | Balanced | 32.4 | 30.8 ms | 33 | 0 / 0 / 0 |
| 3840×2160 → 1920×1080 | Balanced | 29.3 | 34.8 ms | 29 | 0 / 0 / 0 |
| 3840×2160 → 2560×1440 | Sharp | 13.8 | 65.0 ms | 15 | 0 / 0 / 0 |

All four 10-second cases used their stated receiver viewport (1920×1080 or 3840×2160), had zero integrity-marker mismatches, media-time regressions, input sequence gaps, or buffered input bytes. Fast is the recommended software profile when motion smoothness matters; Balanced favors drawing detail; Sharp is CPU-limited on the measured system.

After a physical iPad exposed a roughly 30-second first-picture delay, the startup regression was strengthened to join an encoder already emitting delta frames. The final v0.3.1 candidate run rejected one pre-IDR delta frame, received its connection-requested IDR in 65.091 ms, and rendered the first browser frame in 99.3 ms. The automated release gate requires a first browser frame within five seconds and an IDR within 2.5 seconds; the physical iPad rerun remains required before claiming those exact timings on Safari hardware.

A later physical iPad session connected its signaling and video track but remained at “waiting for first decodable frame.” After eliminating a conflicting diagnostic capture session, an isolated production run of the real 3840×2160 monitor through Balanced 1920×1080 WebRTC presented the first browser frame in 96.4 ms after a 62.549 ms host IDR wait and advanced without RTP loss, decoder drops, freezes, or transport drops. Because the intermittent Safari stall did not recur after restart, this does not prove whether its first IDR was lost, delayed, or rejected. The client now tests actual frame presentation and requests fresh IDRs over authenticated WebSocket, or DataChannel fallback, with a bounded backoff until a frame is visible. Each request is counted in host diagnostics.

The same 12.1-second v0.3.1 candidate run retained 11 synchronized diagnostic samples over 10.003 seconds with none discarded. On its low-motion 1080p Balanced pattern, p95 results were 1.999 ms LAN RTT, 0.176 Mbps receive rate, 32.145 decode/presentation FPS, 12.385 ms jitter-buffer residence per frame, 1.811 ms decode cost per frame, and 88.646 ms component-estimated pipeline delay. Edge did not expose capture-time frame metadata in that run. It delivered 2,402/2,402 simultaneous input samples; diagnostic-sample p95 input arrival was 0.390 ms and native injection was 0.001 ms. RTP loss, decoder drops, freezes, input gaps, integrity mismatches, and transport skips were all zero.

A subsequent real-iPad 4K→1080p Balanced run exposed a software-encoder feedback condition that the low-motion pattern did not: forcing a recovery IDR every wall-clock second made full keyframes dominate once real desktop content slowed OpenH264. The v0.3.1 raw report retained 111/111 visible-client samples and showed zero RTP loss, input gaps, input errors, buffered input bytes, or transport skips, but only 4.75 mean decode fps and 917 ms p95 frame gaps. This isolated the problem to encoding rather than the LAN or input path. In v0.3.2, startup still requests an immediate IDR while periodic recovery moved to five seconds. The repeated automated 4K→1080p run sustained 27.07 host encoded fps and 25.20 mean browser decode fps, used four keyframes through the measured interval, delivered 2,402/2,402 simultaneous input samples, and had zero RTP loss, freezes, decoder drops, input gaps, integrity mismatches, or transport skips. Its p95 component estimate was 112.46 ms. An initial physical v0.3.2 observation sustained about 20 encoded fps on the same real desktop and started video in 71 ms; a longer controlled hardware report remains a release-quality follow-up.

The final v0.3.3 4K→1080p regression exercised the corrected primary-tip and fit-edge lifecycle path. It delivered and injected 2,402/2,402 samples at 240.19 samples/s with exact `0–1` pressure and `±60°` tilt ranges, zero input errors, lifecycle errors, sequence gaps, reordering, RTP loss, freezes, decoder drops, buffered bytes, or transport skips. Browser decode averaged 19.19 fps and the 10-s component estimate was 179.13 ms p95 on the software encoder; first video arrived in 107 ms after a 68.86 ms host IDR wait. This is a continuity pass, not a 60 fps or physical glass-to-glass latency claim.

The v0.4.0 mixed-input candidate added mouse, wheel, keyboard, Unicode text, and semantic gesture traffic before a five-second 240 Hz Pencil stream. The 4K→720p Fast comparison delivered 1,202/1,202 sustained Pencil samples at 240.24 samples/s, then the 1080p Balanced comparison delivered the same count at 240.16 samples/s. Each host observed three mouse samples, one wheel event, four ordered Option+Tab key transitions, one 17-byte Unicode text event, and one intentional gesture command before disconnect resets. Both had zero input errors, sequence/lifecycle gaps, ordering faults, DataChannel backlog, RTP loss, decoder drops, freezes, media regressions, integrity mismatches, or video transport drops. Diagnostic-sample p95 input arrival/injection were 0.667/0.004 ms for Fast and 0.449/0.003 ms for Balanced. The component-estimated pipeline p95 values were 70.03 and 83.43 ms respectively; these are automated localhost/LAN-path measurements, not physical iPad glass-to-glass results.

The 10-minute 4K→720p Fast soak delivered 144,002 exact input samples at 240.001 samples/s, decoded 30,508 frames, advanced media time by 599.997 seconds, and reported zero RTP loss, decoder drops, freezes, transport drops, or integrity mismatches. The encoder averaged about 51 fps over the run. Edge's headless 4K compositor presented only about 18 fps and reported 40.9% presentation drops even though WebRTC decoded every received frame; this is recorded as a test-environment compositor limit, not hidden as network loss.

NFiDB therefore makes no “zero latency,” guaranteed 60/120 fps, color-accuracy, or native-Pencil-latency claim. Actual performance depends on PC CPU/GPU, source motion, Wi-Fi, iPad, codec, and drawing app. Media Foundation hardware encoding and D3D11 monitor preprocessing/surface input are implemented; diagnostics distinguish direct and assisted GPU paths from the CPU compatibility path.

## File-transfer smoke

The v0.5.1 packaged-release smoke used the normal 32 Mbps bulk limiter and temporary files outside the repository. A 3,146,237-byte iPad-shaped upload completed in 0.889 s (28.299 Mbps including four request/verification turns). Two Windows-to-iPad streams totaling 7,340,252 bytes completed in 3.511 s (16.723 Mbps), and a separate 1,024-byte range exactly matched source bytes 1,024–2,047. Every full-file SHA-256 matched. The range remained queued, both later whole-file responses auto-cleared their independent IDs, and the final Outbox was empty. Cancellation cleanup, unauthenticated rejection, and repeated create/finalize behavior also passed. Final counters reported one completed upload, three completed download responses, one intentional cancellation, and zero failed or active transfers.

These are localhost protocol/flow-control measurements from the packaged release EXE, not Wi-Fi or physical Safari throughput claims. The transfer ceiling is intentionally conservative so a bulk copy does not dominate video. Upload memory is bounded to one 1 MiB request body plus hashing buffers; download streaming uses 64 KiB blocks rather than reading an entire file into RAM. The default drawing-priority option was separately tested to keep pacing blocked during Pen Down and resume after Pen Up.

## Measurement procedure

1. Run `./scripts/benchmark.ps1 -Quick` for a representative host and Edge comparison, or `./scripts/benchmark.ps1 -Full` for all codecs, presets/geometries, and workloads. Use `-HostOnly` when no browser receiver is wanted.
2. Open the desktop Diagnostics page, reset the recording, pair a physical iPad, and enable the browser Stats panel.
3. Exercise drawing and active monitor motion for at least 60 seconds, then export the detailed JSON. Record PC GPU/CPU, iPad model, iPadOS/Safari versions, Wi-Fi band, and access-point model alongside it.
4. Repeat on an active Windows monitor and for Fast/Balanced/Sharp.
5. For glass-to-glass latency, film Pencil contact and the Windows ink response with a high-speed camera and publish the method with percentile results.
