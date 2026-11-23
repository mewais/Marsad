import { expect, test, type Page } from "@playwright/test";

// Developer mode's panels against `vite preview` + FixtureBackend (?developer=1 starts with the
// hub's setting on). With MARSAD_SHOTS=<dir> it also writes screenshots.
const SHOTS = process.env.MARSAD_SHOTS;
const FX = "?live=0&latency=10&speed=0.3&developer=1";
const TAURI = "fxmlkjkunvk6"; // upgrade-tauri-2-5 on x1: dropped prompts and an anomaly
const CI = "fxjihggkxkfp"; // ci-triage-lead on hz: failed
const WIN = "fx32zyyvs3em"; // windows-print-check on win (not connected)

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` });
}
async function go(page: Page, hash: string, theme: "dark" | "light" = "dark", fx = FX) {
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
  await page.goto(`/${fx}#${hash}`);
}

for (const theme of ["dark", "light"] as const) {
  test.describe(`${theme} theme`, () => {
    test.use({ viewport: { width: 1440, height: 900 } });

    test(`settings: developer mode (${theme})`, async ({ page }) => {
      await go(page, "/settings?s=diagnostics", theme, "?live=0&latency=10&speed=0.3");
      const sw = page.getByRole("switch", { name: "Developer mode" });
      await expect(sw).toHaveAttribute("aria-checked", "false");
      await shot(page, `diag-settings-off-${theme}-1440`);
      await sw.click();
      await expect(sw).toHaveAttribute("aria-checked", "true");
      await shot(page, `diag-settings-on-${theme}-1440`);
    });

    test(`session diagnostics: prompts and timeline (${theme})`, async ({ page }) => {
      await go(page, `/session/x1/${TAURI}?tab=diagnostics`, theme);
      const panel = page.getByTestId("session-diagnostics");
      await expect(panel.getByText("#640", { exact: true })).toBeVisible();
      await expect(panel.getByText("Dropped (interrupted)").first()).toBeVisible();
      await expect(panel.getByText("The agent produced output after its turn had ended (first: tool_call_started)")).toBeVisible();
      await shot(page, `diag-session-${theme}-1440`);
      if (theme === "light") return;
      await panel.getByText("The agent produced output after its turn had ended").click();
      await shot(page, "diag-session-expanded-dark-1440");
    });

    test(`session diagnostics on a failed session of another machine (${theme})`, async ({ page }) => {
      await go(page, `/session/hz/${CI}?tab=diagnostics`, theme);
      const panel = page.getByTestId("session-diagnostics");
      await expect(panel.getByText("Failed — the process exited with code 1: pnpm test exited 1")).toBeVisible();
      if (theme === "dark") {
        await panel.getByRole("button", { name: /Host log for this session/ }).click();
        await expect(panel.getByText("The daemon has logged nothing about this session")).toBeVisible();
      }
      await shot(page, `diag-session-failed-${theme}-1440`);
    });

    test(`host diagnostics (${theme})`, async ({ page }) => {
      await go(page, "/hosts?h=win", theme);
      const log = page.getByTestId("connection-log");
      await expect(log.getByText("Attempt 3", { exact: true })).toBeVisible();
      await expect(log.getByText("Failed at spawn (unreachable) · exit code 255").last()).toBeVisible();
      await shot(page, `diag-host-failed-${theme}-1440`);
      if (theme === "light") return;
      await go(page, "/hosts?h=hz");
      await expect(page.getByRole("region", { name: "On the host" })).toBeVisible();
      await shot(page, "diag-host-connected-dark-1440");
      await go(page, "/hosts?h=x1");
      await expect(page.getByText(/This is the local host/)).toBeVisible();
      await shot(page, "diag-host-local-dark-1440");
    });
  });
}

test.describe("developer mode and an unreachable host", () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  test("a session on a host that is not connected says so and reads nothing", async ({ page }) => {
    await go(page, `/session/win/${WIN}?tab=diagnostics`);
    await expect(page.getByTestId("diag-unreachable")).toContainText("win-build is not connected");
    await shot(page, "diag-session-unreachable-dark-1440");
  });

  test("with developer mode off there is no Diagnostics tab", async ({ page }) => {
    await go(page, `/session/x1/${TAURI}`, "dark", "?live=0&latency=10&speed=0.3");
    await expect(page.getByRole("heading", { name: "upgrade-tauri-2-5" })).toBeVisible();
    await expect(page.getByRole("tab", { name: "Diagnostics" })).toHaveCount(0);
  });

  test.describe("tight window", () => {
    test.use({ viewport: { width: 1024, height: 700 } });
    test("the panel fits", async ({ page }) => {
      await go(page, `/session/x1/${TAURI}?tab=diagnostics`);
      await expect(page.getByTestId("session-diagnostics").getByText("#640", { exact: true })).toBeVisible();
      await shot(page, "diag-session-dark-1024");
    });
  });
});
