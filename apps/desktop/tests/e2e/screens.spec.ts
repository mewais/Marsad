import { expect, test, type Page } from "@playwright/test";

// Walks every screen against `vite preview` + FixtureBackend. With MARSAD_SHOTS=<dir> it also
// writes screenshots (dark everywhere; light for the main screens; 1024x700 for the tight ones).
const SHOTS = process.env.MARSAD_SHOTS;
const FX = "?live=0&latency=10&speed=0.3";
const S7 = "fxedcbbvaihv"; // migrate-auth-middleware on x1 (fixture id)

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` });
}
async function go(page: Page, hash: string, theme: "dark" | "light" = "dark") {
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
  await page.goto(`/${FX}#${hash}`);
}

for (const theme of ["dark", "light"] as const) {
  test.describe(`${theme} theme`, () => {
    test.use({ viewport: { width: 1440, height: 900 } });

    test(`inbox and session tabs (${theme})`, async ({ page }) => {
      await go(page, "/inbox", theme);
      await expect(page.getByRole("list", { name: "Sessions" }).getByText("pdf-renderer-worker")).toBeVisible();
      await shot(page, `inbox-${theme}-1440`);
      await go(page, `/session/x1/${S7}`, theme);
      await expect(page.getByRole("heading", { name: "migrate-auth-middleware" })).toBeVisible();
      await expect(page.getByText(/find every place that reads/)).toBeVisible();
      await shot(page, `session-${theme}-1440`);
      if (theme === "light") return;
      await page.getByRole("tab", { name: "Changes" }).click();
      await expect(page.getByText("import { authMiddleware } from '@ledgerline/auth'")).toBeVisible();
      await shot(page, "session-changes-dark-1440");
      await page.getByRole("button", { name: "Split" }).click();
      await shot(page, "session-changes-split-dark-1440");
      await page.getByRole("tab", { name: "Config" }).click();
      await expect(page.getByText("turned off at Host").first()).toBeVisible();
      await shot(page, "session-config-dark-1440");
      await page.getByRole("tab", { name: "Usage" }).click();
      await expect(page.getByText("Context window", { exact: true })).toBeVisible();
      await shot(page, "session-usage-dark-1440");
      await page.getByRole("tab", { name: "Terminal" }).click();
      await expect(page.getByText("Terminal not available")).toBeVisible();
      await shot(page, "session-terminal-dark-1440");
    });

    test(`hosts and sign-in (${theme})`, async ({ page }) => {
      await go(page, "/hosts?h=win", theme);
      await expect(page.getByRole("listbox", { name: "Hosts" }).getByText("gpu-box")).toBeVisible();
      await shot(page, `hosts-${theme}-1440`);
      if (theme === "light") return;
      await go(page, "/hosts?h=gpu");
      await page.getByRole("button", { name: "Connect", exact: true }).click();
      const dlg = page.getByRole("dialog", { name: "Sign in to gpu-box" });
      await expect(dlg.getByText("ml@10.0.4.21's password:")).toBeVisible();
      await shot(page, "hosts-signin-dark-1440");
      await dlg.getByLabel("Password or passphrase").fill("hunter2");
      await dlg.getByRole("button", { name: "Sign in" }).click();
      await page.getByLabel("One-time code").fill("123456");
      await page.getByRole("button", { name: "Sign in" }).click();
      await expect(page.getByText(/Waiting for approval on your device/)).toBeVisible();
      await shot(page, "hosts-signin-push-dark-1440");
      await expect(page.locator('[data-host="gpu"]').getByText("Connected")).toBeVisible({ timeout: 10_000 });
      await go(page, "/hosts?add=1");
      await expect(page.getByRole("dialog", { name: "Add host" })).toBeVisible();
      await shot(page, "hosts-add-dark-1440");
    });

    test(`agent config (${theme})`, async ({ page }) => {
      await go(page, `/config?driver=claude&host=x1&session=${S7}`, theme);
      await expect(page.getByText("turned off at Host").first()).toBeVisible();
      await expect(page.getByText("read-only")).toBeVisible();
      await shot(page, `config-${theme}-1440`);
    });

    test(`workstreams and graph (${theme})`, async ({ page }) => {
      if (theme === "dark") {
        await go(page, "/workstreams");
        await expect(page.getByRole("link", { name: /Invoice export v2/ })).toBeVisible();
        await shot(page, "workstreams-dark-1440");
        await page.keyboard.press("Shift+N");
        await page.getByRole("dialog", { name: "New workstream" }).getByLabel("Name").fill("Docs search revamp 2");
        await page.getByRole("radio", { name: /From existing sessions/ }).check();
        await shot(page, "workstreams-new-dark-1440");
        await page.keyboard.press("Escape");
      }
      await go(page, "/workstreams/ws-invoice", theme);
      await expect(page.getByTestId("graph-canvas")).toBeVisible();
      await expect(page.getByRole("button", { name: /Review/ })).toBeVisible();
      await shot(page, `graph-${theme}-1440`);
      await go(page, "/workstreams/ws-docs", theme);
      await expect(page.getByText("Before you launch")).toBeVisible();
      await shot(page, `graph-design-${theme}-1440`);
      if (theme === "light") return;
      await page.getByRole("button", { name: "Add agent" }).first().click();
      await expect(page.getByRole("dialog", { name: /Add agent to Docs search revamp/ })).toBeVisible();
      await shot(page, "graph-design-add-dark-1440");
    });
  });
}

test.describe("settings and dialogs (dark)", () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  test("settings sections, new session, palette and shortcut sheet", async ({ page }) => {
    for (const s of ["general", "notifications", "keyboard", "about"]) {
      await go(page, `/settings?s=${s}`);
      await expect(page.getByRole("heading", { name: s[0].toUpperCase() + s.slice(1) })).toBeVisible();
      await shot(page, `settings-${s}-dark-1440`);
    }
    await go(page, "/inbox");
    await expect(page.getByRole("list", { name: "Sessions" }).getByText("pdf-renderer-worker")).toBeVisible();
    await page.keyboard.press("n");
    const ns = page.getByRole("dialog", { name: "New session" });
    await expect(ns.getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true");
    await shot(page, "new-session-dark-1440");
    await ns.getByRole("button", { name: "Continue an existing one" }).click();
    await expect(ns.getByRole("listbox", { name: "Sessions to continue" })).toBeVisible();
    await shot(page, "new-session-continue-dark-1440");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Control+k");
    await page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox").fill("work");
    await shot(page, "palette-dark-1440");
    await page.keyboard.press("Escape");
    await page.keyboard.press("?");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" }).getByText("Review the waiting handoff")).toBeVisible();
    await shot(page, "shortcuts-dark-1440");
  });
});

test.describe("1024x700", () => {
  test.use({ viewport: { width: 1024, height: 700 } });
  test("new session and graph fit", async ({ page }) => {
    await go(page, "/inbox");
    await expect(page.getByRole("list", { name: "Sessions" }).getByText("pdf-renderer-worker")).toBeVisible();
    await page.keyboard.press("n");
    const ns = page.getByRole("dialog", { name: "New session" });
    await expect(ns.getByRole("button", { name: /Start session/ })).toBeInViewport();
    await shot(page, "new-session-dark-1024");
    await page.keyboard.press("Escape");
    await go(page, "/workstreams/ws-invoice");
    await expect(page.getByTestId("graph-canvas")).toBeVisible();
    await shot(page, "graph-dark-1024");
  });
});
