import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  fullyParallel: false,
  retries: 0,
  reporter: "line",
  use: {
    baseURL: process.env.NFIDB_E2E_URL ?? "http://127.0.0.1:47831/",
    channel: process.env.CI ? undefined : "msedge",
    headless: true,
    trace: "retain-on-failure",
  },
});
