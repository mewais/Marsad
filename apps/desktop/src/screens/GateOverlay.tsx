/**
 * GateOverlay — the ONE unified gate modal for permission / MCP-elicitation /
 * plan / edit / question.
 *
 * All kind-specific knowledge now lives in `bridge/gateModel.ts`
 * (`normalizeGate` builds a `NormalizedGate`, `responseToAnswer`/`submitAnswers`
 * map a chosen response to the concrete api call). This component is a single
 * render path over that model with no per-kind branching: a Markdown body, rich
 * attachments (command box / diff), a risk banner, and the answer options
 * rendered through the shared `<GateOptions>`.
 */
import { useState, useEffect } from "react";
import { useVm } from "../bridge/useVm";
import { DiffView } from "../components/DiffView";
import { Markdown } from "../components/Markdown";
import { GateOptions, type GateOption } from "../components/GateOptions";
import {
  normalizeGate,
  responseToAnswer,
  submitAnswers,
  type GateOptionModel,
} from "../bridge/gateModel";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function kindBadge(kind: string): string {
  if (kind === "plan") return "Plan";
  if (kind === "edit") return "Edit";
  if (kind === "question") return "Question";
  return "Permission";
}

// ---------------------------------------------------------------------------
// GateOverlay
// ---------------------------------------------------------------------------

