import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { toastStore } from "../../src/ui/toast";
import { quietBackend, renderApp, sessionByName } from "../helpers";

type Backend = ReturnType<typeof quietBackend>;

const SESSION = "migrate-auth-middleware";

/** The session page of the fixture session, with its header's Export entry. `manual`: the export's steps are played by hand. */
async function openSession(manual = false, backend: Backend = quietBackend()) {
  backend.sessionExports.manual = manual;
  const s = sessionByName(backend, SESSION);
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name: SESSION });
  const user = userEvent.setup();
  const entry = await screen.findByRole("button", { name: /Export conversation/ });
  await waitFor(() => expect(entry).toBeEnabled());
  await user.click(entry);
  const dialog = await screen.findByRole("dialog", { name: "Export conversation" });
  return { ...r, backend, user, dialog, s };
}

const requests = (b: Backend, method: string) => b.requests.filter((r) => r.method === method);
const step = (b: Backend, n = 1) => act(() => {
  for (let i = 0; i < n; i++) b.sessionExports.step();
});
const finish = (b: Backend) => act(() => {
  while (b.sessionExports.step());
});

beforeEach(() => toastStore.set([]));

describe("Export conversation: the dialog's options and the request they make", () => {
  it("opens from the session header, offers Both with subagents off and earlier history on, and says what the file holds", async () => {
    const { dialog } = await openSession();
    expect(within(dialog).getByRole("group", { name: "Format" })).toBeInTheDocument();
    const fmt = within(dialog).getByRole("group", { name: "Format" });
    expect(within(fmt).getAllByRole("button").map((b) => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([["Markdown", "false"], ["JSON", "false"], ["Both", "true"]]);
    expect(within(dialog).getByRole("checkbox", { name: /Include subagents/ })).not.toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: /Include earlier history/ })).toBeChecked();
    expect(dialog).toHaveTextContent("Tool outputs longer than 64 KB are cut to 64 KB.");
  });

  it("starts with the chosen format and switches, on the session's host", async () => {
    const { backend, user, dialog } = await openSession();
    await user.click(within(dialog).getByRole("button", { name: "Markdown" }));
    await user.click(within(dialog).getByRole("checkbox", { name: /Include subagents/ }));
    await user.click(within(dialog).getByRole("checkbox", { name: /Include earlier history/ }));
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(requests(backend, "session.export.start")).toHaveLength(1));
    const [req] = requests(backend, "session.export.start");
    expect(req.host).toBe("x1");
    expect(req.params).toEqual({ session: sessionByName(backend, SESSION).id, formats: ["markdown"], include_subagents: true, include_earlier: false, tool_output_cap: 65536 });
  });

  it("asks for both files by default", async () => {
    const { backend, user, dialog } = await openSession();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(requests(backend, "session.export.start")).toHaveLength(1));
    expect((requests(backend, "session.export.start")[0].params as { formats: string[] }).formats).toEqual(["markdown", "json"]);
  });
});

