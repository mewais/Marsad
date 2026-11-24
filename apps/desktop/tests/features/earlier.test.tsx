import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { EARLIER_SPECS } from "../../src/bridge/fixture/earlier";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { SPECS } from "../../src/bridge/fixture/sessions";
import { dayTime } from "../../src/lib/format";
import { quietBackend, renderApp, sessionByName } from "../helpers";
import { holdHistory } from "../historyGate";
import { layoutTranscript, reach, toTop, transcript } from "../transcriptLayout";

const COMPLETE = "resumed-invoice-rounding";
const LOADING = "resumed-stripe-webhook";
const MISSING = "resumed-orphan-notes";
const LONG = { ...EARLIER_SPECS[0], n: 60, name: "resumed-long", longHistory: true };

const backend = (extra: typeof SPECS = []) => quietBackend({ specs: [...SPECS, ...EARLIER_SPECS, ...extra] });

// The transcript is one virtual list: rows near its viewport are mounted, so a test scrolls to the rows it looks at.
let unlayout: () => void;
beforeEach(() => {
  unlayout = layoutTranscript();
});
afterEach(() => unlayout());

type Ref = ReturnType<typeof sessionByName>;
async function open(b: FixtureBackend, name: string, query = "") {
  const s = sessionByName(b, name);
  const r = await renderApp(`/session/${s.host}/${s.id}${query}`, b);
  await screen.findByRole("heading", { name });
  // A complete earlier history arrives after the session opens. Its rows move the reader's place (the list keeps the first row on
  // screen where it is) while they come in, so a test looks for rows only once it has all arrived.
  if (name === COMPLETE) await waitFor(() => expect(b.snapshot(s.host, s.id).earlier.state).toEqual({ kind: "loaded", complete: true }));
  return { ...r, s };
}

/** Every mounted row of the earlier conversation is inside a `data-part="earlier"` entry; these find within those entries. */
const inEarlier = (el: Element) => !!el.closest('[data-part="earlier"]');
const earlierWraps = () => [...document.querySelectorAll<HTMLElement>('[data-part="earlier"]')];
const earlierByText = (re: RegExp) => screen.queryAllByText(re).filter(inEarlier);
const earlierByTestId = (id: string) => screen.queryAllByTestId(id).filter(inEarlier);
const earlierByRole = (name: string | RegExp) => screen.queryAllByRole("button", { name }).filter(inEarlier);
const rows = () => earlierWraps().flatMap((w) => [...w.querySelectorAll<HTMLElement>(".msg-user, .msg-ai, .tool, .subag, .think")]);
/** The earlier conversation's text, scrolled to. */
const earlierText = (re: RegExp) => reach(() => earlierByText(re)[0] ?? null);
const part = () => reach(() => screen.queryByTestId("earlier-part"));
const divider = (dir: -1 | 1 = 1) => reach(() => screen.queryByTestId("earlier-divider"), dir);
const snap = (b: FixtureBackend, s: Ref) => b.snapshot(s.host, s.id);