export function GateOverlay() {
  const { selectedGate, selectedSession, state, dispatch } = useVm();

  // Free-text sub-mode (pick-one): the option whose free-text field is open.
  const [freeTextOption, setFreeTextOption] = useState<GateOptionModel | null>(null);
  const [freeText, setFreeText] = useState("");
  const [busy, setBusy] = useState(false);
  // Collect mode (questions): per-prompt selected labels, keyed by question text.
  const [multiSelected, setMultiSelected] = useState<Record<string, string[]>>({});
  // Collect mode: per-prompt "Other" typed answer, keyed by question text.
  const [otherText, setOtherText] = useState<Record<string, string>>({});

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
    setFreeTextOption(null);
    setFreeText("");
  }

  if (!open || !selectedGate || !selectedSession || !selectedSession.gate) {
    // Render nothing visible — keep mount for test compatibility
    return (
      <div
        style={{ display: "none" }}
        data-testid="gate-overlay-hidden"
        aria-hidden="true"
      />
    );
  }

  const raw = selectedSession.gate;
  const normalized = normalizeGate(selectedGate, raw);
  const decisionId = selectedGate.decision_id ?? selectedSession.id;
  // Plan steps still render from the raw payload (old-style parsed steps).
  const planSteps = "Plan" in raw ? raw.Plan.steps : [];
  const questionRequestId = "Question" in raw ? raw.Question.request_id : decisionId;

  const routeId = selectedSession.id;

  // --- Answer dispatch ---

  async function fire(response: GateOptionModel["response"], text?: string) {
    if (!selectedSession) return;
    setBusy(true);
    await responseToAnswer(
      selectedSession.id,
      decisionId,
      questionRequestId,
      response,
      text
    );
    setBusy(false);
    close();
  }

  // pick-one: an option with free-text opens the sub-mode; otherwise fire now.
  function onPickOne(opt: GateOptionModel) {
    if (opt.freeText) {
      setFreeTextOption(opt);
      setFreeText("");
      return;
    }
    void fire(opt.response);
  }

  // collect: accumulate selection (radio / checkbox) per prompt.
  function onCollectSelect(
    questionText: string,
    multiSelect: boolean,
    label: string
  ) {
    setMultiSelected((prev) => {
      const cur = prev[questionText] ?? [];
      let next: string[];
      if (multiSelect) {
        next = cur.includes(label)
          ? cur.filter((l) => l !== label)
          : [...cur, label];
      } else {
        next = [label];
      }
      return { ...prev, [questionText]: next };
    });
  }

  // collect: aggregate every prompt's answer and submit.
  async function onSubmitQuestions() {
    if (!selectedSession) return;
    setBusy(true);
    const answers: Record<string, string> = {};
    for (const prompt of normalized.prompts) {
      const qText = prompt.question ?? prompt.id;
      const other = otherText[qText]?.trim();
      if (other) {
        answers[qText] = other;
        continue;
      }
      const sel = multiSelected[qText] ?? [];
      if (sel.length > 0) answers[qText] = sel.join(", ");
    }
    await submitAnswers(selectedSession.id, questionRequestId, answers);
    setBusy(false);
    close();
  }

  // A lone single-select single-question fires immediately on pick (preserve
  // today's behavior). Anything else accumulates and requires Submit.
  const loneSingle =
    normalized.mode === "collect" &&
    normalized.prompts.length === 1 &&
    !normalized.prompts[0].multiSelect;

  // A lone single-select question fires a preset option immediately, so its
  // Submit button is normally hidden. But choosing "Other…" opens a custom-answer
  // field WITHOUT firing — so once any Other field is open, Submit must be shown
  // (else a typed custom answer can never be sent). Keyed on the presence of an
  // Other field, which `onSelect` opens for the "other" option.
  const otherActive = Object.keys(otherText).length > 0;

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
            {selectedGate.icon}
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
                {kindBadge(normalized.kind)}
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
              {normalized.title}
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
          {/* Body — Markdown for EVERY kind that carries one. */}
          {normalized.body && (
            <div
              data-testid="gate-body"
              style={{
                fontSize: "calc(15px * var(--fs,1))",
                color: "var(--txt2)",
                lineHeight: 1.6,
                marginBottom: 14,
              }}
            >
              <Markdown text={normalized.body} />
            </div>
          )}

          {/* Attachments — command box / diff. */}
          {normalized.attachments.map((att, i) =>
            att.kind === "command" ? (
              <div
                key={i}
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                  marginBottom: 14,
                }}
              >
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
                  $ {att.command}
                </pre>
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
                  cwd: {att.cwd}
                </div>
              </div>
            ) : (
              <div
                key={i}
                data-testid="gate-diff"
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                  marginBottom: 14,
                }}
              >
                <DiffView rows={att.rows} filename={att.filename} />
              </div>
            )
          )}

          {/* Risk banner. */}
          {normalized.risk && (
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
                marginBottom: 14,
              }}
            >
              ⚠ {normalized.risk}
            </div>
          )}

          {/* Plan steps (only present for old-style Plan gates with parsed steps). */}
          {planSteps.length > 0 && (
            <div
              data-testid="gate-plan-steps"
              style={{ display: "flex", flexDirection: "column", gap: 9, marginBottom: 14 }}
            >
              {planSteps.map((step, i) => (
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

          {/* ── Free-text sub-mode (pick-one option with freeText) ── */}
          {freeTextOption ? (
            <div style={{ marginTop: 4 }}>
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
                {freeTextOption.freeText?.label}
              </div>
              <textarea
                data-testid={
                  freeTextOption.response.type === "deny" ? "reject-reason" : undefined
                }
                rows={2}
                placeholder={freeTextOption.freeText?.placeholder}
                value={freeText}
                onChange={(e) => setFreeText(e.target.value)}
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
          ) : (
            /* ── Options ── */
            <div style={{ display: "flex", flexDirection: "column", gap: 18 }}>
              {normalized.prompts.map((prompt) => {
                const qText = prompt.question ?? prompt.id;
                const selectedLabels = multiSelected[qText] ?? [];
                return (
                  <div
                    key={prompt.id}
                    style={{ display: "flex", flexDirection: "column", gap: 8 }}
                  >
                    {/* Question header badge */}
                    {prompt.header && (
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
                        {prompt.header}
                      </span>
                    )}
                    {/* Question text (Markdown) */}
                    {prompt.question && (
                      <div
                        style={{
                          fontSize: "calc(15.5px * var(--fs,1))",
                          fontWeight: 600,
                          color: "var(--txt)",
                          lineHeight: 1.5,
                        }}
                      >
                        <Markdown text={prompt.question} />
                      </div>
                    )}
                    <GateOptions
                      testid={
                        normalized.kind === "permission" &&
                        "Permission" in raw &&
                        raw.Permission.is_elicitation
                          ? "gate-elicitation-choices"
                          : undefined
                      }
                      disabled={busy}
                      options={prompt.options.map((opt): GateOption => {
                        if (normalized.mode === "collect") {
                          const isOther = opt.id === "other";
                          const isSelected =
                            !isOther && selectedLabels.includes(opt.label);
                          return {
                            id: opt.id,
                            testid: opt.testid,
                            label: opt.label,
                            description: opt.description,
                            title: opt.title,
                            selected: isSelected,
                            onSelect: () => {
                              if (isOther) {
                                // Toggle the per-prompt Other text field on.
                                setOtherText((prev) => ({
                                  ...prev,
                                  [qText]: prev[qText] ?? "",
                                }));
                                return;
                              }
                              if (loneSingle) {
                                void fire(opt.response);
                                return;
                              }
                              onCollectSelect(qText, prompt.multiSelect, opt.label);
                            },
                          };
                        }
                        // pick-one
                        return {
                          id: opt.id,
                          testid: opt.testid,
                          label: opt.label,
                          description: opt.description,
                          title: opt.title,
                          onSelect: () => onPickOne(opt),
                        };
                      })}
                    />
                    {/* Per-prompt "Other" free-text input (collect mode). */}
                    {normalized.mode === "collect" &&
                      otherText[qText] !== undefined && (
                        <input
                          data-testid={`question-other-${prompt.id}`}
                          placeholder="Type a custom answer"
                          value={otherText[qText]}
                          onChange={(e) =>
                            setOtherText((prev) => ({
                              ...prev,
                              [qText]: e.target.value,
                            }))
                          }
                          style={{
                            width: "100%",
                            boxSizing: "border-box",
                            background: "var(--panel2)",
                            border: "1px solid var(--accent2)",
                            borderRadius: 9,
                            padding: 11,
                            color: "var(--txt)",
                            fontFamily: "inherit",
                            fontSize: "calc(15px * var(--fs,1))",
                            outline: "none",
                          }}
                        />
                      )}
                  </div>
                );
              })}
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

          {freeTextOption ? (
            /* Free-text confirm/cancel. */
            <>
              <button
                onClick={() => {
                  setFreeTextOption(null);
                  setFreeText("");
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
                Cancel
              </button>
              <button
                data-testid={
                  freeTextOption.response.type === "deny"
                    ? "send-rejection-btn"
                    : "send-answer-btn"
                }
                onClick={() => void fire(freeTextOption.response, freeText)}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: "calc(15px * var(--fs,1))",
                  padding: "9px 18px",
                  borderRadius: 9,
                  border: "none",
                  background:
                    freeTextOption.response.type === "deny"
                      ? "var(--error)"
                      : "var(--done)",
                  color:
                    freeTextOption.response.type === "deny" ? "#fff" : "#0e1116",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                {freeTextOption.response.type === "deny"
                  ? "Send rejection"
                  : "Send answer"}
              </button>
            </>
          ) : normalized.mode === "collect" ? (
            /* Questions: Skip + (Submit unless a lone single-select question). */
            <>
              {normalized.skip && (
                <button
                  data-testid={normalized.skip.testid}
                  onClick={() => void fire(normalized.skip!.response)}
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
                  {normalized.skip.label}
                </button>
              )}
              {(!loneSingle || otherActive) && normalized.submit && (
                <button
                  data-testid="question-submit-btn"
                  onClick={() => void onSubmitQuestions()}
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
                  {normalized.submit.label}
                </button>
              )}
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}
