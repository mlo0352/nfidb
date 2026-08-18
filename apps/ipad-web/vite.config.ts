import { defineConfig } from "vite";
import packageJson from "./package.json";

export default defineConfig({
  base: "./",
  define: {
    __NFIDB_CLIENT_VERSION__: JSON.stringify(packageJson.version),
  },
  build: {
    target: "safari16.4",
    sourcemap: true,
    cssCodeSplit: false,
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
