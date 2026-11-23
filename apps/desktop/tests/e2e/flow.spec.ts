import { expect, test } from "@playwright/test";

// Inbox -> answer a gate -> open the session -> send a prompt, against `vite preview` + FixtureBackend.
test("inbox to answered gate to session to prompt", async ({ page }) => {
  await page.goto("/?live=0&latency=20&speed=0.2#/inbox");

  const list = page.getByRole("list", { name: "Sessions" });
  await expect(list.getByText("pdf-renderer-worker")).toBeVisible();
  await expect(page.getByText("4 need you")).toBeVisible();

  // Answer the permission gate (Claude sends no wording, so Marsad labels it "Allow").
  const pdf = page.locator('[data-session^="hz/"]', { hasText: "pdf-renderer-worker" });
  await expect(pdf).toHaveAttribute("data-status", "needs");
  await pdf.getByRole("button", { name: /^Allow$/ }).click();
  await expect(pdf).not.toHaveAttribute("data-status", "needs");
  await expect(page.getByText("3 need you")).toBeVisible();

  // Open a session and send a prompt.
  await list.getByRole("button", { name: "migrate-auth-middleware" }).click();
  await expect(page.getByRole("heading", { name: "migrate-auth-middleware" })).toBeVisible();
  await expect(page.getByText("Question from Claude Code")).toBeVisible();
  const box = page.getByRole("textbox", { name: "Message" });
  await box.fill("also add a rollback note");
  await box.press("Enter");
  // The agent is waiting on its questions, so it takes nothing new yet: the message is there, no reply is.
  await expect(page.locator(".msg-user", { hasText: "also add a rollback note" })).toBeVisible();

  // An idle agent runs a prompt at once and answers.
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: /Inbox/ }).click();
  await page.getByRole("searchbox", { name: "Filter sessions" }).fill("docs-search");
  await list.getByRole("button", { name: "docs-search-index" }).click();
  await expect(page.getByRole("heading", { name: "docs-search-index" })).toBeVisible();
  await page.getByRole("textbox", { name: "Message" }).fill("add a sitemap");
  await page.getByRole("textbox", { name: "Message" }).press("Enter");
  await expect(page.locator(".msg-user", { hasText: "add a sitemap" })).toBeVisible();
  await expect(page.getByText(/Nothing else needs your attention/)).toBeVisible({ timeout: 10_000 });
});
