/**
 * GateOverlay — full permission / plan / edit gate modal.
 * Approve or reject-with-reason. Routes back to the session on action.
 */
import { useState, useEffect } from "react";
import { useVm } from "../bridge/useVm";
import { answerGate, answerQuestion } from "../bridge/api";
import type { PermissionSuggestion } from "../bridge/types";
import { DiffView } from "../components/DiffView";
import { Markdown } from "../components/Markdown";
import { GateOptions, type GateOption } from "../components/GateOptions";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function approveLabel(kind: string): string {
  if (kind === "plan") return "Approve plan";
  if (kind === "edit") return "Approve change";
  if (kind === "question") return "Submit";
  return "Approve & run";
}

// The reject-with-reason button reads "Keep planning" for a plan gate (the
// design's wording — rejecting a plan asks the agent to keep planning, not
// to abandon the turn) and "Reject…" for everything else.
function rejectLabel(kind: string): string {
  if (kind === "plan") return "Keep planning";
  return "Reject…";
}

function kindBadge(kind: string): string {
  if (kind === "plan") return "Plan";
  if (kind === "edit") return "Edit";
  if (kind === "question") return "Question";
  return "Permission";
}

// Button text for one permission suggestion. ACP options carry a `name` field
// (verbatim from the agent) and no `behavior`/`rules`. Claude options carry
// `behavior` + `rules`. Use `name` when present (ACP path); fall back to the
// claude behavior+rules path so claude gates are never broken.
function suggestionLabel(s: PermissionSuggestion): string {
  // ACP options always have a `name`; claude options never do.
  if (typeof (s as Record<string, unknown>).name === "string") {
    return ((s as Record<string, unknown>).name as string).trim();
  }
  const behavior = (s.behavior ?? "allow").trim();
  const rules = (s.rules ?? [])
    .map((r) => r?.ruleContent?.trim())
    .filter((c): c is string => !!c);
  return rules.length ? `${behavior} ${rules.join(", ")}` : behavior;
}

// ---------------------------------------------------------------------------
// GateOverlay
// ---------------------------------------------------------------------------

