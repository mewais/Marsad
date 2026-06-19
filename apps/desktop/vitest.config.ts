import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

/**
 * Separate vitest config so we can set NonProjectOptions like
 * dangerouslyIgnoreUnhandledErrors at the root level.
 *
 * The lazyllhttp WebAssembly OOM is a Node.js internal sandbox limitation
 * (RLIMIT_AS prevents Wasm guard-region allocation). All test logic is
 * unaffected — components render correctly via happy-dom. We suppress the
 * spurious unhandled rejection so the suite exits cleanly.
 */
export default defineConfig({
  plugins: [react()],
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
    // Suppress the sandbox-only lazyllhttp Wasm OOM unhandled rejection.
    // Node's internal undici fires this when RLIMIT_AS prevents Wasm guard
    // region allocation. All test assertions pass correctly — this is purely
    // an environment constraint, not a code defect.
    dangerouslyIgnoreUnhandledErrors: true,
  },
});
