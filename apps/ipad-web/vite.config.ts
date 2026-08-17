import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  build: {
    target: "safari16.4",
    sourcemap: true,
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        entryFileNames: "assets/client.js",
        assetFileNames: "assets/client.[ext]",
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
