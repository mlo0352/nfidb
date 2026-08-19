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

test("starts and advances a real monitor stream and honors a fresh-IDR request", async ({ page }) => {
  const pin = process.env.NFIDB_E2E_PIN;
  test.skip(!pin, "Set NFIDB_E2E_PIN when running against a live NFiDB host.");
  test.setTimeout(90_000);

  await page.goto("/");
  await page.locator("#pin").fill(pin!);
  await expect(page.locator("#surface")).toBeVisible();
  await expect(page.locator("#connectionState")).toContainText("Connected locally", { timeout: 20_000 });
  await expect.poll(() => page.locator("video").evaluate((video) => video.videoWidth), { timeout: 30_000 }).toBeGreaterThan(0);

  const before = await snapshot(page);
  expect(before.connectionState).toBe("connected");
  expect(before.peerConnectionState).toBe("connected");
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

  await page.waitForTimeout(2_500);
  const after = await snapshot(page);
  expect(after.video.currentTime).toBeGreaterThan(before.video.currentTime + 1.5);
  expect(after.video.totalFrames).toBeGreaterThan(before.video.totalFrames);
  expect(Number(after.host.capture_frames)).toBeGreaterThan(Number(before.host.capture_frames));
  expect(Number(after.host.encoded_frames)).toBeGreaterThan(Number(before.host.encoded_frames));
  expect(Number(after.host.encoded_keyframes)).toBeGreaterThan(Number(before.host.encoded_keyframes));
  expect(Number(after.host.video_transport_drops)).toBe(0);
  expect(Number(after.inboundVideo?.packetsLost ?? 0)).toBe(0);
  expect(Number(after.inboundVideo?.framesDecoded ?? 0)).toBeGreaterThan(0);
  expect(Number(after.inboundVideo?.framesDropped ?? 0)).toBe(0);
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
