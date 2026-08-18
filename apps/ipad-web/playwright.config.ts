import { defineConfig } from "@playwright/test";

const [viewportWidth, viewportHeight] = (process.env.NFIDB_E2E_VIEWPORT ?? "1280x720")
  .split("x")
  .map(Number);

export default defineConfig({
  testDir: "./e2e",
  timeout: 120_000,
  fullyParallel: false,
  retries: 0,
  reporter: "line",
  use: {
    baseURL: process.env.NFIDB_E2E_URL ?? "http://127.0.0.1:47831/",
    channel: process.env.CI ? undefined : "msedge",
    headless: true,
    viewport: { width: viewportWidth || 1280, height: viewportHeight || 720 },
    trace: "retain-on-failure",
  },
});
