import { defineConfig } from "@playwright/test";

// Walks the UI against `vite preview` (run `pnpm build` first). No Tauri runtime exists in a
// plain browser, so the app falls back to the in-memory FixtureBackend.
// Set PW_CHROMIUM to use a specific chromium binary (e.g. /opt/pw-browsers/chromium).
const PORT = 4321;
const executablePath = process.env.PW_CHROMIUM || undefined;

export default defineConfig({
  testDir: "./tests/e2e",
  use: {
    baseURL: `http://localhost:${PORT}`,
    launchOptions: executablePath ? { executablePath } : {},
  },
  webServer: {
    command: `pnpm exec vite preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
