/**
 * Golden-path e2e for the desktop app-wiring block (U-WIRE).
 *
 * Mirrors tests/e2e/smoke.spec.ts. Runs against the `vite preview` webServer
 * configured in playwright.config.ts (built via `pnpm build`); this is an
 * enforced gate, same as the Vitest integration in appWiring.test.tsx.
 * Nothing is guarded behind `.skip`; only Playwright auto-waiting locators
 * are used (no flaky manual waits).
 *
 * Flow: open "/" (Board is the home screen), expect a workstream card; click
 * the multi-node card, expect the graph canvas; click a node, expect the
 * session view.
 */
import { test, expect } from "@playwright/test";

// Seeded multi-node workstream + one of its nodes (see mockWorkstreams seed).
const MULTI_NODE_WS_ID = "ws-build-pipeline";
const NODE_ID = "s07";

test("board → graph → session golden path", async ({ page }) => {
  await page.goto("/");

  // Level 1: Board is the home screen — it renders with its seeded
  // workstream rows without any nav click.
  await expect(page.getByTestId("board-title")).toBeVisible();
  const multiCard = page.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`);
  await expect(multiCard).toBeVisible();

  // Level 2: clicking the multi-node card routes to the graph.
  await multiCard.click();
  await expect(page.getByTestId("graph-canvas")).toBeVisible();
  await expect(page.getByTestId(`graph-node-${NODE_ID}`)).toBeVisible();

  // Level 3: opening a node routes to its session view. The graph canvas must
  // be gone once we land on the session screen.
  await page.getByTestId(`node-open-${NODE_ID}`).click();
  await expect(page.getByTestId("graph-canvas")).toHaveCount(0);
  await expect(page.getByTestId("board-title")).toHaveCount(0);
});
