import { defineConfig } from "@playwright/test";

// Golden-path e2e for the desktop UI against a mocked core bridge.
//
// `vite preview` serves the production build (`pnpm build` must run first —
// see package.json `test:e2e` / CI). No Tauri runtime is present in a plain
// browser, so `isTauri()` in src/bridge/api.ts returns false and every bridge
// call automatically falls back to the JS mock fixture/backend — no extra
// env flag needed.
//
// Port 4321 avoids colliding with the `vite dev` Tauri convention (1420) and
// with vite's own preview default (4173), so a stray `pnpm preview` left
// running elsewhere on this machine won't collide with the e2e run.
const PORT = 4321;

export default defineConfig({
  testDir: "./tests/e2e",
  use: { baseURL: `http://localhost:${PORT}` },
  webServer: {
    command: `pnpm exec vite preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
