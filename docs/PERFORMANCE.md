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

## Current evidence

On the development Windows 11 machine, a debug build's 1280×720 software OpenH264 test pattern reached Safari-compatible H.264 through the real WebRTC path. Debug encode time was roughly 120 ms and therefore intentionally failed the 60 FPS target. A separate 3840×2160 WGC source smoke test captured at 28 FPS while the downscaled software encoder produced 3 FPS at 257.2 ms/frame and dropped 27 stale frames. Release performance and physical iPad glass-to-glass latency remain to be measured.

This is why the MVP makes no “zero latency,” 60 FPS, 120 FPS, color-accuracy, or native-Pencil-latency claim. Moving the isolated encoder layer to Media Foundation hardware encoding is the main performance milestone.

## Measurement procedure

1. Run `nfidb --capture test-pattern --input-sink log --diagnostics` from a console/debug build.
2. Pair the target iPad and enable the browser Stats panel.
3. Record capture/encode FPS, encode time, dropped frames, RTT, resolution, PC GPU/CPU, iPad model, iPadOS/Safari versions, Wi-Fi band, and access-point model.
4. Repeat on an active Windows monitor and for Fast/Balanced/Sharp.
5. For glass-to-glass latency, film Pencil contact and the Windows ink response with a high-speed camera and publish the method with percentile results.
