import { expect, test, type Locator, type Page } from "@playwright/test";

// Side panels never scroll sideways: with long paths, names, versions and errors (`?long=1`)
// every card and list inside them fits its width. MARSAD_SHOTS=<dir> also writes screenshots.
const SHOTS = process.env.MARSAD_SHOTS;
const FX = "?live=0&latency=10&speed=0.3&long=1";
const S7 = "fxedcbbvaihv";

async function go(page: Page, hash: string) {
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.goto(`/${FX}#${hash}`);
}

/** Elements under `root` (and `root` itself) whose content is wider than their box. */
async function overflowing(root: Locator) {
  return root.evaluate((el) => {
    const out: string[] = [];
    const all = [el, ...Array.from(el.querySelectorAll<HTMLElement>("*"))];
    for (const e of all) {
      const cs = getComputedStyle(e);
      if (cs.display === "inline" || cs.display === "none") continue;
      // A deliberate ellipsis (with the full value in a title) is fine.
      if (cs.textOverflow === "ellipsis" && cs.overflowX === "hidden") continue;
      // Scrollers of their own (code, terminals) are allowed to scroll inside, but not the panel.
      if (e !== el && (cs.overflowX === "auto" || cs.overflowX === "scroll")) continue;
      if (e.scrollWidth > e.clientWidth + 1) {
        out.push(`${e.tagName.toLowerCase()}.${String(e.className).replace(/\s+/g, ".")} ${e.scrollWidth}>${e.clientWidth} "${(e.textContent ?? "").slice(0, 60)}"`);
      }
    }
    // Anything that sticks out of the panel's right edge.
    const right = el.getBoundingClientRect().right;
    for (const e of all) {
      const r = e.getBoundingClientRect();
      if (r.width && r.right > right + 1 && getComputedStyle(e).position !== "fixed") {
        out.push(`${e.tagName.toLowerCase()}.${String(e.className).replace(/\s+/g, ".")} sticks out by ${Math.round(r.right - right)}px`);
      }
    }
    return out;
  });
}

async function noSideways(page: Page, root: Locator, name: string) {
  await expect(root).toBeVisible();
  // Let fonts and late layout settle (fixture responses arrive after first paint).
  await page.waitForTimeout(150);
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/nit-${name}.png` });
  expect(await overflowing(root), name).toEqual([]);
}

for (const width of [1024, 1440]) {
  test.describe(`at ${width}px`, () => {
    test.use({ viewport: { width, height: 860 } });

    test("agent config side panel", async ({ page }) => {
      await go(page, "/config?driver=claude&host=x1");
      const panel = page.getByRole("complementary", { name: "Configuration layers" });
      await expect(panel.getByText(/some-long-skill-name\/SKILL\.md/)).toBeVisible();
      await noSideways(page, panel, `config-${width}`);
      const eff = page.getByRole("region", { name: "Effective configuration" });
      await expect(eff.getByText("some-long-skill-name", { exact: true })).toBeVisible();
      expect(await overflowing(eff), "effective table").toEqual([]);
    });

    test("session Config tab and right rail", async ({ page }) => {
      await go(page, `/session/x1/${S7}`);
      await noSideways(page, page.getByRole("complementary", { name: "Session details" }), `session-rail-${width}`);
      await page.getByRole("tab", { name: "Config" }).click();
      const tab = page.getByRole("tabpanel");
      await expect(tab.getByText("some-long-skill-name", { exact: true })).toBeVisible();
      await noSideways(page, tab, `session-config-${width}`);
    });

    test("hosts detail panel", async ({ page }) => {
      for (const h of ["hz", "win"]) {
        await go(page, `/hosts?h=${h}`);
        const panel = page.locator("aside.hd");
        await expect(panel.getByRole("heading").first()).toBeVisible();
        await noSideways(page, panel, `hosts-${h}-${width}`);
      }
    });

    test("workstream inspector", async ({ page }) => {
      await go(page, "/workstreams/ws-invoice");
      await expect(page.getByTestId("graph-canvas")).toBeVisible();
      await noSideways(page, page.locator("aside.insp"), `workstream-${width}`);
      // An agent on the host with the long name, branch and versions.
      await page.getByTestId("graph-canvas").getByText("pdf-renderer-worker").first().click();
      await noSideways(page, page.getByRole("complementary", { name: "Agent" }), `workstream-agent-${width}`);
    });

    test("new session dialog", async ({ page }) => {
      await go(page, "/inbox");
      await expect(page.getByRole("list", { name: "Sessions" }).getByText("pdf-renderer-worker")).toBeVisible();
      await page.keyboard.press("n");
      const dlg = page.getByRole("dialog", { name: "New session" });
      await expect(dlg).toBeVisible();
      await noSideways(page, dlg, `newsession-${width}`);
    });
  });
}
