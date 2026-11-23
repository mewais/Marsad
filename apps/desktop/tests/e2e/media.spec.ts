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
  await imgs.nth(1).scrollIntoViewIfNeeded();
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

// Polish: relative HTML resources, highlighted code, Markdown, path chips, first-prompt attachments.
test("html preview inlines its relative image and counts blocked remote content; code is coloured; markdown renders", async ({ page }) => {
  await open(page);
  const chip = (name: string) => page.getByTestId("file-ref").filter({ hasText: name }).first();
  const pv = page.getByTestId("preview");

  await chip("index.html").click();
  await expect(page.getByTestId("html-blocked")).toContainText("Remote content blocked: 2");
  const frame = page.getByTestId("preview-html");
  expect(await frame.getAttribute("sandbox")).toBe("");
  const doc = await frame.getAttribute("srcdoc");
  expect(doc).toContain("data:image/png;base64,");
  expect(doc).not.toContain("example.com");
  // The image really renders inside the frame.
  const img = page.frameLocator("[data-testid=preview-html]").locator("#dot");
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
  await page.keyboard.press("Escape");

  await chip("app.ts:2").click();
  await expect(pv).toHaveAttribute("data-kind", "text");
  await expect(page.locator("[data-testid=preview-text] .hljs-keyword").first()).toBeVisible();
  await expect(page.locator("[data-testid=preview-text] [data-ln='2'].focus")).toHaveCount(1);
  await page.getByLabel("Wrap lines").check();
  await page.keyboard.press("Escape");

  await chip("guide.md").click();
  await expect(page.getByTestId("preview-markdown").locator("h1")).toHaveText("Guide");
  await expect(page.getByTestId("preview-markdown").locator("script")).toHaveCount(0);
  await page.getByRole("button", { name: "Source" }).click();
  await expect(page.getByTestId("preview-text")).toHaveAttribute("data-lang", "markdown");
});

test("the transcript colours code and diffs, and only paths inside the folder are chips", async ({ page }) => {
  await open(page);
  await expect(page.locator("[data-testid=md-code] .hljs-keyword").first()).toBeVisible();
  const chips = await page.getByTestId("file-ref").allTextContents();
  expect(chips.some((c) => c.startsWith("todo.md"))).toBe(false);
  expect(chips.some((c) => c.startsWith("config"))).toBe(false);
  await expect(page.locator("main")).toContainText("../notes/todo.md");
  // The Edit is one of three consecutive tool calls: open that run first.
  await page.getByTestId("tool-run").getByRole("button", { name: /tool calls/ }).last().click();
  await page.getByRole("button", { name: /^Edit/ }).first().click();
  await expect(page.getByTestId("tool-diff").first()).toBeVisible();
});

test("new session: attach an image to the first prompt; it is uploaded and sent after the session is created", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/${FX}#/inbox`);
  await expect(page.locator("[data-session]").first()).toBeVisible();
  await page.keyboard.press("n");
  const dlg = page.getByRole("dialog", { name: "New session" });
  await expect(dlg.getByLabel("Project folder")).not.toHaveValue("");
  await expect(dlg.getByRole("button", { name: "Attach files" })).toBeEnabled();
  await dlg.getByTestId("ns-attach-input").setInputFiles({ name: "shot.png", mimeType: "image/png", buffer: Buffer.from([0x89, 0x50, 0x4e, 0x47]) });
  await expect(dlg.getByTestId("pending-attachment")).toHaveCount(1);
  await dlg.getByLabel(/First message/).fill("what is in this picture");
  await dlg.getByRole("button", { name: /Start session/ }).click();
  await expect(page).toHaveURL(/#\/session\/x1\//);
});
