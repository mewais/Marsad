import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and ignores VITE_ env in the Rust layer.
//
// Test config lives solely in vitest.config.ts (see that file for why it's
// separate) — Vitest resolves `vitest.config.ts` in preference to this file
// when both exist, and the `test` script points at it explicitly, so a
// duplicate `test` block here would be dead config that could silently
// drift out of sync.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", outDir: "dist" },
});
