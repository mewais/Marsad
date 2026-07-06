//! marsad-format — display formatters and presentation metadata.
//!
//! See `docs/specs/format.md` for the authoritative contract.

use marsad_gates::{Gate, Sig};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Token — design token name
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Token {
    Accent,
    Accent2,
    Needs,
    Error,
    Done,
    Neutral,
}

// ---------------------------------------------------------------------------
// Phase — mirrors events::Phase by value
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Starting,
    Working,
    Needs,
    Error,
    Done,
    Stale,
    Detached,
}

// ---------------------------------------------------------------------------
// Meta — state metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub label: &'static str,
    pub color: Token,
    pub bg: Token,
}

// ---------------------------------------------------------------------------
// GateMeta — gate metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct GateMeta {
    pub kind: &'static str,
    pub icon: &'static str,
    pub lead: &'static str,
    pub summary: String,
    pub approve_label: &'static str,
}

// ---------------------------------------------------------------------------
// fmt_tokens
// ---------------------------------------------------------------------------

/// Format a token count for display.
///
/// - `n < 1000` → plain integer string.
/// - `n >= 1000` → one decimal place with trailing `.0` stripped, suffix `k`.
pub fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    // Compute one decimal: round(n / 100) gives tenths-of-thousands.
    let tenths = (n as f64 / 100.0).round() as u64;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{frac}k")
    }
}

// ---------------------------------------------------------------------------
// fmt_elapsed
// ---------------------------------------------------------------------------

/// Format an elapsed duration in seconds for display.
///
/// - `< 3600s` (i.e., `< 60 minutes`) → `"Mm SSs"` with zero-padded seconds.
/// - `>= 3600s` → `"Hh Mm"`.
pub fn fmt_elapsed(secs: u64) -> String {
    let total_minutes = secs / 60;
    if total_minutes < 60 {
        let m = total_minutes;
        let s = secs % 60;
        format!("{m}m {s:02}s")
    } else {
        let h = total_minutes / 60;
        let m = total_minutes % 60;
        format!("{h}h {m}m")
    }
}

// ---------------------------------------------------------------------------
// state_meta
// ---------------------------------------------------------------------------

/// Return display metadata for a session phase.
pub fn state_meta(phase: Phase) -> Meta {
    match phase {
        Phase::Starting => Meta {
            label: "Starting",
            color: Token::Neutral,
            bg: Token::Neutral,
        },
        Phase::Working => Meta {
            label: "Working",
            color: Token::Accent,
            bg: Token::Accent,
        },
        Phase::Needs => Meta {
            label: "Needs you",
            color: Token::Needs,
            bg: Token::Needs,
        },
        Phase::Error => Meta {
            label: "Error",
            color: Token::Error,
            bg: Token::Error,
        },
        Phase::Done => Meta {
            label: "Done",
            color: Token::Done,
            bg: Token::Done,
        },
        Phase::Stale => Meta {
            label: "Stale",
            color: Token::Neutral,
            bg: Token::Neutral,
        },
        Phase::Detached => Meta {
            label: "Detached",
            color: Token::Accent2,
            bg: Token::Accent2,
        },
    }
}

// ---------------------------------------------------------------------------
// ctx_color
// ---------------------------------------------------------------------------

/// Return the context-bar color token for a given fill percentage.
///
/// Corrected thresholds (spec overrides the prototype ordering bug):
/// - `>= 90` → `Error`
/// - `>= 75` → `Needs`
/// - else    → `Accent`
pub fn ctx_color(pct: f32) -> Token {
    if pct >= 90.0 {
        Token::Error
    } else if pct >= 75.0 {
        Token::Needs
    } else {
        Token::Accent
    }
}

// ---------------------------------------------------------------------------
// gate_meta
// ---------------------------------------------------------------------------

/// Return display metadata for a pending gate.
pub fn gate_meta(gate: &Gate) -> GateMeta {
    match gate {
        Gate::Permission { command, .. } => GateMeta {
            kind: "Permission",
            icon: "⌘",
            lead: "Wants to run",
            summary: format!("$ {command}"),
            approve_label: "Approve & run",
        },
        Gate::Plan { summary, steps, .. } => {
            let n = steps.len();
            let plan_summary = if n == 0 {
                // Derive a summary from the plan markdown body: first non-empty line.
                let first_line = summary
                    .lines()
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty())
                    .unwrap_or("");
                if first_line.is_empty() {
                    String::from("Plan")
                } else if first_line.len() > 60 {
                    format!("{}…", &first_line[..60])
                } else {
                    first_line.to_string()
                }
            } else {
                let first = &steps[0].title;
                format!("{n} steps · {first}…")
            };
            GateMeta {
                kind: "Plan review",
                icon: "❑",
                lead: "Proposed plan",
                summary: plan_summary,
                approve_label: "Approve plan",
            }
        }
        Gate::Question { questions, .. } => {
            let summary = match questions.len() {
                0 => "Question".to_string(),
                1 => questions[0].question.clone(),
                n => format!("{n} questions"),
            };
            GateMeta {
                kind: "question",
                icon: "?",
                lead: "Wants your input",
                summary,
                approve_label: "Submit",
            }
        }
        Gate::Edit { file, diff, .. } => {
            let basename = std::path::Path::new(file)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(file.as_str());
            let adds: usize = diff.iter().filter(|dl| dl.sig == Sig::Add).count();
            let dels: usize = diff.iter().filter(|dl| dl.sig == Sig::Del).count();
            GateMeta {
                kind: "File change",
                icon: "±",
                lead: "Wants to edit",
                summary: format!("{basename} · +{adds} \u{2212}{dels}"),
                approve_label: "Approve change",
            }
        }
        // X1/X2 mechanical compile-fix (docs/specs/wasl-connector.md §5.1):
        // Gate::Handoff is additive in marsad-gates; this arm keeps
        // gate_meta's exhaustive match compiling. `lead` is a `&'static str`
        // in this struct (every other arm's `lead` is a literal), so the
        // spec's per-occurrence "Wasl <kind> from <from>" text — which needs
        // runtime data — can't be reproduced without widening `GateMeta.lead`
        // to `String`, a broader change out of this block's scope; a static
        // lead is used instead. `summary` (preview, falling back to summary
        // when preview is empty) matches the spec exactly.
        Gate::Handoff {
            summary, preview, ..
        } => GateMeta {
            kind: "Handoff",
            icon: "⇄",
            lead: "Wasl handoff",
            summary: if preview.is_empty() {
                summary.clone()
            } else {
                preview.clone()
            },
            approve_label: "Approve & send",
        },
    }
}
