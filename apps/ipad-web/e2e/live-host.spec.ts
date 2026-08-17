import { expect, test } from "@playwright/test";

test("pairs with the live host, receives video, and sends pointer input", async ({ page }) => {
  const pin = process.env.NFIDB_E2E_PIN;
  test.skip(!pin, "Set NFIDB_E2E_PIN when running against a live NFiDB host.");

  await page.goto("/");
  await page.locator("#pin").fill(pin!);
  await page.getByRole("button", { name: "Connect" }).click();

  await expect(page.locator("#surface")).toBeVisible();
  await expect(page.locator("#connectionState")).toContainText("Connected locally", {
    timeout: 20_000,
  });
  await expect
    .poll(() => page.locator("video").evaluate((video) => video.videoWidth), {
      timeout: 20_000,
    })
    .toBeGreaterThan(0);

  const overlay = page.locator("#interactionOverlay");
  await overlay.dispatchEvent("pointerdown", {
    pointerId: 17,
    pointerType: "pen",
    clientX: 240,
    clientY: 180,
    pressure: 0.2,
    tiltX: -12,
    tiltY: 18,
    buttons: 1,
  });
  await overlay.dispatchEvent("pointermove", {
    pointerId: 17,
    pointerType: "pen",
    clientX: 320,
    clientY: 230,
    pressure: 0.75,
    tiltX: 20,
    tiltY: -8,
    buttons: 1,
  });
  await overlay.dispatchEvent("pointerup", {
    pointerId: 17,
    pointerType: "pen",
    clientX: 360,
    clientY: 260,
    pressure: 0,
    tiltX: 20,
    tiltY: -8,
    buttons: 0,
  });
});
