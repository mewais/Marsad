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
  // Scope to the sidebar filter chips (role="button") — "Needs you" also
  // appears as a StatTile label and an "Needs your attention" section
  // heading on the Overview screen, so a plain text locator is ambiguous.
  await expect(page.getByRole("button", { name: "All" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^Needs you/ })).toBeVisible();
});
