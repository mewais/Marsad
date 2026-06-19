/**
 * Minimal e2e smoke spec — verifies the app loads and the wordmark is visible.
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

test("filter chips are visible", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("All")).toBeVisible();
  await expect(page.getByText(/Needs you/)).toBeVisible();
});
