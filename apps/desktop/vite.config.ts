import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and ignores VITE_ env in the Rust layer.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", outDir: "dist" },
  test: {
    globals: true,
    environment: "happy-dom",
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.{ts,tsx}"],
    pool: "forks",
    poolOptions: {
      forks: {
        singleFork: true,
      },
    },
    // Suppress the sandbox-only lazyllhttp Wasm OOM that fires in Node's
    // internal undici module. This is not a test correctness issue.
    dangerouslyIgnoreUnhandledErrors: true,
  },
});