describe("a resumed conversation shows its earlier messages above the session's own", () => {
  it("in the same scrolling list: the earlier part, then the divider, then the session's own rows", async () => {
    const b = backend();
    await open(b, COMPLETE);
    const earlier = await part();
    // The top of the earlier part, the divider and the session's own rows are all in the transcript's one list.
    expect(earlier.closest(".transcript")).toBe(transcript());
    expect(document.querySelectorAll(".transcript")).toHaveLength(1);
    const div = await divider();
    expect(div).toHaveTextContent("Earlier in this conversation — before this session");
    expect(div).toHaveAttribute("role", "separator");
    // Virtualised, the top and the divider are rarely mounted together: the order is checked on the rows either side of it.
    expect(rows().pop()!.compareDocumentPosition(div) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const own = await reach(() => screen.queryByText("Work on resumed-invoice-rounding in ledgerline (fix/invoice-rounding)."), 1);
    expect(div.compareDocumentPosition(own) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(own.closest(".transcript")).toBe(transcript());
    // Nothing of the earlier conversation is in the session's own rows, nor the other way round.
    expect(await earlierText(/The invoice totals are off by a cent/)).toBeInTheDocument();
    expect(earlierByText(/Work on resumed-invoice-rounding/)).toHaveLength(0);
    expect(snap(b, sessionByName(b, COMPLETE)).earlier.state).toEqual({ kind: "loaded", complete: true });
  });

  it("through the same components: the user's messages with Markdown, assistant text, thinking, a collapsed run of tool calls, compaction", async () => {
    const user = userEvent.setup();
    await open(backend(), COMPLETE);
    const msg = (await earlierText(/I'll look at where/)).closest(".msg-ai")!;
    expect(msg.querySelector("strong")).toHaveTextContent("totals");
    expect(await earlierText(/Thinking/)).toBeInTheDocument();
    // Grep and Read are one run, as live ones are; opened, it is the calls.
    const run = await reach(() => earlierByTestId("tool-run")[0] ?? null);
    expect(run).toHaveTextContent("2 tool calls");
    await user.click(within(run).getByRole("button", { name: /2 tool calls/ }));
    expect(within(run).getByText("Grep")).toBeInTheDocument();
    expect(await earlierText(/History compacted at \d\d:\d\d \(auto\)/)).toBeInTheDocument();
    // A tool call's result opens as live ones do.
    await user.click(await reach(() => earlierByRole(/Edit packages\/invoices\/src\/pdf\.ts/)[0] ?? null));
    await waitFor(() => expect(earlierByTestId("tool-diff")).toHaveLength(1));
  });

  it("shows the conversation's own times (`at_ms`), never when it was imported", async () => {
    const b = backend();
    const { s } = await open(b, COMPLETE);
    const msgText = await earlierText(/The invoice totals are off by a cent/);
    const items = b.earlier(s.host, s.id).items();
    const first = items.find((e) => e.kind === "agent" && e.event?.type === "history" && e.event.event.type === "user_message" && e.event.event.text.startsWith("The invoice totals"))!;
    const at = (first.kind === "agent" && first.event?.type === "history" ? first.event.at_ms : undefined) as number;
    const imported = first.t;
    expect(at).toBeLessThan(imported - 3_600_000);
    const msg = msgText.closest(".msg-user")!;
    expect(within(msg as HTMLElement).getByTestId("msg-time")).toHaveTextContent(dayTime(at));
    expect(msg).not.toHaveTextContent(dayTime(imported));
    // The session's own rows show when they happened (their event's time), not the history's.
    const own = (await reach(() => screen.queryByText(/^Work on resumed-invoice-rounding/), 1)).closest(".msg-user")!;
    expect(within(own as HTMLElement).getByTestId("msg-time")).toHaveTextContent(/\d/);
    expect(within(own as HTMLElement).getByTestId("msg-time")).not.toHaveTextContent(dayTime(at));
  });

  it("is read only: no delivery state on its messages, and every message can be copied", async () => {
    await open(backend(), COMPLETE);
    const msgText = await earlierText(/The invoice totals are off by a cent/);
    expect(earlierByTestId("delivery")).toHaveLength(0);
    expect(earlierByText(/Not sent|Queued/)).toHaveLength(0);
    for (const m of earlierWraps().flatMap((w) => [...w.querySelectorAll(".msg-user")])) expect(m).toHaveAttribute("data-delivery", "sent");
    const user = msgText.closest(".msg-user")!;
    expect(within(user as HTMLElement).getByRole("button", { name: "Copy prompt" })).toBeInTheDocument();
    const ai = (await earlierText(/Fixed: totals are rounded once/)).closest(".msg-ai")!;
    expect(within(ai as HTMLElement).getByRole("button", { name: "Copy response" })).toBeInTheDocument();
  });

  it("keeps a subagent's lines out of the list and reaches them the way a live subagent's are reached: its own transcript, with its earlier lines above its live part", async () => {
    const user = userEvent.setup();
    const b = backend();
    const { history } = await open(b, COMPLETE);
    await part();
    expect(earlierByText(/Three callers: the invoice list/)).toHaveLength(0);
    const card = await reach(() => earlierByTestId("subagent")[0] ?? null);
    expect(card).toHaveTextContent("Subagent · Explore");
    expect(card).toHaveTextContent("Find every caller of total()");
    await user.click(within(card).getByTestId("subagent-open"));
    await waitFor(() => expect(history.store.get()).toMatch(/\?.*sub=h-sub/));
    const sub = await reach(() => screen.queryByTestId("earlier-part"));
    expect(await earlierText(/Three callers: the invoice list, the PDF and the CSV export\./)).toBeInTheDocument();
    expect(earlierByText(/Searching for callers of/).length).toBeGreaterThan(0);
    // Below the divider is what the session itself has of this subagent: nothing, and it says so.
    const div = await divider();
    const empty = await reach(() => screen.queryByTestId("transcript-empty"), 1);
    expect(div.compareDocumentPosition(empty) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(earlierByText(/The invoice totals are off by a cent/)).toHaveLength(0);
    expect(sub).toBeInTheDocument();
    // The subagent's own transcript says it is read only, as ever.
    expect(screen.getByTestId("subagent-readonly")).toBeInTheDocument();
  });
});

describe("the earlier conversation while it is loading", () => {
  it("says so where the rest will appear, with the count so far, and shows what has arrived", async () => {
    await open(backend(), LOADING);
    const joint = await reach(() => screen.queryByTestId("earlier-joint"));
    expect(joint).toHaveTextContent("Loading the earlier conversation… 40 items so far");
    expect(joint).toHaveAttribute("data-tone", "loading");
    await earlierText(/Round \d+: /);
    // The notice sits below the items (where the newest are still to come), above the divider.
    const last = rows().pop()!;
    expect(last.compareDocumentPosition(joint) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(joint.compareDocumentPosition(screen.getByTestId("earlier-divider")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The session is usable meanwhile: nothing here blocks the composer.
    expect(screen.getByRole("textbox")).toBeEnabled();
  });

  it("follows each record, and when it has all arrived the notice goes and what was left out is said at the top", async () => {
    const b = backend();
    const { s } = await open(b, LOADING);
    const handle = b.earlier(s.host, s.id);
    await reach(() => screen.queryByText("Loading the earlier conversation… 40 items so far"));
    act(() => void handle.step());
    await reach(() => screen.queryByText("Loading the earlier conversation… 80 items so far"));
    act(() => void handle.step());
    await reach(() => screen.queryByText("Loading the earlier conversation… 120 items so far"));
    act(() => void handle.step());
    await waitFor(() => expect(screen.queryByTestId("earlier-joint")).toBeNull());
    await divider();
    expect(screen.queryByTestId("earlier-joint")).toBeNull();
    const top = await part();
    expect(screen.queryByTestId("earlier-gaps")).not.toBeNull();
    const gaps = screen.getByTestId("earlier-gaps");
    expect(gaps).toHaveTextContent("120 older messages exist in the agent's own history and are not shown.");
    expect(gaps).toHaveTextContent("2 items were too large to keep in full and are shortened, or have images or files left out.");
    expect(gaps).toHaveTextContent("3 records were not understood (queue-operation, file-history-snapshot) and are missing.");
    // The gaps come before the first message.
    expect(gaps.compareDocumentPosition(rows()[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(top).toContainElement(gaps);
  });

  it("a history that arrives whole says nothing extra", async () => {
    await open(backend(), COMPLETE);
    await earlierText(/The invoice totals are off by a cent/);
    await toTop();
    expect(screen.queryByTestId("earlier-gaps")).toBeNull();
    await divider();
    expect(screen.queryByTestId("earlier-joint")).toBeNull();
  });
});

describe("the earlier conversation that cannot be had", () => {
  it("says why where it would be, never an empty transcript without a word", async () => {
    await open(backend(), MISSING);
    const joint = await reach(() => screen.queryByTestId("earlier-joint"));
    expect(joint).toHaveTextContent("The agent's file for this conversation could not be found, so the earlier conversation cannot be shown.");
    expect(joint).toHaveAttribute("data-tone", "unavailable");
    expect(screen.getByTestId("earlier-divider")).toBeInTheDocument();
    expect(earlierWraps()).toHaveLength(0);
    expect(rows()).toHaveLength(0);
  });

  it("an import that failed after some of it arrived keeps what arrived and says why it stopped", async () => {
    const b = backend();
    const { s } = await open(b, LOADING);
    await reach(() => screen.queryByText("Loading the earlier conversation… 40 items so far"));
    act(() => b.earlier(s.host, s.id).fail({ kind: "source_unreadable", detail: "permission denied" }));
    await waitFor(() => expect(screen.getByTestId("earlier-joint")).toHaveTextContent("could not be read (permission denied), so the earlier conversation cannot be shown. 40 items arrived before that and are shown."));
    expect(rows().length).toBeGreaterThan(0);
  });

  it("a request for the items that fails says so, with the daemon's words, and can be tried again", async () => {
    const user = userEvent.setup();
    const b = backend();
    let failing = true;
    const request = b.request.bind(b);
    b.request = (<T,>(method: string, params?: unknown, opts?: object) =>
      method === "session.history" && (params as { earlier?: boolean }).earlier && failing ? Promise.reject({ code: "io", message: "the link dropped" }) : request<T>(method, params, opts)) as typeof b.request;
    await open(b, COMPLETE);
    await part();
    const alert = await reach(() => screen.queryByTestId("earlier-error"));
    expect(alert).toHaveTextContent("The earlier conversation could not be loaded: the link dropped");
    failing = false;
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    await earlierText(/The invoice totals are off by a cent/);
    await toTop();
    expect(screen.queryByTestId("earlier-error")).toBeNull();
  });
});

describe("a session that began its conversation here", () => {
  it("shows nothing extra: no divider, no notice", async () => {
    await open(backend(), "csv-schema-worker");
    await waitFor(() => expect(document.querySelector('[data-part="live"] .msg-user')).not.toBeNull());
    await toTop();
    expect(screen.queryByTestId("earlier-part")).toBeNull();
    expect(screen.queryByTestId("earlier-divider")).toBeNull();
  });
});

describe("Load earlier messages: one control, up through the session and on into the earlier conversation", () => {
  it("pages the session's own events first (the earlier conversation does not appear above a gap), then the earlier conversation", async () => {
    const user = userEvent.setup();
    const b = backend([LONG]);
    // The first page passes at once; every later page is held until the test releases it.
    const gate = holdHistory(b, 1);
    await open(b, LONG.name);
    const load = () => screen.queryByRole("button", { name: "Load earlier messages" });
    await reach(load);
    const asked = () => [...gate.answered().map((a) => a.params), ...gate.pending()];
    const dividerNow = () => screen.queryByTestId("earlier-divider");
    for (;;) {
      await waitFor(() => expect(gate.pending()).toHaveLength(1));
      const next = gate.pending()[0];
      if (next.earlier) break;
      // The session's own events are not all paged in yet: no divider, and the earlier conversation is not asked for.
      expect(dividerNow()).toBeNull();
      expect(asked().some((p) => p.earlier)).toBe(false);
      await gate.release();
      await waitFor(() => expect(gate.pending()[0]).not.toBe(next));
    }
    // The session's own events reached the start: only now is the earlier conversation asked for (the divider was absent at every own page above).
    expect(gate.pending()[0].earlier).toBe(true);
    await gate.release();
    await divider();
    const order = asked().map((p) => (p.earlier ? "earlier" : "live"));
    expect(order[order.length - 1]).toBe("earlier");
    expect(order.slice(0, -1).every((k) => k === "live")).toBe(true);
    await gate.open();
    await earlierText(/The invoice totals are off by a cent/);
    // Manual paging still steps through older history within the earlier conversation.
    await toTop();
    const oldest = () => rows()[0].textContent;
    const was = oldest();
    await user.click((await reach(load)));
    await waitFor(() => expect(oldest()).not.toBe(was));
    for (let i = 0; i < 6 && load(); i++) {
      await user.click(await reach(load));
      await waitFor(() => expect(b.requests.filter((r) => r.method === "session.history" && (r.params as { before?: number }).before !== undefined).length).toBeGreaterThan(0));
    }
    await toTop();
    await waitFor(() => expect(load()).toBeNull());
    expect(earlierByText(/^Round 1: please look at module 0/)).toHaveLength(1);
  });
});

describe("recalling earlier prompts with the up arrow", () => {
  it("walks back through the session's own prompts and on through the earlier conversation's, newest first", async () => {
    const user = userEvent.setup();
    await open(backend(), COMPLETE);
    await earlierText(/The invoice totals are off by a cent/);
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
    await earlierText(/The invoice totals are off by a cent/);
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

