import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Unmount all rendered components and reset the DOM between every test.
// Required in singleFork mode (happy-dom persists across test files otherwise)
// so getByTestId queries don't find stale DOM from a previous test file.
afterEach(() => {
  cleanup();
});

// Suppress the lazyllhttp WebAssembly unhandled rejection that fires in the
// sandbox environment where RLIMIT_AS prevents Wasm guard-region allocation.
// This is a Node.js internal undici detail, not a test correctness issue.
// Tests still exercise all real component logic via happy-dom.
if (typeof process !== "undefined") {
  process.on("unhandledRejection", (reason) => {
    const msg = String(reason);
    if (msg.includes("lazyllhttp") || msg.includes("WebAssembly.instantiate")) {
      // swallow — sandbox RLIMIT_AS limitation, not a test failure
      return;
    }
    // Re-throw anything else so real failures surface
    throw reason;
  });
}
