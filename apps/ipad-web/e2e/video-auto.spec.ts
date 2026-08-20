import { expect, test } from "@playwright/test";

test("benchmarks mutually supported codecs and returns to Auto without restarting capture", async ({ page }) => {
  const pin = process.env.NFIDB_E2E_PIN;
  test.skip(!pin, "Set NFIDB_E2E_PIN when running against a live NFiDB host.");
  test.setTimeout(120_000);

  await page.goto("/");
  await page.locator("#pin").fill(pin!);
  await expect(page.locator("#connectionState")).toContainText("Connected locally", { timeout: 20_000 });
  await expect.poll(() => page.locator("video").evaluate((video) => video.videoWidth), { timeout: 20_000 })
    .toBeGreaterThan(0);

  const cleared = await page.evaluate(async () => (await fetch("/api/video/benchmark-results", { method: "DELETE" })).ok);
  expect(cleared).toBe(true);

  const captureBefore = await page.evaluate(async () => (await (await fetch("/api/metrics")).json()).capture_frames as number);
  await page.locator("#toolbarReveal").click();
  await page.locator("#videoButton").click();
  await expect(page.locator("#videoAutoTest")).toBeVisible();
  await page.locator("#videoAutoTest").click();
  await expect(page.locator(".video-benchmark p")).toContainText("Auto selected", { timeout: 75_000 });

  const control = await page.evaluate(async () => await (await fetch("/api/video")).json());
  expect(control.settings.settings.encoder).toBe("auto");
  expect(control.learned_results.length).toBeGreaterThanOrEqual(2);
  expect(control.learned_results.every((result: { end_to_end_verified: boolean }) => result.end_to_end_verified)).toBe(true);
  expect(control.learned_results.some((result: { mode: string }) => result.mode === "h264-hardware")).toBe(true);
  expect(control.learned_results.some((result: { mode: string }) => result.mode === "av1-hardware")).toBe(true);
  expect(control.runtime.restart_count).toBeGreaterThanOrEqual(control.learned_results.length);

  const captureAfter = await page.evaluate(async () => (await (await fetch("/api/metrics")).json()).capture_frames as number);
  expect(captureAfter).toBeGreaterThan(captureBefore);
  await expect(page.locator("#connectionState")).toContainText("Connected locally");
  await expect.poll(() => page.locator("video").evaluate((video) => video.currentTime), { timeout: 10_000 })
    .toBeGreaterThan(1);
});
