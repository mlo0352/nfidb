# Codec benchmarks

NFiDB has two deliberately separate benchmark levels.

- The host benchmark needs no receiver. It renders deterministic screen-detail, drawing, and high-motion frames; performs the same CPU resize/color conversion as live capture; feeds each functional encoder; and records throughput, bytes, startup, preprocess/encode distributions, process CPU, working set, and Auto score.
- Quick Auto Test runs from a paired browser. It switches only among mutually supported modes, waits for a presented frame, samples the real capture/WebRTC/decode/presentation path for four seconds, stores the observation locally, and returns the host to Auto. Desktop browser automation is always labeled as Edge, never iPad Safari.

`scripts/benchmark.ps1` exports `environment.json`, `capabilities.json`, `results.json`, `results.csv`, and `summary.md` for host runs. Receiver runs add raw, CSV, and Markdown Edge reports. `build/benchmarks/latest.json` remains the machine-readable latest host result.

## Auto gates and score

For a candidate targeting `F` frames per second, Auto rejects it before scoring when any of these occurs:

- presented FPS (encoded FPS when no receiver measurement exists) is below `0.92 × F`;
- encode p95 exceeds `0.72 × (1000 / F)` milliseconds;
- measured drop rate exceeds 5%;
- any presentation freeze is observed.

Survivors receive up to 100 points: encode latency 35, frame stability 25, bandwidth 18, process CPU 10, working set 6, and objective quality 6. Missing CPU, memory, or quality data receives a documented neutral component rather than a fabricated measurement. Codec age is not a score input. AV1 is not provisionally preferred until an end-to-end result passes; HEVC and hardware H.264 are the conservative hardware candidates, with OpenH264 as the universal fallback.

Learned results stay in `%APPDATA%\NFiDB\codec-benchmarks.json`. The cache key includes NFiDB version, receiver user agent/runtime, exact encoder identity, profile, width, and FPS. A different driver-exposed encoder identity or runtime therefore requires new evidence. **Clear learned results** and **Run quick test** are available in the UI.

## Development machine

Measured 2026-08-19 in optimized mode:

- Windows 11 Pro x64 build 22631
- Intel Core i9-13900K
- NVIDIA GeForce RTX 4090
- NVIDIA display driver 32.0.15.6094, dated 2024-08-13 (collected from Windows for the report; the current MFT capability JSON leaves driver version unavailable)
- Media Foundation MFTs: NVIDIA H.264 Encoder MFT, NVIDIA HEVC Encoder MFT, NVIDIA AV1 Encoder MFT
- Software fallback: Cisco OpenH264 0.9.8 crate binding
- Receiver automation: Headless Microsoft Edge 151 on the local authenticated WebRTC path

Windows also enumerated a Microsoft `H264 Encoder MFT`, but it was only initializeable in this run and was not promoted to functional because it was not the transform that returned the encoded probe sample.

## Full host benchmark

These are 60-frame drawing-workload rows from the first optimized 48-row run. FPS is unpaced end-to-end host throughput, including deterministic rendering, resize, color conversion, and encode. CPU is normalized to total logical processor capacity, matching a whole-process system percentage. Bitrate reflects actual bytes produced for this synthetic content and is not a quality-equivalent codec comparison. Startup outliers remain in the mean/max while p95 describes steady frames.

| Source → output | Encoder | Throughput | Encode p95 | Peak RAM | Actual Mbps | Auto score |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 4K → 720p | H.264 hardware | 68.79 fps | 3.03 ms | 159.89 MiB | 0.99 | 89.27 |
| 4K → 720p | HEVC hardware | 66.75 fps | 3.29 ms | 167.93 MiB | 1.97 | 87.81 |
| 4K → 720p | AV1 hardware | 68.09 fps | 3.33 ms | 172.87 MiB | 1.13 | 88.48 |
| 4K → 720p | H.264 software | 41.22 fps | 19.38 ms | 175.01 MiB | 0.23 | rejected |
| 1080p → 1080p | H.264 hardware | 109.59 fps | 4.94 ms | 132.12 MiB | 1.59 | 84.81 |
| 1080p → 1080p | HEVC hardware | 102.12 fps | 5.54 ms | 135.41 MiB | 6.49 | 79.13 |
| 1080p → 1080p | AV1 hardware | 110.87 fps | 4.76 ms | 137.44 MiB | 0.91 | 85.79 |
| 1080p → 1080p | H.264 software | 28.97 fps | 34.60 ms | 160.50 MiB | 0.16 | rejected |
| 4K → 1080p | H.264 hardware | 55.16 fps | 4.86 ms | 203.60 MiB | 1.22 | rejected |
| 4K → 1080p | HEVC hardware | 52.59 fps | 5.68 ms | 208.29 MiB | 7.84 | rejected |
| 4K → 1080p | AV1 hardware | 55.22 fps | 4.89 ms | 209.21 MiB | 0.82 | 83.38 |
| 4K → 1080p | H.264 software | 23.46 fps | 44.37 ms | 229.96 MiB | 0.24 | rejected |
| 4K → 1440p | H.264 hardware | 33.93 fps | 10.66 ms | 183.29 MiB | 1.03 | rejected |
| 4K → 1440p | HEVC hardware | 27.50 fps | 17.79 ms | 192.17 MiB | 1.44 | rejected |
| 4K → 1440p | AV1 hardware | 27.68 fps | 15.66 ms | 212.38 MiB | 0.56 | rejected |
| 4K → 1440p | H.264 software | 9.68 fps | 88.77 ms | 255.77 MiB | 0.07 | rejected |

