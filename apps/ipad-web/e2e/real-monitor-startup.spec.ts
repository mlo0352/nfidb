import { expect, test } from "@playwright/test";
import { writeFileSync } from "node:fs";

interface DiagnosticSnapshot {
  connectionState: string;
  peerConnectionState: string;
  inputTransport: string;
  video: { width: number; height: number; readyState: number; currentTime: number; totalFrames: number; droppedFrames: number; startupMs: number | null };
  inboundVideo: Record<string, number> | null;
  host: Record<string, number | boolean>;
}

test("starts and advances video, carries mouse input on DataChannel, and honors a fresh-IDR request", async ({ page }) => {
  const pin = process.env.NFIDB_E2E_PIN;
  test.skip(!pin, "Set NFIDB_E2E_PIN when running against a live NFiDB host.");
  test.setTimeout(90_000);

  await page.goto("/");
  await page.locator("#pin").fill(pin!);
  await expect(page.locator("#surface")).toBeVisible();
  await expect(page.locator("#connectionState")).toContainText("Connected locally", { timeout: 20_000 });

  // Exercise the real rendered hit targets, not just the class-changing unit
  // methods. A release is unusable on iPad if the Controls button cannot
  // recover the toolbar after it has been dismissed.
  await page.locator("#controlsClose").click();
  await expect(page.locator("#toolbar")).not.toHaveClass(/visible/);
  await expect(page.locator("#toolbarReveal")).toBeVisible();
  await page.locator("#toolbarReveal").tap();
  await expect(page.locator("#toolbar")).toHaveClass(/visible/);
  await expect(page.locator("#touchButton")).toBeVisible();
  const layout = await page.evaluate(() => {
    const surface = document.querySelector<HTMLElement>("#surface")!.getBoundingClientRect();
    const video = document.querySelector<HTMLVideoElement>("#remoteVideo")!;
    const videoBox = video.getBoundingClientRect();
    const toolbar = document.querySelector<HTMLElement>("#toolbar")!.getBoundingClientRect();
    const style = getComputedStyle(video);
    return {
      surface: { top: surface.top, bottom: surface.bottom, height: surface.height },
      video: { top: videoBox.top, bottom: videoBox.bottom, height: videoBox.height },
      toolbar: { top: toolbar.top, bottom: toolbar.bottom },
      objectFit: style.objectFit,
      objectPosition: style.objectPosition,
    };
  });
  expect(layout.video.top).toBeCloseTo(layout.surface.top, 0);
  expect(layout.video.bottom).toBeCloseTo(layout.surface.bottom, 0);
  expect(layout.video.height).toBeCloseTo(layout.surface.height, 0);
  expect(layout.objectFit).toBe("contain");
  expect(["center", "50% 50%", "center center"]).toContain(layout.objectPosition);
  expect(layout.toolbar.top).toBeGreaterThanOrEqual(layout.surface.top);
  expect(layout.toolbar.bottom).toBeLessThanOrEqual(layout.surface.bottom);

  await expect.poll(() => page.locator("video").evaluate((video) => video.videoWidth), { timeout: 30_000 }).toBeGreaterThan(0);

  const transportBefore = await snapshot(page);
  const overlay = await page.locator("#interactionOverlay").boundingBox();
  expect(overlay).not.toBeNull();
  await page.mouse.move(overlay!.x + overlay!.width / 2, overlay!.y + overlay!.height / 2);
  await expect.poll(async () => (await snapshot(page)).inputTransport, { timeout: 10_000 }).toBe("datachannel");
  await expect.poll(async () => Number((await snapshot(page)).host.mouse_samples), { timeout: 10_000 })
    .toBeGreaterThan(Number(transportBefore.host.mouse_samples));

  const before = await snapshot(page);
  expect(before.connectionState).toBe("connected");
  expect(before.peerConnectionState).toBe("connected");
  expect(before.inputTransport).toBe("datachannel");
  expect(before.video.readyState).toBeGreaterThanOrEqual(2);
  expect(before.video.startupMs).not.toBeNull();
  expect(Number(before.video.startupMs)).toBeLessThan(5_000);
  expect(Number(before.host.video_startup_wait_ms)).toBeLessThan(2_500);
  expect(Number(before.host.capture_frames)).toBeGreaterThan(0);
  expect(Number(before.host.encoded_frames)).toBeGreaterThan(0);

  const recoveryBefore = Number(before.host.video_recovery_requests);
  await requestKeyframe(page);
  await expect.poll(async () => Number((await snapshot(page)).host.video_recovery_requests), { timeout: 10_000 })
    .toBe(recoveryBefore + 1);

  if (process.env.NFIDB_E2E_DRIVE_POINTER === "1") {
    for (let index = 0; index < 14; index += 1) {
      const x = overlay!.x + overlay!.width * (0.2 + (index % 7) * 0.1);
      const y = overlay!.y + overlay!.height * (index % 2 === 0 ? 0.35 : 0.65);
      await page.mouse.move(x, y);
      await page.waitForTimeout(200);
    }
  } else {
    await page.waitForTimeout(2_500);
  }
  const after = await snapshot(page);
  expect(after.video.currentTime).toBeGreaterThan(before.video.currentTime + 1.5);
  expect(after.video.totalFrames).toBeGreaterThan(before.video.totalFrames);
  expect(Number(after.host.capture_frames)).toBeGreaterThan(Number(before.host.capture_frames));
  expect(Number(after.host.encoded_frames)).toBeGreaterThan(Number(before.host.encoded_frames));
  expect(Number(after.host.encoded_keyframes)).toBeGreaterThan(Number(before.host.encoded_keyframes));
  expect(Number(after.host.video_transport_drops)).toBe(0);
  expect(after.inputTransport).toBe("datachannel");
  expect(Number(after.inboundVideo?.packetsLost ?? 0)).toBe(0);
  expect(Number(after.inboundVideo?.framesDecoded ?? 0)).toBeGreaterThan(0);
  expect(
    Number(after.inboundVideo?.framesDropped ?? 0) - Number(before.inboundVideo?.framesDropped ?? 0),
  ).toBeLessThanOrEqual(1);
  expect(Number(after.inboundVideo?.freezeCount ?? 0)).toBe(0);

  const expectedWidth = Number(process.env.NFIDB_E2E_EXPECT_WIDTH ?? 0);
  const expectedHeight = Number(process.env.NFIDB_E2E_EXPECT_HEIGHT ?? 0);
  if (expectedWidth > 0) expect(after.video.width).toBe(expectedWidth);
  if (expectedHeight > 0) expect(after.video.height).toBe(expectedHeight);

  const report = { before, after };
  console.log(JSON.stringify(report, null, 2));
  if (process.env.NFIDB_E2E_REPORT) {
    writeFileSync(process.env.NFIDB_E2E_REPORT, JSON.stringify(report, null, 2));
  }
});

async function snapshot(page: import("@playwright/test").Page): Promise<DiagnosticSnapshot> {
  return page.evaluate(() =>
    (window as Window & { __nfidbDiagnostics: () => Promise<DiagnosticSnapshot> }).__nfidbDiagnostics(),
  );
}

async function requestKeyframe(page: import("@playwright/test").Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolve, reject) => {
        const protocol = location.protocol === "https:" ? "wss:" : "ws:";
        const socket = new WebSocket(`${protocol}//${location.host}/api/ws`);
        socket.addEventListener(
          "open",
          () => {
            socket.send(JSON.stringify({ type: "request-keyframe" }));
            window.setTimeout(() => {
              socket.close();
              resolve();
            }, 100);
          },
          { once: true },
        );
        socket.addEventListener("error", () => reject(new Error("Recovery WebSocket failed to open")), { once: true });
      }),
  );
}
