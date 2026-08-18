# Performance

NFiDB favors fresh input and pictures over complete frame delivery. Both the capture-to-encoder boundary and video broadcast are bounded; an overloaded stage drops stale frames.

## Profiles

| Profile | Maximum width | Target bitrate | Target frame rate |
| --- | ---: | ---: | ---: |
| Fast | 1280 | 5 Mbps | configuration, default 60 |
| Balanced | 1920 | 10 Mbps | configuration, default 60 |
| Sharp | 2560 | 18 Mbps | configuration, default 60 |

Aspect ratio is preserved and encoder dimensions are even. The browser requests fit by default and keeps predicted Pointer Events local so prediction cannot corrupt the remote ink stream.

## Metrics and recording

The host and browser expose capture/encode/decode/presentation FPS; source and output dimensions; dropped, skipped, lost, and frozen frames by stage; encoded and received bandwidth; encode/decode/jitter-buffer cost; LAN RTT and clock offset; startup IDR/first-frame timing; frame-gap percentiles; DataChannel/WebSocket buffers; input rate, pressure, angles, continuity, estimated arrival age, and native injection cost. The generated test pattern provides a repeatable video path without depending on desktop activity.

The iPad sends a structured sample once per second. The host retains approximately six hours, joins each sample to host counters, and processes count/min/mean/p50/p95/p99/max distributions. A report can be reset and exported from the desktop Diagnostics page. Direct browser capture-to-presentation values are included when Safari exposes frame metadata; otherwise the report labels a component-derived pipeline estimate. Neither method measures physical Pencil-contact-to-photon latency.

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

The same 12.1-second v0.3.1 candidate run retained 11 synchronized diagnostic samples over 10.003 seconds with none discarded. On its low-motion 1080p Balanced pattern, p95 results were 1.999 ms LAN RTT, 0.176 Mbps receive rate, 32.145 decode/presentation FPS, 12.385 ms jitter-buffer residence per frame, 1.811 ms decode cost per frame, and 88.646 ms component-estimated pipeline delay. Edge did not expose capture-time frame metadata in that run. It delivered 2,402/2,402 simultaneous input samples; diagnostic-sample p95 input arrival was 0.390 ms and native injection was 0.001 ms. RTP loss, decoder drops, freezes, input gaps, integrity mismatches, and transport skips were all zero.

The 10-minute 4K→720p Fast soak delivered 144,002 exact input samples at 240.001 samples/s, decoded 30,508 frames, advanced media time by 599.997 seconds, and reported zero RTP loss, decoder drops, freezes, transport drops, or integrity mismatches. The encoder averaged about 51 fps over the run. Edge's headless 4K compositor presented only about 18 fps and reported 40.9% presentation drops even though WebRTC decoded every received frame; this is recorded as a test-environment compositor limit, not hidden as network loss.

NFiDB therefore makes no “zero latency,” guaranteed 60/120 fps, color-accuracy, or native-Pencil-latency claim. Actual performance depends on PC CPU, source motion, Wi-Fi, iPad, and drawing app. Moving the isolated encoder layer to Media Foundation hardware encoding remains the main performance milestone.

## Measurement procedure

1. Run `./scripts/benchmark.ps1 -DurationSeconds 10` for the four generated-pattern scenarios. Use `-ScenarioName 4k-to-720p-fast -DurationSeconds 600` for the soak.
2. Open the desktop Diagnostics page, reset the recording, pair a physical iPad, and enable the browser Stats panel.
3. Exercise drawing and active monitor motion for at least 60 seconds, then export the detailed JSON. Record PC GPU/CPU, iPad model, iPadOS/Safari versions, Wi-Fi band, and access-point model alongside it.
4. Repeat on an active Windows monitor and for Fast/Balanced/Sharp.
5. For glass-to-glass latency, film Pencil contact and the Windows ink response with a high-speed camera and publish the method with percentile results.
