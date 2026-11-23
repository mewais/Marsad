import { describe, expect, it } from "vitest";
import { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { quietBackend, sessionByName } from "../helpers";

/** The fixture reports `continuable`, `not_continuable` and `resume_command` the way the runtime derives them (`continue_verdict`). */
describe("the fixture's conversation facts", () => {
  const snap = (b: FixtureBackend, name: string) => {
    const s = sessionByName(b, name);
    return { ...s, now: () => b.snapshot(s.host, s.id) };
  };

  it("a live session is not continuable, whatever its driver can do, and has a resume command only when a conversation is saved", () => {
    const b = quietBackend();
    for (const name of ["docs-search-index", "invoice-export-orchestrator", "release-notes-1-4"]) {
      const s = snap(b, name).now();
      expect(["starting", "idle", "working", "needs_you"]).toContain(s.phase.state);
      expect(s).toMatchObject({ continuable: false, not_continuable: "not_ended" });
    }
    // A saved conversation of a driver with a command in the user's own CLI: that command, run where the session ran.
    const claude = snap(b, "release-notes-1-4").now();
    expect(claude.resume_command).toEqual({ argv: ["claude", "--resume", claude.native_session_id], cwd: claude.cwd });
    // A driver whose CLI has no such command has none.
    expect(snap(b, "docs-search-index").now().resume_command).toBeNull();
  });

  it("an ended session is continuable when its driver can resume and a conversation is saved", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "release-notes-1-4");
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    const ended = b.snapshot(s.host, s.id);
    expect(ended.phase.state).not.toBe("idle");
    expect(ended).toMatchObject({ continuable: true, not_continuable: null });
    expect(ended.resume_command?.argv).toEqual(["claude", "--resume", ended.native_session_id]);
  });

  it("an ended session with nothing saved says so; one without a conversation id says that", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "release-notes-1-4");
    const e = (b as unknown as { hosts: Map<string, { sessions: Map<string, { snap: import("../../src/bridge/wire").SessionSnapshot }> }> }).hosts.get(s.host)!.sessions.get(s.id)!;
    // The agent reported an id but no turn ever ran (so it saved nothing).
    e.snap = { ...e.snap, turns_completed: 0, turn: null, phase: { state: "idle" } };
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    expect(b.snapshot(s.host, s.id)).toMatchObject({ continuable: false, not_continuable: "nothing_saved", resume_command: null });
    e.snap = { ...e.snap, native_session_id: null, phase: { state: "idle" } };
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    expect(b.snapshot(s.host, s.id)).toMatchObject({ continuable: false, not_continuable: "no_conversation_id" });
  });

  it("a session launched from the app is starting, so not continuable, and has no conversation to open yet", async () => {
    const b = quietBackend();
    const r = await b.request<{ id: string; host: string }>("sessions.launch", { host: "x1", driver: "claude", cwd: "/home/u/proj", model: null, mode: null, first_prompt: "hi", resume: { kind: "new" }, labels: {}, overrides: { items: [], env: [], disallowed_tools: [] } });
    expect(b.snapshot("x1", r.id)).toMatchObject({ continuable: false, not_continuable: "not_ended", resume_command: null });
  });
});