export function GateOverlay() {
  const { selectedGate, selectedSession, state, dispatch } = useVm();

  const [rejectOpen, setRejectOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  // Multi-select state for question gates: maps question text → selected labels.
  const [multiSelected, setMultiSelected] = useState<Record<string, string[]>>({});

  const open = state.screen === "gate" && selectedGate !== null;

  // Close on Escape
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  function close() {
    // Return to the session that owns this gate
    if (selectedSession) {
      dispatch({ type: "SELECT_SESSION", id: selectedSession.id });
    } else {
      // Board is the home screen (Workstreams redesign SLICE A).
      dispatch({ type: "SET_SCREEN", screen: "board" });
    }
    setRejectOpen(false);
    setReason("");
  }

  async function handleApprove() {
    if (!selectedSession) return;
    setBusy(true);
    await answerGate({
      session_id: selectedSession.id,
      decision_id: selectedGate?.decision_id ?? selectedSession.id,
      allow: true,
    });
    setBusy(false);
    close();
  }

  // Accept a `can_use_tool` gate by picking one of claude's OWN
  // `permission_suggestions` ("always allow …"). We relay the human's answer
  // exactly like Approve & run, plus echo claude's chosen suggestion back
  // VERBATIM as updatedPermissions — the real, claude-provided way to stop being
  // asked, with NO session restart and nothing invented on our side.
  async function handleAllowSuggestion(suggestion: PermissionSuggestion) {
    if (!selectedSession) return;
    setBusy(true);
    const raw = suggestion as Record<string, unknown>;
    // ACP options carry `optionId` (no `behavior`/`rules`); surface it at
    // the top level so the backend can reply to the ACP request by optionId.
    // Claude options use the existing permission_updates path.
    if (typeof raw.optionId === "string") {
      await answerGate({
        session_id: selectedSession.id,
        decision_id: selectedGate?.decision_id ?? selectedSession.id,
        allow: true,
        optionId: raw.optionId,
      });
    } else {
      await answerGate({
        session_id: selectedSession.id,
        decision_id: selectedGate?.decision_id ?? selectedSession.id,
        allow: true,
        permission_updates: [suggestion],
      });
    }
    setBusy(false);
    close();
  }

  async function handleReject() {
    if (!selectedSession) return;
    setBusy(true);
    await answerGate({
      session_id: selectedSession.id,
      decision_id: selectedGate?.decision_id ?? selectedSession.id,
      allow: false,
      deny_reason: reason.trim() || undefined,
    });
    setBusy(false);
    close();
  }

  // Accept an MCP elicitation with a specific scope choice (e.g. grant=session).
  // The chosen key/value becomes the elicitation response `content`.
  async function handleElicitationChoice(key: string, value: string) {
    if (!selectedSession) return;
    setBusy(true);
    await answerGate({
      session_id: selectedSession.id,
      decision_id: selectedGate?.decision_id ?? selectedSession.id,
      allow: true,
      elicitation_content: { [key]: value },
    });
    setBusy(false);
    close();
  }

  if (!open || !selectedGate || !selectedSession) {
    // Render nothing visible — keep mount for test compatibility
    return (
      <div
        style={{ display: "none" }}
        data-testid="gate-overlay-hidden"
        aria-hidden="true"
      />
    );
  }

  const gate = selectedGate;
  const kind = gate.kind; // "permission" | "plan" | "edit"

  // Recover the raw gate payload from the session for rich display
  const raw = selectedSession.gate;
  const permissionData =
    raw && "Permission" in raw ? raw.Permission : null;
  // Scope-of-allowance choices offered by an MCP elicitation (once/session/deny…).
  // When present, they REPLACE the binary Approve/Reject with one button per choice.
  const elicitationChoices = permissionData?.elicitation_choices ?? [];
  const hasChoices = elicitationChoices.length > 0;
  // claude's OWN "always allow …" options for a can_use_tool gate, verbatim.
  const permissionSuggestions = permissionData?.permission_suggestions ?? [];
  const planData =
    raw && "Plan" in raw ? raw.Plan : null;
  const editData =
    raw && "Edit" in raw ? raw.Edit : null;
  const questionData =
    raw && "Question" in raw ? raw.Question : null;

  // The route ID shown in the footer (mirrors decision_id / session id)
  const routeId = selectedSession.id;

  return (
    /* ── Backdrop ── */
    <div
      onClick={close}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(5,7,10,0.62)",
        backdropFilter: "blur(3px)",
        WebkitBackdropFilter: "blur(3px)",
        zIndex: 60,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 32,
      }}
    >
      {/* ── Panel ── */}
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "100%",
          maxWidth: 900,
          maxHeight: "88vh",
          background: "var(--panel)",
          border: "1px solid var(--border2)",
          borderRadius: 18,
          boxShadow: "var(--shadow)",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          animation: "slideup 0.18s ease",
        }}
      >
        {/* ===== HEADER ===== */}
        <div
          style={{
            padding: "18px 22px",
            borderBottom: "1px solid var(--border)",
            display: "flex",
            alignItems: "flex-start",
            gap: 13,
            flexShrink: 0,
          }}
        >
          {/* Gate icon box */}
          <div
            style={{
              width: 40,
              height: 40,
              borderRadius: 11,
              background: "var(--needs-bg)",
              border: "1px solid var(--needs)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: "calc(22px * var(--fs,1))",
              color: "var(--needs)",
              flexShrink: 0,
            }}
          >
            {gate.icon}
          </div>

          {/* Title block */}
          <div style={{ flex: 1, minWidth: 0 }}>
            {/* Kind badge + host label row */}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 9,
                marginBottom: 6,
              }}
            >
              <span
                style={{
                  fontSize: "calc(12.5px * var(--fs,1))",
                  fontWeight: 700,
                  letterSpacing: 0.4,
                  padding: "2px 7px",
                  borderRadius: 5,
                  color: "var(--needs)",
                  background: "var(--needs-bg)",
                  textTransform: "uppercase",
                }}
                data-testid="gate-kind-badge"
              >
                {kindBadge(kind)}
              </span>
              <span
                className="mono"
                style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)" }}
              >
                {selectedSession.host_id}
              </span>
            </div>

            {/* Gate title */}
            <h2
              style={{
                fontSize: "calc(19px * var(--fs,1))",
                fontWeight: 700,
                margin: "0 0 1px",
                letterSpacing: -0.2,
                color: "var(--txt)",
              }}
            >
              {gate.lead}
            </h2>

            {/* Session · project */}
            <div
              className="mono"
              style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt3)" }}
            >
              {selectedSession.name} · {selectedSession.project}
            </div>
          </div>

          {/* Close button */}
          <button
            onClick={close}
            aria-label="Close"
            style={{
              width: 30,
              height: 30,
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
              fontSize: "calc(17px * var(--fs,1))",
              flexShrink: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              lineHeight: 1,
            }}
          >
            ✕
          </button>
        </div>

        {/* ===== SCROLLABLE BODY ===== */}
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "20px 22px",
          }}
        >
          {/* ---- PERMISSION variant ---- */}
          {gate.is_permission && permissionData && (
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Explanation. For MCP elicitations (e.g. a request_path_access
                  prompt) this is the server's FULL markdown prompt — carrying the
                  requested path/mode/reason — so preserve its line breaks. */}
              <div
                style={{
                  fontSize: "calc(15px * var(--fs,1))",
                  color: "var(--txt2)",
                  marginBottom: 11,
                  lineHeight: 1.55,
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {permissionData.explanation}
              </div>

              {/* Command block — only when there is an actual command to run.
                  Elicitations without a fenced command (e.g. path-access requests)
                  carry an empty command; showing "$ " with a blank line there was
                  meaningless, so hide the whole block. */}
              {permissionData.command.trim() && (
              <div
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                  marginBottom: 14,
                }}
              >
                {/* Block header */}
                <div
                  style={{
                    padding: "8px 13px",
                    background: "var(--panel2)",
                    borderBottom: "1px solid var(--border)",
                    fontSize: "calc(13px * var(--fs,1))",
                    fontWeight: 700,
                    letterSpacing: 0.5,
                    textTransform: "uppercase",
                    color: "var(--txt3)",
                  }}
                >
                  Command to run
                </div>

                {/* $ command */}
                <pre
                  className="mono"
                  data-testid="gate-command"
                  style={{
                    margin: 0,
                    padding: 14,
                    fontSize: "calc(15.5px * var(--fs,1))",
                    color: "var(--accent2)",
                    background: "var(--bg)",
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-all",
                  }}
                >
                  $ {permissionData.command}
                </pre>

                {/* cwd */}
                <div
                  className="mono"
                  data-testid="gate-cwd"
                  style={{
                    padding: "8px 14px",
                    fontSize: "calc(13px * var(--fs,1))",
                    color: "var(--txt3)",
                    borderTop: "1px solid var(--border)",
                  }}
                >
                  cwd: {permissionData.cwd}
                </div>
              </div>
              )}

              {/* Risk warning — single-line amber banner. Only shown when there
                  IS a risk note; command gates carry no risk string, so an empty
                  banner used to render as a blank amber box. */}
              {permissionData.risk.trim() && (
                <div
                  data-testid="gate-risk"
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "10px 13px",
                    borderRadius: 9,
                    background: "var(--needs-bg)",
                    border: "1px solid var(--needs)",
                    fontSize: "calc(14.5px * var(--fs,1))",
                    color: "var(--needs)",
                  }}
                >
                  ⚠ {permissionData.risk}
                </div>
              )}

              {/* Scope choices (MCP elicitation): one button per allowed scope.
                  Clicking sends that choice as the elicitation response. */}
              {hasChoices && (
                <GateOptions
                  testid="gate-elicitation-choices"
                  disabled={busy}
                  options={elicitationChoices.map((c, i) => ({
                    id: `${c.key}=${c.value}-${i}`,
                    testid: `gate-choice-${c.value}`,
                    label: c.label,
                    onSelect: () => handleElicitationChoice(c.key, c.value),
                  }))}
                />
              )}

              {/* claude's OWN "always allow …" suggestions / ACP options, rendered
                  VERBATIM (one row each) in the body — same stacked layout as every
                  other gate choice. Picking one approves now AND persists the exact
                  rule/option the agent offered. Absent (or an elicitation) ⇒ only the
                  footer Allow/Reject show. */}
              {!gate.is_elicitation && (
                <GateOptions
                  disabled={busy}
                  options={permissionSuggestions.map((s, i) => ({
                    id: `suggestion-${i}`,
                    testid: `approve-suggestion-${i}`,
                    label: suggestionLabel(s),
                    title: s.destination ? `destination: ${s.destination}` : undefined,
                    onSelect: () => handleAllowSuggestion(s),
                  }))}
                />
              )}
            </div>
          )}

          {/* ---- PLAN variant ---- */}
          {gate.is_plan && planData && (
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Plan markdown body (ExitPlanMode populates summary with the markdown) */}
              <div
                data-testid="gate-plan-body"
                style={{
                  fontSize: "calc(15px * var(--fs,1))",
                  color: "var(--txt)",
                  lineHeight: 1.6,
                  marginBottom: 16,
                }}
              >
                <Markdown text={planData.summary} />
              </div>

              {/* Steps (only present for old-style Plan gates with parsed steps) */}
              {planData.steps.length > 0 && (
                <div
                  data-testid="gate-plan-steps"
                  style={{ display: "flex", flexDirection: "column", gap: 9 }}
                >
                  {planData.steps.map((step, i) => (
                    <div
                      key={i}
                      style={{
                        display: "flex",
                        gap: 12,
                        padding: "13px 14px",
                        borderRadius: 11,
                        background: "var(--panel2)",
                        border: "1px solid var(--border)",
                      }}
                    >
                      {/* Step number circle — accent style */}
                      <span
                        style={{
                          width: 23,
                          height: 23,
                          borderRadius: "50%",
                          background: "var(--accent-bg)",
                          color: "var(--accent2)",
                          fontWeight: 800,
                          fontSize: "calc(14px * var(--fs,1))",
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                          flexShrink: 0,
                        }}
                      >
                        {i + 1}
                      </span>

                      <div>
                        <div
                          style={{
                            fontWeight: 600,
                            fontSize: "calc(15.5px * var(--fs,1))",
                            marginBottom: 2,
                            color: "var(--txt)",
                          }}
                        >
                          {step.title}
                        </div>
                        <div
                          style={{
                            fontSize: "calc(14px * var(--fs,1))",
                            color: "var(--txt3)",
                            lineHeight: 1.5,
                          }}
                        >
                          {step.detail}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* ---- QUESTION variant ---- */}
          {gate.is_question && questionData && (
            <div
              data-testid="gate-question-body"
              style={{ display: "flex", flexDirection: "column", gap: 18 }}
            >
              {questionData.questions.map((q, qi) => (
                <div key={qi} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                  {/* Header badge */}
                  {q.header && (
                    <span
                      style={{
                        fontSize: "calc(12.5px * var(--fs,1))",
                        fontWeight: 700,
                        letterSpacing: 0.4,
                        padding: "2px 7px",
                        borderRadius: 5,
                        color: "var(--accent2)",
                        background: "var(--accent-bg)",
                        textTransform: "uppercase",
                        alignSelf: "flex-start",
                      }}
                    >
                      {q.header}
                    </span>
                  )}
                  {/* Question text */}
                  <div
                    style={{
                      fontSize: "calc(15.5px * var(--fs,1))",
                      fontWeight: 600,
                      color: "var(--txt)",
                      lineHeight: 1.5,
                    }}
                  >
                    {q.question}
                  </div>
                  {/* Options — same stacked renderer as every other gate choice.
                      Selection/accumulation stays here; GateOptions is presentational. */}
                  <GateOptions
                    disabled={busy}
                    options={q.options.map((opt, oi) => {
                      const selectedLabels = multiSelected[q.question] ?? [];
                      const isSelected = selectedLabels.includes(opt.label);

                      // Fire-and-close immediately ONLY for a lone single-select
                      // question. With multiple questions (or multi-select), an
                      // immediate send would submit before the other questions are
                      // answered — so accumulate and require the Submit button.
                      const immediate =
                        !q.multi_select && questionData.questions.length === 1;

                      const onSelect = async () => {
                        if (!selectedSession || !questionData) return;
                        if (immediate) {
                          setBusy(true);
                          await answerQuestion({
                            session_id: selectedSession.id,
                            request_id: questionData.request_id,
                            answers: { [q.question]: opt.label },
                          });
                          setBusy(false);
                          close();
                          return;
                        }
                        setMultiSelected((prev) => {
                          const cur = prev[q.question] ?? [];
                          let next: string[];
                          if (q.multi_select) {
                            next = cur.includes(opt.label)
                              ? cur.filter((l) => l !== opt.label)
                              : [...cur, opt.label];
                          } else {
                            // Single-select within a multi-question gate = radio.
                            next = [opt.label];
                          }
                          return { ...prev, [q.question]: next };
                        });
                      };

                      return {
                        id: String(oi),
                        label: opt.label,
                        description: opt.description || undefined,
                        selected: isSelected,
                        onSelect,
                      } as GateOption;
                    })}
                  />
                </div>
              ))}
            </div>
          )}

          {/* ---- EDIT variant ---- */}
          {gate.is_edit && editData && (
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Summary */}
              <div
                style={{
                  fontSize: "calc(15px * var(--fs,1))",
                  color: "var(--txt2)",
                  marginBottom: 13,
                  lineHeight: 1.55,
                }}
              >
                {gate.lead}
              </div>

              {/* Diff block */}
              <div
                data-testid="gate-diff"
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                }}
              >
                <DiffView rows={gate.diff_rows} filename={editData.file} />
              </div>
            </div>
          )}

          {/* ---- REJECT REASON (inside scrollable body) ---- */}
          {rejectOpen && (
            <div style={{ marginTop: 15 }}>
              <div
                style={{
                  fontSize: "calc(13.5px * var(--fs,1))",
                  fontWeight: 700,
                  color: "var(--error)",
                  marginBottom: 7,
                  textTransform: "uppercase",
                  letterSpacing: 0.4,
                }}
              >
                Reason for rejection (sent back to the session)
              </div>
              <textarea
                data-testid="reject-reason"
                rows={2}
                placeholder="e.g. Don't run migrations against prod — target the staging DB instead."
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                style={{
                  width: "100%",
                  boxSizing: "border-box",
                  background: "var(--panel2)",
                  border: "1px solid var(--error)",
                  borderRadius: 9,
                  padding: 11,
                  color: "var(--txt)",
                  fontFamily: "inherit",
                  fontSize: "calc(15px * var(--fs,1))",
                  outline: "none",
                  resize: "vertical",
                }}
              />
            </div>
          )}
        </div>

        {/* ===== FOOTER ===== */}
        <div
          style={{
            flexShrink: 0,
            padding: "15px 22px",
            borderTop: "1px solid var(--border)",
            display: "flex",
            alignItems: "center",
            gap: 10,
            background: "var(--panel)",
          }}
        >
          {/* Route info */}
          <div
            className="mono"
            style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)", flex: 1 }}
          >
            Decision routes only to this session · {routeId}
          </div>

          {/* ---- Reject-open state: Cancel + Send rejection ---- */}
          {rejectOpen ? (
            <>
              <button
                onClick={() => setRejectOpen(false)}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 600,
                  fontSize: "calc(15px * var(--fs,1))",
                  padding: "9px 15px",
                  borderRadius: 9,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                Cancel
              </button>
              <button
                data-testid="send-rejection-btn"
                onClick={handleReject}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: "calc(15px * var(--fs,1))",
                  padding: "9px 18px",
                  borderRadius: 9,
                  border: "none",
                  background: "var(--error)",
                  color: "#fff",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                Send rejection
              </button>
            </>
          ) : (
            <>
              {/* ---- Normal state ---- */}
              {/* For question gates: Skip + (multi-select Submit); no Reject… */}
              {gate.is_question ? (
                <>
                  {/* Skip button — sends empty answers */}
                  <button
                    data-testid="question-skip-btn"
                    onClick={async () => {
                      if (!selectedSession || !questionData) return;
                      setBusy(true);
                      await answerQuestion({
                        session_id: selectedSession.id,
                        request_id: questionData.request_id,
                        answers: {},
                      });
                      setBusy(false);
                      close();
                    }}
                    disabled={busy}
                    style={{
                      fontFamily: "inherit",
                      fontWeight: 600,
                      fontSize: "calc(15px * var(--fs,1))",
                      padding: "9px 15px",
                      borderRadius: 9,
                      border: "1px solid var(--border2)",
                      background: "var(--panel2)",
                      color: "var(--txt2)",
                      cursor: busy ? "not-allowed" : "pointer",
                      opacity: busy ? 0.6 : 1,
                    }}
                  >
                    Skip
                  </button>
                  {/* Submit button — shown whenever immediate fire-on-click does
                      NOT apply: multiple questions, or any multi-select question. */}
                  {questionData &&
                    (questionData.questions.length > 1 ||
                      questionData.questions.some((q) => q.multi_select)) && (
                    <button
                      data-testid="question-submit-btn"
                      onClick={async () => {
                        if (!selectedSession || !questionData) return;
                        setBusy(true);
                        const answers: Record<string, string> = {};
                        for (const q of questionData.questions) {
                          const sel = multiSelected[q.question] ?? [];
                          if (sel.length > 0) {
                            answers[q.question] = sel.join(", ");
                          }
                        }
                        await answerQuestion({
                          session_id: selectedSession.id,
                          request_id: questionData.request_id,
                          answers,
                        });
                        setBusy(false);
                        close();
                      }}
                      disabled={busy}
                      style={{
                        fontFamily: "inherit",
                        fontWeight: 700,
                        fontSize: "calc(15px * var(--fs,1))",
                        padding: "9px 20px",
                        borderRadius: 9,
                        border: "none",
                        background: "var(--done)",
                        color: "#0e1116",
                        cursor: busy ? "not-allowed" : "pointer",
                        opacity: busy ? 0.6 : 1,
                      }}
                    >
                      Submit
                    </button>
                  )}
                </>
              ) : (
                <>
                  <button
                    data-testid="reject-btn"
                    onClick={() => setRejectOpen(true)}
                    disabled={busy}
                    style={{
                      fontFamily: "inherit",
                      fontWeight: 600,
                      fontSize: "calc(15px * var(--fs,1))",
                      padding: "9px 15px",
                      borderRadius: 9,
                      border: "1px solid var(--border2)",
                      background: "var(--panel2)",
                      color: "var(--txt2)",
                      cursor: busy ? "not-allowed" : "pointer",
                      opacity: busy ? 0.6 : 1,
                    }}
                  >
                    {rejectLabel(kind)}
                  </button>
                  {/* claude/ACP "always allow …" options now render in the BODY
                      (GateOptions), consistent with every other gate choice — not as
                      footer buttons. The footer keeps only the generic actions. */}
                  {/* Elicitation scope choices replace the generic Approve —
                      the per-choice buttons in the body ARE the accept action. */}
                  {!hasChoices && (
                    <button
                      data-testid="approve-btn"
                      onClick={handleApprove}
                      disabled={busy}
                      style={{
                        fontFamily: "inherit",
                        fontWeight: 700,
                        fontSize: "calc(15px * var(--fs,1))",
                        padding: "9px 20px",
                        borderRadius: 9,
                        border: "none",
                        background: "var(--done)",
                        color: "#0e1116",
                        cursor: busy ? "not-allowed" : "pointer",
                        opacity: busy ? 0.6 : 1,
                      }}
                    >
                      {approveLabel(kind)}
                    </button>
                  )}
                </>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