describe("Export conversation: progress, cancel and the result", () => {
  it("shows the progress the export reports while it runs", async () => {
    const { backend, user, dialog } = await openSession(true);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByTestId("export-progress")).toHaveTextContent("0 events"));
    step(backend);
    await waitFor(() => expect(within(dialog).getByTestId("export-progress")).toHaveTextContent("40 events"));
    step(backend);
    await waitFor(() => expect(within(dialog).getByTestId("export-progress")).toHaveTextContent("120 events"));
    expect(within(dialog).queryByRole("button", { name: "Export" })).toBeNull();
  });

  it("cancels the running export on the host and shows that it did not finish", async () => {
    const { backend, user, dialog } = await openSession(true);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await user.click(await within(dialog).findByRole("button", { name: "Cancel export" }));
    expect(requests(backend, "session.export.cancel").map((r) => [r.host, r.params])).toEqual([["x1", { export_id: "sx001" }]]);
    const failed = await within(dialog).findByTestId("export-failed");
    expect(failed).toHaveTextContent("The export did not finish.");
    expect(failed).toHaveTextContent("cancelled");
  });

  it("shows how many secrets were masked, and that the file holds conversation content", async () => {
    const { backend, user, dialog } = await openSession();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByTestId("export-masked")).toHaveTextContent("3 secrets masked (best effort)"));
    expect(dialog).toHaveTextContent("The file contains conversation content");
    expect(within(dialog).getByRole("button", { name: "Save…" })).toBeEnabled();
    expect(backend.sessionExports.text("sx001", "markdown")).toContain("migrate-auth-middleware");
  });

  it("counts a single masked secret in the singular", async () => {
    const backend = quietBackend();
    backend.sessionExports.redactions = { api_key: 1 };
    const { user, dialog } = await openSession(false, backend);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByTestId("export-masked")).toHaveTextContent("1 secret masked (best effort)"));
  });

  it("shows the reason an export failed, and offers it again", async () => {
    const { backend, user, dialog } = await openSession(true);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    act(() => backend.sessionExports.fail("disk full"));
    const failed = await within(dialog).findByTestId("export-failed");
    expect(failed).toHaveTextContent("The export did not finish. disk full");
    expect(within(dialog).getByRole("button", { name: "Export" })).toBeEnabled();
  });

  it("shows a refusal of the start, such as a host that will not export", async () => {
    const backend = quietBackend();
    backend.sessionExports.refuse = { code: "conflict", message: "the host is busy" };
    const { user, dialog } = await openSession(false, backend);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    expect(await within(dialog).findByTestId("export-failed")).toHaveTextContent("the host is busy");
  });
});

describe("Export conversation: saving the files", () => {
  it("asks where to save each file and appends the file's bytes, chunk by chunk, to the place the user chose", async () => {
    const { backend, user, dialog } = await openSession();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Save…" })).toBeEnabled());
    await user.click(within(dialog).getByRole("button", { name: "Save…" }));
    await waitFor(() => expect(within(dialog).getByTestId("export-saved")).toBeInTheDocument());

    const chooses = requests(backend, "desktop.download_choose").map((r) => r.params);
    expect(chooses).toEqual([{ name: "conversation-sx001.md" }, { name: "conversation-sx001.json" }]);
    const reads = requests(backend, "session.export.read").map((r) => r.params as { format: string; offset: number; len: number; export_id: string });
    expect(reads.map((r) => [r.export_id, r.format, r.offset, r.len])).toEqual([
      ["sx001", "markdown", 0, 1 << 20],
      ["sx001", "json", 0, 1 << 20],
    ]);
    // The text written to each chosen file is the export's file, exactly.
    const md = "/home/me/Downloads/conversation-sx001.md";
    const chunks = backend.downloads.get(md)!;
    expect(chunks.length).toBeGreaterThan(0);
    const written = chunks.map((c) => new TextDecoder().decode(Uint8Array.from(atob(c), (ch) => ch.charCodeAt(0)))).join("");
    expect(written).toBe(backend.sessionExports.text("sx001", "markdown"));
    expect(within(dialog).getByTestId("export-saved")).toHaveTextContent(md);
  });

  it("says nothing was saved when the Save dialog is cancelled, and reads nothing", async () => {
    const backend = quietBackend();
    backend.saveAsAnswer = () => ({ saved: false });
    const { user, dialog } = await openSession(false, backend);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Save…" })).toBeEnabled());
    await user.click(within(dialog).getByRole("button", { name: "Save…" }));
    expect(await within(dialog).findByTestId("export-saved")).toHaveTextContent("Not saved: the Save dialog was cancelled.");
    expect(requests(backend, "session.export.read")).toEqual([]);
  });

  it("closing the dialog while the export runs cancels it on the host", async () => {
    const { backend, user, dialog } = await openSession(true);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(within(dialog).getByTestId("export-progress")).toBeInTheDocument());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(requests(backend, "session.export.cancel")).toHaveLength(1));
    expect(screen.queryByRole("dialog", { name: "Export conversation" })).toBeNull();
  });

  it("does not offer Save until the export is finished", async () => {
    const { backend, user, dialog } = await openSession(true);
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    step(backend);
    expect(within(dialog).queryByRole("button", { name: "Save…" })).toBeNull();
    finish(backend);
    expect(await within(dialog).findByRole("button", { name: "Save…" })).toBeEnabled();
  });
});
