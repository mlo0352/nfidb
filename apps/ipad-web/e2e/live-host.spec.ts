import { expect, test } from "@playwright/test";
import { writeFileSync } from "node:fs";

interface DiagnosticSnapshot {
  connectionState: string;
  peerConnectionState: string;
  inputTransport: string;
  dataChannelBufferedBytes: number;
  webSocketBufferedBytes: number;
  video: { width: number; height: number; currentTime: number; totalFrames: number; droppedFrames: number; startupMs: number | null };
  inboundVideo: Record<string, number> | null;
  candidatePair: Record<string, unknown> | null;
  host: Record<string, number | boolean>;
  hostDiagnostics: { sample_count: number; retained_seconds: number; discarded_samples: number };
}

test("sustains coalesced pressure/tilt input while receiving integrity-checked H.264", async ({ page }) => {
  const pin = process.env.NFIDB_E2E_PIN;
  test.skip(!pin, "Set NFIDB_E2E_PIN when running against a live NFiDB host.");
  const durationMs = Number(process.env.NFIDB_E2E_DURATION_MS ?? 10_000);
  test.setTimeout(Math.max(120_000, durationMs + 90_000));
  const eventsPerSecond = 60;
  const samplesPerEvent = 4;
  const moveEvents = Math.round((durationMs / 1000) * eventsPerSecond);
  const expectedSamples = moveEvents * samplesPerEvent + 2;

  await page.goto("/");
  await page.locator("#pin").fill(pin!);

  await expect(page.locator("#surface")).toBeVisible();
  await expect(page.locator("#connectionState")).toContainText("Connected locally", { timeout: 20_000 });
  await expect.poll(() => page.locator("video").evaluate((video) => video.videoWidth), { timeout: 30_000 }).toBeGreaterThan(0);

  const before = await snapshot(page);
  expect(before.peerConnectionState).toBe("connected");
  expect(before.video.startupMs).not.toBeNull();
  expect(Number(before.video.startupMs)).toBeLessThan(5_000);
  expect(Number(before.host.video_startup_wait_ms)).toBeLessThan(2_500);
  const result = await page.evaluate(
    async ({ durationMs, eventsPerSecond, samplesPerEvent, moveEvents }) => {
      const overlay = document.querySelector<HTMLCanvasElement>("#interactionOverlay")!;
      const video = document.querySelector<HTMLVideoElement>("#remoteVideo")!;
      const bounds = overlay.getBoundingClientRect();
      const makeEvent = (
        type: string,
        index: number,
        pressure: number,
        tiltX: number,
        tiltY: number,
        buttons: number,
      ) =>
        new PointerEvent(type, {
          bubbles: true,
          cancelable: true,
          pointerId: 73,
          pointerType: "pen",
          isPrimary: true,
          buttons,
          pressure,
          tiltX,
          tiltY,
          twist: index % 360,
          clientX: bounds.left + bounds.width * (0.1 + 0.8 * (index / Math.max(1, moveEvents * samplesPerEvent))),
          clientY: bounds.top + bounds.height * (0.5 + 0.3 * Math.sin(index / 50)),
        });

      const videoFrameTimes: number[] = [];
      let integrityChecks = 0;
      let integrityMismatches = 0;
      let firstIntegrityMismatch: { top: number[]; bottom: number[] } | null = null;
      let mediaTimeRegressions = 0;
      let lastMediaTime = -1;
      const markerCanvas = document.createElement("canvas");
      markerCanvas.width = 640;
      markerCanvas.height = 360;
      const markerContext = markerCanvas.getContext("2d", { willReadFrequently: true })!;
      let videoFrameRequest = 0;
      const callback = (_now: DOMHighResTimeStamp, metadata: VideoFrameCallbackMetadata) => {
        videoFrameTimes.push(performance.now());
        if (metadata.mediaTime < lastMediaTime) mediaTimeRegressions += 1;
        lastMediaTime = metadata.mediaTime;
        if (video.videoWidth > 0 && video.videoHeight > 0 && videoFrameTimes.length % 15 === 1) {
          markerContext.drawImage(video, 0, 0, markerCanvas.width, markerCanvas.height);
          const pixels = markerContext.getImageData(0, 0, markerCanvas.width, markerCanvas.height).data;
          const luminance = (x: number, y: number) => {
            const offset = (Math.floor(y) * markerCanvas.width + Math.floor(x)) * 4;
            return pixels[offset]! + pixels[offset + 1]! + pixels[offset + 2]!;
          };
          const top = Array.from({ length: 16 }, (_, bit) =>
            luminance(((1.5 + bit) / 64) * markerCanvas.width, (1.5 / 36) * markerCanvas.height),
          );
          const bottom = Array.from({ length: 16 }, (_, bit) =>
            luminance((1 - (1.5 + bit) / 64) * markerCanvas.width, (1 - 1.5 / 36) * markerCanvas.height),
          );
          let mismatch = false;
          for (let bit = 0; bit < 16; bit += 1) {
            if ((top[bit]! >= 384) !== (bottom[bit]! >= 384)) mismatch = true;
          }
          integrityChecks += 1;
          if (mismatch) {
            integrityMismatches += 1;
            if (!firstIntegrityMismatch) {
              firstIntegrityMismatch = { top, bottom };
            }
          }
        }
        videoFrameRequest = video.requestVideoFrameCallback(callback);
      };
      videoFrameRequest = video.requestVideoFrameCallback(callback);

      overlay.dispatchEvent(makeEvent("pointerdown", 0, 0.05, 0, 60, 1));
      const started = performance.now();
      let eventIndex = 0;
      while (eventIndex < moveEvents) {
        // Hidden/headless browsers may clamp timers to roughly 30 ms. Derive the
        // amount of work from elapsed time and catch up in small bursts so the
        // generated Pencil rate remains 60 events / 240 samples per second.
        const elapsed = performance.now() - started;
        const due = Math.min(moveEvents, Math.floor((elapsed * eventsPerSecond) / 1000));
        if (eventIndex >= due) {
          await new Promise((resolve) => window.setTimeout(resolve, 4));
          continue;
        }
        const coalesced = Array.from({ length: samplesPerEvent }, (_, offset) => {
          const index = eventIndex * samplesPerEvent + offset + 1;
          const phase = (index / (moveEvents * samplesPerEvent)) * Math.PI * 24;
          return makeEvent(
            "pointermove",
            index,
            0.05 + 0.95 * (0.5 + 0.5 * Math.sin(phase)),
            60 * Math.sin(phase),
            60 * Math.cos(phase),
            1,
          );
        });
        const parent = makeEvent("pointermove", eventIndex * samplesPerEvent + samplesPerEvent, 0.5, 0, 0, 1);
        Object.defineProperty(parent, "getCoalescedEvents", { value: () => coalesced });
        overlay.dispatchEvent(parent);
        eventIndex += 1;
      }
      overlay.dispatchEvent(makeEvent("pointerup", moveEvents * samplesPerEvent + 1, 0, 0, 0, 0));
      video.cancelVideoFrameCallback(videoFrameRequest);
      const frameGaps = videoFrameTimes.slice(1).map((time, index) => time - videoFrameTimes[index]!);
      return {
        elapsedMs: performance.now() - started,
        videoCallbacks: videoFrameTimes.length,
        maxVideoFrameGapMs: Math.max(0, ...frameGaps),
        integrityChecks,
        integrityMismatches,
        firstIntegrityMismatch,
        mediaTimeRegressions,
      };
    },
    { durationMs, eventsPerSecond, samplesPerEvent, moveEvents },
  );

  await expect.poll(async () => (await snapshot(page)).host.input_samples, { timeout: 15_000 }).toBe(
    Number(before.host.input_samples) + expectedSamples,
  );
  const after = await snapshot(page);
  const playbackFrames = after.video.totalFrames - before.video.totalFrames;
  const playbackDrops = after.video.droppedFrames - before.video.droppedFrames;
  Object.assign(result, {
    presentationFramesPerSecond: result.videoCallbacks / (result.elapsedMs / 1000),
    playbackDropRate: playbackDrops / Math.max(1, playbackFrames),
  });
  const report = { expectedSamples, inputRate: expectedSamples / (result.elapsedMs / 1000), result, before, after };
  console.log(JSON.stringify(report, null, 2));
  if (process.env.NFIDB_E2E_REPORT) {
    writeFileSync(process.env.NFIDB_E2E_REPORT, JSON.stringify(report, null, 2));
  }
  expect(Number(after.host.injected_samples) - Number(before.host.injected_samples)).toBe(expectedSamples);
  expect(after.host.input_samples).toBe(after.host.injected_samples);
  expect(after.host.batch_sequence_gaps).toBe(0);
  expect(after.host.sample_sequence_gaps).toBe(0);
  expect(after.host.out_of_order_batches).toBe(0);
  expect(after.host.out_of_order_samples).toBe(0);
  expect(after.host.lifecycle_errors).toBe(0);
  expect(after.host.input_errors).toBe(0);
  expect(after.host.active_pointers).toBe(0);
  expect(after.host.pressure_min).toBe(0);
  expect(Number(after.host.pressure_max)).toBeGreaterThan(0.99);
  expect(Number(after.host.tilt_x_min)).toBeLessThan(-59);
  expect(Number(after.host.tilt_x_max)).toBeGreaterThan(59);
  expect(Number(after.host.tilt_y_min)).toBeLessThan(-59);
  expect(Number(after.host.tilt_y_max)).toBeGreaterThan(59);

  expect(after.inputTransport).toBe("datachannel");
  expect(after.dataChannelBufferedBytes).toBe(0);
  expect(after.webSocketBufferedBytes).toBe(0);
  expect(after.hostDiagnostics.sample_count).toBeGreaterThan(5);
  expect(after.hostDiagnostics.discarded_samples).toBe(0);
  expect(Number(after.inboundVideo?.packetsLost ?? 0)).toBe(0);
  expect(Number(after.inboundVideo?.framesDecoded ?? 0)).toBeGreaterThan(0);
  expect(result.videoCallbacks).toBeGreaterThan(Math.max(2, durationMs / 1000));
  expect(result.maxVideoFrameGapMs).toBeLessThan(2_500);
  expect(result.mediaTimeRegressions).toBe(0);
  expect(result.integrityChecks).toBeGreaterThan(0);
  expect(result.integrityMismatches).toBe(0);
  expect(after.video.currentTime).toBeGreaterThan(before.video.currentTime + durationMs / 1000 - 2.5);

  const expectedWidth = Number(process.env.NFIDB_E2E_EXPECT_WIDTH ?? 0);
  const expectedHeight = Number(process.env.NFIDB_E2E_EXPECT_HEIGHT ?? 0);
  if (expectedWidth > 0) expect(after.video.width).toBe(expectedWidth);
  if (expectedHeight > 0) expect(after.video.height).toBe(expectedHeight);
});

async function snapshot(page: import("@playwright/test").Page): Promise<DiagnosticSnapshot> {
  return page.evaluate(() =>
    (window as Window & { __nfidbDiagnostics: () => Promise<DiagnosticSnapshot> }).__nfidbDiagnostics(),
  );
}
