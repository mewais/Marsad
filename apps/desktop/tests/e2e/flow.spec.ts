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
  const aiBefore = await page.locator(".msg-ai").count();
  const box = page.getByRole("textbox", { name: "Message" });
  await box.fill("also add a rollback note");
  await box.press("Enter");
  // The prompt is written to the agent (shown as "sent", not "Not sent"). The agent is still waiting on
  // its question, and no assistant reply has been added to the transcript.
  const mine = page.locator(".msg-user", { hasText: "also add a rollback note" });
  await expect(mine).toBeVisible();
  await expect(mine).toHaveAttribute("data-delivery", "sent");
  await expect(mine.getByTestId("delivery")).toHaveCount(0);
  await expect(page.getByText("Question from Claude Code")).toBeVisible();
  await expect(page.locator(".msg-ai")).toHaveCount(aiBefore);

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
