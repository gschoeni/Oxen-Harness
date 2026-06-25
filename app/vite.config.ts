/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Vite options tailored for Tauri development, applied in `tauri dev`/`build`.
  // 1. Don't let Vite clear the screen so Rust errors stay visible.
  clearScreen: false,
  // 2. Tauri expects a fixed port; fail if it's unavailable.
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1431,
        }
      : undefined,
    watch: {
      // 3. Don't watch the Rust crate from Vite (Tauri handles that).
      ignored: ["**/src-tauri/**"],
    },
  },

  // Headless UI tests (Vitest + Testing Library + mocked Tauri IPC).
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: "./src/test/setup.ts",
    css: false,
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
