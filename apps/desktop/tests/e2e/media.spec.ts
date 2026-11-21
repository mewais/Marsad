import { expect, test, type Page } from "@playwright/test";

// Images and files in the transcript, attachments in the composer and the preview panel, on
// the fixture backend (`?media=1` adds a session with media). MARSAD_SHOTS=<dir> also writes
// screenshots.
const SHOTS = process.env.MARSAD_SHOTS;
const FX = "?live=0&latency=10&speed=0.3&media=1";
const MEDIA = "fxmlkjsdygj3";

async function open(page: Page) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.goto(`/${FX}#/session/x1/${MEDIA}`);
  await expect(page.getByText("Turn data.csv into a bar chart")).toBeVisible();
}

const shot = async (page: Page, name: string) => {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` });
};

test("transcript shows images inline and files as chips; the lightbox opens them", async ({ page }) => {
  await open(page);
  // The user's attachment and the tool's image load as thumbnails.
  const imgs = page.getByTestId("media-image");
  await expect(imgs).toHaveCount(2);
  await expect(imgs.nth(1).locator("img")).toHaveJSProperty("complete", true);
  expect(await imgs.nth(1).locator("img").evaluate((i: HTMLImageElement) => i.naturalWidth)).toBe(240);
  // Files the agent wrote and mentions, and the resource it showed.
  for (const name of ["chart.svg", "report.pdf", "preview.html", "README.md"]) await expect(page.getByTestId("file-ref").or(page.getByTestId("file-chip")).filter({ hasText: name }).first()).toBeVisible();
  await shot(page, "media-transcript");

  await imgs.nth(1).click();
  const pv = page.getByTestId("preview");
  await expect(pv).toHaveAttribute("data-kind", "image");
  await expect(page.getByTestId("preview-image")).toBeVisible();
  await shot(page, "media-lightbox");
  await page.keyboard.press("Escape");
  await expect(pv).toHaveCount(0);
});

test("previews: svg as an image, pdf, html sandboxed, csv, markdown, json, other", async ({ page }) => {
  await open(page);
  const chip = (name: string) => page.getByTestId("file-ref").or(page.getByTestId("file-chip")).filter({ hasText: name }).first();
  const pv = page.getByTestId("preview");

  await chip("chart.svg").click();
  await expect(pv).toHaveAttribute("data-kind", "svg");
  // Shown through <img>: its <script> never runs (the page title is untouched).
  await expect(page.getByTestId("preview-image")).toBeVisible();
  expect(await page.title()).not.toBe("pwned");
  await page.keyboard.press("Escape");

  await chip("report.pdf").click();
  await expect(pv).toHaveAttribute("data-kind", "pdf");
  await expect(page.getByTestId("preview-pdf")).toHaveAttribute("data-pages", "1");
  await expect(page.locator(".pv-pages canvas")).toHaveCount(1);
  await shot(page, "media-pdf");
  await page.keyboard.press("Escape");

  await chip("preview.html").click();
  await expect(pv).toHaveAttribute("data-kind", "html");
  const frame = page.getByTestId("preview-html");
  await expect(frame).toHaveAttribute("sandbox", "");
  const doc = page.frameLocator('[data-testid="preview-html"]');
  await expect(doc.locator("h1")).toHaveText("Quarterly report");
  await shot(page, "media-html");
  await page.getByLabel("Run scripts").check();
  await expect(page.getByTestId("preview-html")).toHaveAttribute("sandbox", "allow-scripts");
  await expect(doc.locator("h1")).toHaveText("Scripts ran");
  // The remote image was never fetched (CSP: no network).
  expect(await doc.locator("img").evaluate((i: HTMLImageElement) => i.naturalWidth)).toBe(0);
  await page.keyboard.press("Escape");

  await chip("README.md").click();
  await expect(page.getByTestId("preview-markdown")).toContainText("The chart is in");
  await expect(page.getByTestId("preview-markdown").locator("img, script")).toHaveCount(0);
  await page.keyboard.press("Escape");
});

test("composer: attach, see chips, remove, send uploads and shows the attachment", async ({ page }) => {
  await open(page);
  const input = page.getByTestId("attach-input");
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP4z8BAEiJN9aiGUQ1DSgMAkPn/Afnh+ngAAAAASUVORK5CYII=", "base64");
  await input.setInputFiles([
    { name: "red.png", mimeType: "image/png", buffer: png },
    { name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("hello") },
    { name: "huge.png", mimeType: "image/png", buffer: Buffer.alloc(6 * 1024 * 1024, 1) },
  ]);
  const pending = page.getByTestId("pending-attachment");
  await expect(pending).toHaveCount(3);
  await expect(page.getByRole("alert").filter({ hasText: "huge.png" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();
  await expect(pending.filter({ hasText: "notes.txt" })).toHaveAttribute("title", /names its path/);
  await page.getByLabel("Message").fill("What is in these?");
  await shot(page, "media-composer");
  await page.getByRole("button", { name: "Remove huge.png" }).click();
  await expect(pending).toHaveCount(2);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(pending).toHaveCount(0);
  const sent = page.getByTestId("sent-attachments").last();
  await expect(sent.getByTestId("media-image")).toHaveCount(1);
  await expect(sent.getByTestId("file-chip").filter({ hasText: "notes.txt" })).toBeVisible();
});