The high-motion workload is intentionally harsher: it changes most pixels every frame and exposes both CPU pattern-generation/preprocessing limits and rate-control behavior. The complete JSON/CSV report, rather than this selected table, is authoritative.

The exact packaged 0.6.0 full-script repeat completed another 48/48 rows while the workstation had roughly 5–8% unrelated background CPU activity. It exposed material host-run variance: drawing throughput ranged from 48.87/45.82/45.20 fps for H.264/HEVC/AV1 at 4K→720p, 58.82/55.83/63.26 at native 1080p, 37.85/37.10/38.06 at 4K→1080p, and 29.68/28.01/29.57 at 4K→1440p. The same final binary's isolated Quick host test immediately afterward measured 1080p encode p95 of 4.91 ms H.264, 5.71 ms HEVC, 5.54 ms AV1, and 57.20 ms OpenH264. NFiDB exports raw per-case results so this variability is visible; Auto ultimately trusts the paired receiver test, not a single favorable host row.

## End-to-end Edge evidence

The exact packaged live Quick Auto Test at 1920×1080/60 verified presented video for H.264 hardware, AV1 hardware, and H.264 software. Edge did not report H.265/HEVC receiver capability, so HEVC was not offered to that browser. It measured H.264 hardware at 56.25 encoded/53.5 presented fps, AV1 hardware at 57.25/57.25, and OpenH264 at 16.25/16.25 in that run, with zero test-window freezes. Headless Edge's compositor reported 47.6–50% presentation drops for the hardware paths, so the strict reliability gate rejected them and Auto conservatively chose hardware H.264. This is a test-environment result, not evidence that a physical iPad drops half its frames.

The separately exported five-second Edge comparison measured H.264 hardware at 58.03 encoded fps, 6.24 ms encode mean/9.33 ms p95, 0.29% process CPU, 90.46 MiB working set, 1.04 Mbps receive p95, 88.06 ms component pipeline p95, zero RTP loss/decoder drops/freezes, and 1,202/1,202 input samples. AV1 measured 57.07 fps, 5.92/7.54 ms encode, 0.49% CPU, 95.46 MiB, 0.64 Mbps receive p95, 62.96 ms component pipeline p95, and the same zero-loss/input-continuity result. OpenH264 measured 18.83 fps, 39.66/52.18 ms encode, 1.66% CPU, 93.42 MiB, 0.23 Mbps receive p95, and 123.62 ms component pipeline p95. Headless compositor callback rates were only 16.0–17.8 fps despite the hardware decoders receiving 57–58 fps; this is why decode and presentation are kept separate.

Earlier five-second codec runs carried 1,202/1,202 synthetic 240 Hz Pencil samples with zero sequence/lifecycle errors while H.264 hardware and AV1 each decoded more than 300 frames, with zero RTP packet loss. AV1 successfully negotiated profile 0 in Edge. HEVC was correctly unavailable to Edge despite a functional host encoder.

## Before and after

The preserved OpenH264 release `0.2.0` end-to-end baseline was 53.8 encoded fps at 4K→720p, 32.4 at 1080p→1080p, 29.3 at 4K→1080p, and 13.8 at 4K→1440p. Its mean encode times were 19.2, 30.8, 34.8, and 65.0 ms respectively. The new deterministic run is not numerically identical to that older live WebRTC method, but it shows the intended architectural change clearly: at 1080p the current hardware encoders have about 4.8–5.5 ms steady encode p95 while OpenH264 measures 34.6 ms and misses the frame-rate gate.

## Honest limitations

- No physical iPad Safari codec benchmark was collected for 0.6.0. Previous physical-iPad H.264/input checks remain valid but are not relabeled as HEVC or AV1 evidence.
- Objective decoded-frame PSNR/SSIM is not yet implemented, so quality is unavailable. The low synthetic bitrates must not be read as equal-quality comparisons.
- Safari/Edge may omit capture-time frame metadata. In that case the report records a component-derived pipeline estimate, not glass-to-glass latency.
- The active hardware pipeline still performs CPU preprocessing and memory copies. GPU encode acceleration is real; GPU zero-copy is not claimed.

## Primary references

- [Microsoft: MFTEnumEx](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mftenumex) documents category/type filtering, hardware enumeration flags, activation objects, and `ActivateObject`.
- [Microsoft: Hardware MFTs](https://learn.microsoft.com/en-us/windows/win32/medfound/hardware-mfts) documents the asynchronous hardware-transform model and the separation of encode from video processing.
- [Microsoft: CODECAPI_AVLowLatencyMode](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avlowlatencymode) describes real-time low-latency operation and the no-reordering expectation.
- [W3C WebRTC](https://www.w3.org/TR/webrtc/) defines `RTCRtpReceiver.getCapabilities()` as an optimistic receive-capability view and defines `setCodecPreferences()` negotiation behavior. That optimistic wording is why NFiDB requires SDP, negotiation, keyframe, and presentation evidence separately.
- [WebKit: Safari 18 beta WebRTC HEVC](https://webkit.org/blog/15443/news-from-wwdc24-webkit-in-safari-18-beta/) documents RFC 7789 HEVC RTP support. [WebKit: Safari 18.4 media formats](https://webkit.org/blog/16574/webkit-features-in-safari-18-4/) documents H.264, HEVC, and hardware-dependent AV1 video-track support. NFiDB still trusts the actual browser report and playback test rather than the platform version alone.
