/**
 * Minimal e2e smoke spec — verifies the app loads onto the Board (the home
 * screen since the Workstreams redesign SLICE A) with the glass shell chrome.
 * Requires `pnpm dev` or `pnpm build && pnpm preview` to be running.
 */
import { test, expect } from "@playwright/test";

test("wordmark is visible", async ({ page }) => {
  await page.goto("/");
  // The wordmark div should be present and contain "Marsad"
  const wordmark = page.getByTestId("wordmark");
  await expect(wordmark).toBeVisible();
  await expect(wordmark).toContainText("Marsad");
});

test("boots directly onto the Board with the shell controls visible", async ({ page }) => {
  await page.goto("/");
  // Board is the home screen — no click needed to reach it.
  await expect(page.getByTestId("board-title")).toBeVisible();
  await expect(page.getByTestId("new-workstream-btn")).toBeVisible();
  await expect(page.getByTestId("open-settings-btn")).toBeVisible();
});
