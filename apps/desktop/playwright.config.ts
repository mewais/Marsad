import { defineConfig } from "@playwright/test";

// Golden-path e2e for the desktop UI against a mocked core bridge.
// TODO(D02): point webServer at `vite preview` with a mock-bridge env flag.
export default defineConfig({
  testDir: "./tests/e2e",
  use: { baseURL: "http://localhost:1420" },
});
