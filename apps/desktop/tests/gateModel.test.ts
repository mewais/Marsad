/**
 * gateModel — unit tests for the pure normalization + response→api mapping that
 * powers the unified GateOverlay. No React here: normalizeGate/responseToAnswer
 * are pure (the latter fires an api call, which we mock).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { normalizeGate, responseToAnswer, submitAnswers, suggestionLabel } from "../src/bridge/gateModel";
import type { Gate, GateVM } from "../src/bridge/types";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    answerGate: vi.fn().mockResolvedValue(undefined),
    answerQuestion: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.restoreAllMocks());

// ---------------------------------------------------------------------------
// GateVM builders (minimal — normalizeGate only reads lead/diff_rows/kind)
// ---------------------------------------------------------------------------

function gv(over: Partial<GateVM> = {}): GateVM {
  return {
    kind: "permission",
    icon: "⌘",
    lead: "Permission request",
    summary: "",
    decision_id: "dec-1",
    is_edit: false,
    diff_rows: [],
    ...over,
  };
}

// ---------------------------------------------------------------------------
// suggestionLabel
// ---------------------------------------------------------------------------

describe("suggestionLabel", () => {
  it("uses the ACP `name` field when present", () => {
    expect(suggestionLabel({ optionId: "a", name: "Allow once" } as never)).toBe(
      "Allow once"
    );
  });

  it("builds behavior + rules for a claude suggestion", () => {
    expect(
      suggestionLabel({
        behavior: "allow",
        rules: [{ toolName: "Bash", ruleContent: "git push *" }],
      })
    ).toBe("allow git push *");
  });

  it("falls back to bare behavior when there are no rules", () => {
    expect(suggestionLabel({ behavior: "allow" })).toBe("allow");
    expect(suggestionLabel({})).toBe("allow");
  });
});

// ---------------------------------------------------------------------------
// normalizeGate — PERMISSION (claude)
// ---------------------------------------------------------------------------

describe("normalizeGate — claude permission", () => {
  const SUGGESTION = {
    type: "addRules",
    behavior: "allow",
    destination: "localSettings",
    rules: [{ toolName: "Bash", ruleContent: "git push *" }],
  };

  function permRaw(suggestions: unknown[] = []): Gate {
    return {
      Permission: {
        request_id: "req-1",
        command: "prisma migrate deploy",
        cwd: "/app",
        risk: "medium",
        explanation: "Runs migrations.",
        is_elicitation: false,
        permission_suggestions: suggestions as never,
      },
    };
  }

  it("body/risk/command attachment come from the payload", () => {
    const n = normalizeGate(gv(), permRaw());
    expect(n.kind).toBe("permission");
    expect(n.body).toBe("Runs migrations.");
    expect(n.risk).toBe("medium");
    expect(n.attachments).toEqual([
      { kind: "command", command: "prisma migrate deploy", cwd: "/app" },
    ]);
    expect(n.mode).toBe("pick-one");
  });

  it("no command attachment when command is blank", () => {
    const raw = permRaw();
    (raw as { Permission: { command: string } }).Permission.command = "  ";
    const n = normalizeGate(gv(), raw);
    expect(n.attachments).toEqual([]);
  });

  it("with no suggestions: options are Approve + Reject only", () => {
    const n = normalizeGate(gv(), permRaw([]));
    const opts = n.prompts[0].options;
    expect(opts.map((o) => o.testid)).toEqual(["approve-btn", "reject-btn"]);
    expect(opts[0].response).toEqual({ type: "allow" });
    expect(opts[1].response).toEqual({ type: "deny" });
    expect(opts[1].freeText).toBeTruthy();
  });

  it("with suggestions: verbatim rule + Approve + Reject", () => {
    const n = normalizeGate(gv(), permRaw([SUGGESTION]));
    const opts = n.prompts[0].options;
    expect(opts[0].testid).toBe("approve-suggestion-0");
    expect(opts[0].label).toBe("allow git push *");
    expect(opts[0].response).toEqual({ type: "permissionRule", suggestion: SUGGESTION });
    expect(opts[1].testid).toBe("approve-btn");
    expect(opts[2].testid).toBe("reject-btn");
  });
});

// ---------------------------------------------------------------------------
// normalizeGate — PERMISSION (ACP)
// ---------------------------------------------------------------------------

describe("normalizeGate — ACP permission", () => {
  const ACP = [
    { optionId: "a", name: "Allow once", kind: "allow_once" },
    { optionId: "b", name: "Always allow", kind: "allow_always" },
    { optionId: "c", name: "Reject", kind: "reject_once" },
  ];

  function acpRaw(): Gate {
    return {
      Permission: {
        request_id: "req-acp",
        command: "",
        cwd: "/x",
        risk: "",
        explanation: "opencode wants to read a file",
        is_elicitation: false,
        permission_suggestions: ACP as never,
      },
    };
  }

  it("options are the ACP options verbatim, no synth Allow/Deny", () => {
    const n = normalizeGate(gv(), acpRaw());
    const opts = n.prompts[0].options;
    expect(opts).toHaveLength(3);
    expect(opts.map((o) => o.label)).toEqual(["Allow once", "Always allow", "Reject"]);
    expect(opts.map((o) => o.testid)).toEqual([
      "approve-suggestion-0",
      "approve-suggestion-1",
      "approve-suggestion-2",
    ]);
    expect(opts[0].response).toEqual({ type: "acpOption", optionId: "a" });
    // No synthesized approve-btn / reject-btn.
    expect(opts.some((o) => o.testid === "approve-btn")).toBe(false);
    expect(opts.some((o) => o.testid === "reject-btn")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// normalizeGate — PERMISSION (elicitation)
// ---------------------------------------------------------------------------

describe("normalizeGate — elicitation permission", () => {
  function elicRaw(): Gate {
    return {
      Permission: {
        request_id: "req-elic",
        command: "",
        cwd: "/x",
        risk: "",
        explanation: "Grant path access?",
        is_elicitation: true,
        elicitation_choices: [
          { key: "grant", value: "once", label: "Just this once" },
          { key: "grant", value: "session", label: "For this session" },
        ],
      },
    };
  }

  it("one option per choice + a Deny, no generic Allow", () => {
    const n = normalizeGate(gv(), elicRaw());
    const opts = n.prompts[0].options;
    expect(opts).toHaveLength(3);
    expect(opts[0].testid).toBe("gate-choice-once");
    expect(opts[0].response).toEqual({ type: "elicitation", key: "grant", value: "once" });
    expect(opts[1].testid).toBe("gate-choice-session");
    expect(opts[2].testid).toBe("reject-btn");
    expect(opts.some((o) => o.testid === "approve-btn")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// normalizeGate — PLAN / EDIT
// ---------------------------------------------------------------------------

describe("normalizeGate — plan", () => {
  it("body is the summary, options Approve plan + Keep planning", () => {
    const raw: Gate = {
      Plan: { request_id: "req-p", summary: "# Plan\ndo things", steps: [] },
    };
    const n = normalizeGate(gv({ kind: "plan", lead: "Plan for approval" }), raw);
    expect(n.kind).toBe("plan");
    expect(n.body).toBe("# Plan\ndo things");
    const opts = n.prompts[0].options;
    expect(opts[0].label).toBe("Approve plan");
    expect(opts[0].testid).toBe("approve-btn");
    expect(opts[1].label).toBe("Keep planning");
    expect(opts[1].testid).toBe("reject-btn");
    expect(opts[1].freeText).toBeTruthy();
  });
});

describe("normalizeGate — edit", () => {
  it("carries a diff attachment + Approve change / Reject", () => {
    const raw: Gate = {
      Edit: { request_id: "req-e", file: "src/x.ts", diff: [] },
    };
    const rows = [{ l: {} as never, r: {} as never }];
    const n = normalizeGate(
      gv({ kind: "edit", lead: "File edit", is_edit: true, diff_rows: rows as never }),
      raw
    );
    expect(n.attachments).toEqual([
      { kind: "diff", rows, filename: "src/x.ts" },
    ]);
    const opts = n.prompts[0].options;
    expect(opts[0].label).toBe("Approve change");
    expect(opts[1].testid).toBe("reject-btn");
  });
});

// ---------------------------------------------------------------------------
// normalizeGate — QUESTION
// ---------------------------------------------------------------------------

describe("normalizeGate — question", () => {
  function qRaw(multi = false, two = false): Gate {
    const questions = [
      {
        question: "Which DB?",
        header: "Storage",
        multi_select: multi,
        options: [
          { label: "Postgres", description: "relational" },
          { label: "MySQL", description: "" },
        ],
      },
    ];
    if (two) {
      questions.push({
        question: "Which cache?",
        header: "Cache",
        multi_select: false,
        options: [{ label: "Redis", description: "" }],
      });
    }
    return { Question: { request_id: "req-q", questions } };
  }

  it("single question: collect mode, options + Other, submit/skip", () => {
    const n = normalizeGate(gv({ kind: "question" }), qRaw());
    expect(n.mode).toBe("collect");
    expect(n.prompts).toHaveLength(1);
    const opts = n.prompts[0].options;
    expect(opts.map((o) => o.label)).toEqual(["Postgres", "MySQL", "Other…"]);
    expect(opts[0].response).toEqual({ type: "answer", question: "Which DB?", label: "Postgres" });
    // Other carries freeText + an empty label.
    expect(opts[2].freeText).toBeTruthy();
    expect(opts[2].response).toEqual({ type: "answer", question: "Which DB?", label: "" });
    expect(n.submit?.label).toBe("Submit");
    expect(n.skip?.testid).toBe("question-skip-btn");
    expect(n.skip?.response).toEqual({ type: "skipQuestions" });
  });

  it("multi-select carries multiSelect:true on the prompt", () => {
    const n = normalizeGate(gv({ kind: "question" }), qRaw(true));
    expect(n.prompts[0].multiSelect).toBe(true);
  });

  it("multiple questions: one prompt each", () => {
    const n = normalizeGate(gv({ kind: "question" }), qRaw(false, true));
    expect(n.prompts).toHaveLength(2);
    expect(n.prompts[0].question).toBe("Which DB?");
    expect(n.prompts[1].question).toBe("Which cache?");
  });
});

// ---------------------------------------------------------------------------
// responseToAnswer — the response → api mapping
// ---------------------------------------------------------------------------

describe("responseToAnswer", () => {
  const S = "s01";
  const D = "dec-1";
  const R = "req-1";

  it("allow → answerGate allow:true", async () => {
    await responseToAnswer(S, D, R, { type: "allow" });
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: true,
    });
  });

  it("deny → answerGate allow:false + deny_reason", async () => {
    await responseToAnswer(S, D, R, { type: "deny" }, "Too risky");
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: false,
      deny_reason: "Too risky",
    });
  });

  it("deny with blank text → deny_reason undefined", async () => {
    await responseToAnswer(S, D, R, { type: "deny" }, "   ");
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: false,
      deny_reason: undefined,
    });
  });

  it("acpOption → answerGate allow:true + optionId inside permission_updates", async () => {
    // The optionId MUST travel inside permission_updates — the agent reads it from
    // permission_updates.optionId (the wire AnswerGate has no top-level optionId).
    await responseToAnswer(S, D, R, { type: "acpOption", optionId: "b" });
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: true,
      permission_updates: { optionId: "b" },
    });
  });

  it("elicitation → answerGate allow:true + elicitation_content", async () => {
    await responseToAnswer(S, D, R, { type: "elicitation", key: "grant", value: "session" });
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: true,
      elicitation_content: { grant: "session" },
    });
  });

  it("permissionRule → answerGate allow:true + permission_updates", async () => {
    const sug = { type: "addRules" };
    await responseToAnswer(S, D, R, { type: "permissionRule", suggestion: sug });
    expect(api.answerGate).toHaveBeenCalledWith({
      session_id: S,
      decision_id: D,
      allow: true,
      permission_updates: [sug],
    });
  });

  it("answer → answerQuestion with the single answer", async () => {
    await responseToAnswer(S, D, R, { type: "answer", question: "Q?", label: "A" });
    expect(api.answerQuestion).toHaveBeenCalledWith({
      session_id: S,
      request_id: R,
      answers: { "Q?": "A" },
    });
  });

  it("skipQuestions → answerQuestion with empty answers", async () => {
    await responseToAnswer(S, D, R, { type: "skipQuestions" });
    expect(api.answerQuestion).toHaveBeenCalledWith({
      session_id: S,
      request_id: R,
      answers: {},
    });
  });

  it("submitAnswers → answerQuestion with the aggregated answers", async () => {
    await submitAnswers(S, R, { "Which DB?": "Postgres", "Which cache?": "Redis" });
    expect(api.answerQuestion).toHaveBeenCalledWith({
      session_id: S,
      request_id: R,
      answers: { "Which DB?": "Postgres", "Which cache?": "Redis" },
    });
  });
});
