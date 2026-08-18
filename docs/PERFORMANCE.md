# Performance

NFiDB favors fresh input and pictures over complete frame delivery. Both the capture-to-encoder boundary and video broadcast are bounded; an overloaded stage drops stale frames.

## Profiles

| Profile | Maximum width | Target bitrate | Target frame rate |
| --- | ---: | ---: | ---: |
| Fast | 1280 | 5 Mbps | configuration, default 60 |
| Balanced | 1920 | 10 Mbps | configuration, default 60 |
| Sharp | 2560 | 18 Mbps | configuration, default 60 |

Aspect ratio is preserved and encoder dimensions are even. The browser requests fit by default and keeps predicted Pointer Events local so prediction cannot corrupt the remote ink stream.

## Metrics

The host and browser expose capture FPS, encoded FPS, dropped frames, encoded bytes, encode time, input samples per second, pressure/tilt, and LAN ping RTT. The generated test pattern provides a repeatable video path without depending on desktop activity.

## Release-mode measurements

These results are from the development Windows 11 PC (Intel Core i9-13900K), release `0.2.0`, Microsoft Edge headless, a real LAN-address WebRTC connection, a generated 60 fps 4K/1080p integrity pattern, and a simultaneous 240-sample/s pen stream. They measure software encoding and end-to-end decode, not physical Pencil glass-to-glass latency.

| Source → encoded output | Profile | Host encoded fps | Mean encode | Decoded fps at end | RTP loss / decoder drops / freezes |
| --- | --- | ---: | ---: | ---: | --- |
| 3840×2160 → 1280×720 | Fast | 53.8 | 19.2 ms | 53 | 0 / 0 / 0 |
| 1920×1080 → 1920×1080 | Balanced | 32.4 | 30.8 ms | 33 | 0 / 0 / 0 |
| 3840×2160 → 1920×1080 | Balanced | 29.3 | 34.8 ms | 29 | 0 / 0 / 0 |
| 3840×2160 → 2560×1440 | Sharp | 13.8 | 65.0 ms | 15 | 0 / 0 / 0 |

All four 10-second cases used their stated receiver viewport (1920×1080 or 3840×2160), had zero integrity-marker mismatches, media-time regressions, input sequence gaps, or buffered input bytes. Fast is the recommended software profile when motion smoothness matters; Balanced favors drawing detail; Sharp is CPU-limited on the measured system.

The 10-minute 4K→720p Fast soak delivered 144,002 exact input samples at 240.001 samples/s, decoded 30,508 frames, advanced media time by 599.997 seconds, and reported zero RTP loss, decoder drops, freezes, transport drops, or integrity mismatches. The encoder averaged about 51 fps over the run. Edge's headless 4K compositor presented only about 18 fps and reported 40.9% presentation drops even though WebRTC decoded every received frame; this is recorded as a test-environment compositor limit, not hidden as network loss.

NFiDB therefore makes no “zero latency,” guaranteed 60/120 fps, color-accuracy, or native-Pencil-latency claim. Actual performance depends on PC CPU, source motion, Wi-Fi, iPad, and drawing app. Moving the isolated encoder layer to Media Foundation hardware encoding remains the main performance milestone.

## Measurement procedure

1. Run `./scripts/benchmark.ps1 -DurationSeconds 10` for the four generated-pattern scenarios. Use `-ScenarioName 4k-to-720p-fast -DurationSeconds 600` for the soak.
2. Pair a physical iPad and enable the browser Stats panel for hardware validation.
3. Record capture/encode/presentation FPS, preprocessing and encode time, all three drop counters, RTT, resolution, PC GPU/CPU, iPad model, iPadOS/Safari versions, Wi-Fi band, and access-point model.
4. Repeat on an active Windows monitor and for Fast/Balanced/Sharp.
5. For glass-to-glass latency, film Pencil contact and the Windows ink response with a high-speed camera and publish the method with percentile results.
