import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { EARLIER_SPECS } from "../../src/bridge/fixture/earlier";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { SPECS } from "../../src/bridge/fixture/sessions";
import { dayTime } from "../../src/lib/format";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const COMPLETE = "resumed-invoice-rounding";
const LOADING = "resumed-stripe-webhook";
const MISSING = "resumed-orphan-notes";
const LONG = { ...EARLIER_SPECS[0], n: 60, name: "resumed-long", longHistory: true };

const backend = (extra: typeof SPECS = []) => quietBackend({ specs: [...SPECS, ...EARLIER_SPECS, ...extra] });

type Ref = ReturnType<typeof sessionByName>;
async function open(b: FixtureBackend, name: string, query = "") {
  const s = sessionByName(b, name);
  const r = await renderApp(`/session/${s.host}/${s.id}${query}`, b);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}
const part = () => screen.findByTestId("earlier-part");
const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>(".msg-user, .msg-ai, .tool, .subag, .think")];
const snap = (b: FixtureBackend, s: Ref) => b.snapshot(s.host, s.id);

describe("a resumed conversation shows its earlier messages above the session's own", () => {
  it("in the same scrolling list: the earlier part, then the divider, then the session's own rows", async () => {
    const b = backend();
    await open(b, COMPLETE);
    const earlier = await part();
    const divider = screen.getByTestId("earlier-divider");
    const own = await screen.findByText("Work on resumed-invoice-rounding in ledgerline (fix/invoice-rounding).");
    expect(divider).toHaveTextContent("Earlier in this conversation — before this session");
    expect(divider).toHaveAttribute("role", "separator");
    // Document order is the transcript's order, and all of it is one list.
    expect(earlier.compareDocumentPosition(divider) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(divider.compareDocumentPosition(own) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(earlier.closest(".tr")).toBe(divider.closest(".tr"));
    expect(own.closest(".tr")).toBe(divider.closest(".tr"));
    expect(earlier.closest(".transcript")!.querySelectorAll(".tr")).toHaveLength(1);
    // Nothing of the earlier conversation is in the session's own rows, nor the other way round.
    expect(within(earlier).queryByText(/Work on resumed-invoice-rounding/)).toBeNull();
    expect(within(earlier).getByText(/The invoice totals are off by a cent/)).toBeInTheDocument();
    expect(snap(b, sessionByName(b, COMPLETE)).earlier.state).toEqual({ kind: "loaded", complete: true });
  });

  it("through the same components: the user's messages with Markdown, assistant text, thinking, a collapsed run of tool calls, compaction", async () => {
    const user = userEvent.setup();
    await open(backend(), COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    const msg = within(earlier).getAllByText(/I'll look at where/)[0].closest(".msg-ai")!;
    expect(msg.querySelector("strong")).toHaveTextContent("totals");
    expect(within(earlier).getAllByText(/Thinking/).length).toBeGreaterThan(0);
    // Grep and Read are one run, as live ones are; opened, it is the calls.
    const run = within(earlier).getByTestId("tool-run");
    expect(run).toHaveTextContent("2 tool calls");
    await user.click(within(run).getByRole("button", { name: /2 tool calls/ }));
    expect(within(run).getByText("Grep")).toBeInTheDocument();
    expect(within(earlier).getByText(/History compacted at \d\d:\d\d \(auto\)/)).toBeInTheDocument();
    // A tool call's result opens as live ones do.
    await user.click(within(earlier).getByRole("button", { name: /Edit packages\/invoices\/src\/pdf\.ts/ }));
    expect(await within(earlier).findByTestId("tool-diff")).toBeInTheDocument();
  });

  it("shows the conversation's own times (`at_ms`), never when it was imported", async () => {
    const b = backend();
    const { s } = await open(b, COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    const items = b.earlier(s.host, s.id).items();
    const first = items.find((e) => e.kind === "agent" && e.event?.type === "history" && e.event.event.type === "user_message" && e.event.event.text.startsWith("The invoice totals"))!;
    const at = (first.kind === "agent" && first.event?.type === "history" ? first.event.at_ms : undefined) as number;
    const imported = first.t;
    expect(at).toBeLessThan(imported - 3_600_000);
    const msg = within(earlier).getByText(/The invoice totals are off by a cent/).closest(".msg-user")!;
    expect(within(msg as HTMLElement).getByTestId("msg-time")).toHaveTextContent(dayTime(at));
    expect(msg).not.toHaveTextContent(dayTime(imported));
    // The session's own rows show none.
    const own = (await screen.findByText(/^Work on resumed-invoice-rounding/)).closest(".msg-user")!;
    expect(within(own as HTMLElement).queryByTestId("msg-time")).toBeNull();
  });

  it("is read only: no delivery state on its messages, and every message can be copied", async () => {
    await open(backend(), COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    expect(within(earlier).queryByTestId("delivery")).toBeNull();
    expect(within(earlier).queryByText(/Not sent|Queued/)).toBeNull();
    for (const m of earlier.querySelectorAll(".msg-user")) expect(m).toHaveAttribute("data-delivery", "sent");
    const user = within(earlier).getByText(/The invoice totals are off by a cent/).closest(".msg-user")!;
    expect(within(user as HTMLElement).getByRole("button", { name: "Copy prompt" })).toBeInTheDocument();
    const ai = within(earlier).getAllByText(/Fixed: totals are rounded once/)[0].closest(".msg-ai")!;
    expect(within(ai as HTMLElement).getByRole("button", { name: "Copy response" })).toBeInTheDocument();
  });

  it("keeps a subagent's lines out of the list and reaches them the way a live subagent's are reached: its own transcript, with its earlier lines above its live part", async () => {
    const user = userEvent.setup();
    const b = backend();
    const { history } = await open(b, COMPLETE);
    const earlier = await part();
    expect(within(earlier).queryByText(/Three callers: the invoice list/)).toBeNull();
    const card = await within(earlier).findByTestId("subagent");
    expect(card).toHaveTextContent("Subagent · Explore");
    expect(card).toHaveTextContent("Find every caller of total()");
    await user.click(within(card).getByTestId("subagent-open"));
    await waitFor(() => expect(history.store.get()).toMatch(/\?.*sub=h-sub/));
    const sub = await screen.findByTestId("earlier-part");
    expect(await within(sub).findByText("Three callers: the invoice list, the PDF and the CSV export.")).toBeInTheDocument();
    expect(within(sub).getByText(/Searching for callers of/)).toBeInTheDocument();
    // Below the divider is what the session itself has of this subagent: nothing, and it says so.
    const divider = screen.getByTestId("earlier-divider");
    expect(divider.compareDocumentPosition(screen.getByTestId("transcript-empty")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(sub).queryByText(/The invoice totals are off by a cent/)).toBeNull();
    // The subagent's own transcript says it is read only, as ever.
    expect(screen.getByTestId("subagent-readonly")).toBeInTheDocument();
  });
});

describe("the earlier conversation while it is loading", () => {
  it("says so where the rest will appear, with the count so far, and shows what has arrived", async () => {
    const b = backend();
    const { s } = await open(b, LOADING);
    const earlier = await part();
    const joint = await within(earlier).findByTestId("earlier-joint");
    expect(joint).toHaveTextContent("Loading the earlier conversation… 40 items so far");
    expect(joint).toHaveAttribute("data-tone", "loading");
    await waitFor(() => expect(within(earlier).getAllByText(/Round \d+: /).length).toBeGreaterThan(0));
    // The notice sits below the items (where the newest are still to come), above the divider.
    const last = rows(earlier).pop()!;
    expect(last.compareDocumentPosition(joint) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(joint.compareDocumentPosition(screen.getByTestId("earlier-divider")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The session is usable meanwhile: nothing here blocks the composer.
    expect(screen.getByRole("textbox")).toBeEnabled();
    void s;
  });

  it("follows each record, and when it has all arrived the notice goes and what was left out is said at the top", async () => {
    const b = backend();
    const { s } = await open(b, LOADING);
    const handle = b.earlier(s.host, s.id);
    const earlier = await part();
    await within(earlier).findByText("Loading the earlier conversation… 40 items so far");
    act(() => void handle.step());
    await within(earlier).findByText("Loading the earlier conversation… 80 items so far");
    act(() => void handle.step());
    await within(earlier).findByText("Loading the earlier conversation… 120 items so far");
    expect(within(earlier).queryByTestId("earlier-gaps")).toBeNull();
    act(() => void handle.step());
    await waitFor(() => expect(within(earlier).queryByTestId("earlier-joint")).toBeNull());
    const gaps = within(earlier).getByTestId("earlier-gaps");
    expect(gaps).toHaveTextContent("120 older messages exist in the agent's own history and are not shown.");
    expect(gaps).toHaveTextContent("2 items were too large to keep in full and are shortened, or have images or files left out.");
    expect(gaps).toHaveTextContent("3 records were not understood (queue-operation, file-history-snapshot) and are missing.");
    // The gaps come before the first message.
    expect(gaps.compareDocumentPosition(rows(earlier)[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("a history that arrives whole says nothing extra", async () => {
    await open(backend(), COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    expect(within(earlier).queryByTestId("earlier-gaps")).toBeNull();
    expect(within(earlier).queryByTestId("earlier-joint")).toBeNull();
  });
});

describe("the earlier conversation that cannot be had", () => {
  it("says why where it would be, never an empty transcript without a word", async () => {
    await open(backend(), MISSING);
    const earlier = await part();
    expect(within(earlier).getByTestId("earlier-joint")).toHaveTextContent("The agent's file for this conversation could not be found, so the earlier conversation cannot be shown.");
    expect(within(earlier).getByTestId("earlier-joint")).toHaveAttribute("data-tone", "unavailable");
    expect(screen.getByTestId("earlier-divider")).toBeInTheDocument();
    expect(rows(earlier)).toHaveLength(0);
  });

  it("an import that failed after some of it arrived keeps what arrived and says why it stopped", async () => {
    const b = backend();
    const { s } = await open(b, LOADING);
    const earlier = await part();
    await within(earlier).findByText("Loading the earlier conversation… 40 items so far");
    act(() => b.earlier(s.host, s.id).fail({ kind: "source_unreadable", detail: "permission denied" }));
    await waitFor(() => expect(within(earlier).getByTestId("earlier-joint")).toHaveTextContent("could not be read (permission denied), so the earlier conversation cannot be shown. 40 items arrived before that and are shown."));
    expect(rows(earlier).length).toBeGreaterThan(0);
  });

  it("a request for the items that fails says so, with the daemon's words, and can be tried again", async () => {
    const user = userEvent.setup();
    const b = backend();
    let failing = true;
    const request = b.request.bind(b);
    b.request = (<T,>(method: string, params?: unknown, opts?: object) =>
      method === "session.history" && (params as { earlier?: boolean }).earlier && failing ? Promise.reject({ code: "io", message: "the link dropped" }) : request<T>(method, params, opts)) as typeof b.request;
    await open(b, COMPLETE);
    const earlier = await part();
    const alert = await within(earlier).findByTestId("earlier-error");
    expect(alert).toHaveTextContent("The earlier conversation could not be loaded: the link dropped");
    failing = false;
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    expect(within(earlier).queryByTestId("earlier-error")).toBeNull();
  });
});

describe("a session that began its conversation here", () => {
  it("shows nothing extra: no divider, no notice", async () => {
    await open(backend(), "csv-schema-worker");
    await waitFor(() => expect(document.querySelector(".tr .msg-user")).not.toBeNull());
    expect(screen.queryByTestId("earlier-part")).toBeNull();
    expect(screen.queryByTestId("earlier-divider")).toBeNull();
  });
});

describe("Load earlier messages: one control, up through the session and on into the earlier conversation", () => {
  it("pages the session's own events first (the earlier conversation does not appear above a gap), then the earlier conversation", async () => {
    const user = userEvent.setup();
    const b = backend([LONG]);
    await open(b, LONG.name);
    const load = () => screen.getByRole("button", { name: "Load earlier messages" });
    await waitFor(() => expect(load()).toBeInTheDocument());
    expect(screen.queryByTestId("earlier-divider")).toBeNull();
    for (let i = 0; i < 12 && !screen.queryByTestId("earlier-divider"); i++) {
      await user.click(load());
      await waitFor(() => expect(screen.queryByRole("button", { name: "Load earlier messages" }) === null || !screen.getByRole("button", { name: "Load earlier messages" }).hasAttribute("disabled")).toBe(true));
    }
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    // The control goes on up through the earlier conversation.
    const oldest = () => rows(earlier)[0].textContent;
    const was = oldest();
    await user.click(load());
    await waitFor(() => expect(oldest()).not.toBe(was));
    // Until the earlier conversation is all here: then there is no control.
    for (let i = 0; i < 6 && screen.queryByRole("button", { name: "Load earlier messages" }); i++) {
      await user.click(screen.getByRole("button", { name: "Load earlier messages" }));
      await waitFor(() => expect(b.requests.filter((r) => r.method === "session.history").length).toBeGreaterThan(0));
    }
    await waitFor(() => expect(screen.queryByRole("button", { name: "Load earlier messages" })).toBeNull());
    expect(within(earlier).getAllByText(/^Round 1: please look at module 0/).length).toBe(1);
  });
});

describe("recalling earlier prompts with the up arrow", () => {
  it("walks back through the session's own prompts and on through the earlier conversation's, newest first", async () => {
    const user = userEvent.setup();
    await open(backend(), COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    const box = screen.getByRole("textbox");
    await user.click(box);
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("Work on resumed-invoice-rounding in ledgerline (fix/invoice-rounding).");
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("The invoice totals are off by a cent on multi-currency invoices. Find where we round and fix it.");
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("Round 28: now check the tests for module 27.");
    // And back down to the draft.
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(box).toHaveValue("");
  });

  it("pages in the earlier conversation's prompts that are not loaded yet, in the background", async () => {
    const user = userEvent.setup();
    const b = backend();
    await open(b, COMPLETE);
    const earlier = await part();
    await within(earlier).findByText(/The invoice totals are off by a cent/);
    const box = screen.getByRole("textbox");
    await user.click(box);
    // 28 rounds + the tail's message + the session's own: the oldest is further back than the first page holds. (A key pressed while
    // an older page is being read is not used: the next one is, once the prompt it came for is in the box.)
    for (let i = 0; i < 30; i++) {
      const was = (box as HTMLTextAreaElement).value;
      await user.keyboard("{ArrowUp}");
      await waitFor(() => expect((box as HTMLTextAreaElement).value).not.toBe(was));
    }
    expect(box).toHaveValue("Round 1: please look at module 0 and tidy the **imports**.");
    expect(b.requests.filter((r) => r.method === "session.history" && (r.params as { earlier?: boolean; before?: number }).earlier && (r.params as { before?: number }).before !== undefined).length).toBeGreaterThan(0);
  });
});
