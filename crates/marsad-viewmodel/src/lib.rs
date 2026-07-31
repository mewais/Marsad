//! marsad-viewmodel — pure dashboard aggregation (port of renderVals()).
//!
//! See `docs/specs/viewmodel.md` for the authoritative contract.

use serde::{Deserialize, Serialize};

// Re-export Host types so callers/tests can name them as
// `marsad_viewmodel::{Host, HostStatus}` without depending on marsad-hosts directly.
pub use marsad_hosts::{Host, HostStatus};

// ---------------------------------------------------------------------------
// Phase re-use
// ---------------------------------------------------------------------------

use marsad_events::Phase;

// ---------------------------------------------------------------------------
// SessionView — flattened, display-ready session record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub name: String,
    pub host_id: String,
    pub project: String,
    pub branch: String,
    pub model: String,
    pub state: Phase,
    pub changed_files: u32,
    /// Total completed subagents count.
    pub subagents: u32,
    /// Number of currently running (in-progress) subagents.
    pub running_subagents: u32,
    /// Per-subagent detail list (name, model, status), sorted running first then done.
    /// Empty before the first Task/Agent tool-use event. Used by the Activity tab's
    /// collapsible running-subagents strip to show per-agent detail.
    #[serde(default)]
    pub subagent_list: Vec<SubagentView>,
    /// True while an async `!` shell command is running (the transcript shows the
    /// `! <cmd>` line and a working indicator until the output arrives). Drives
    /// the Activity tab's shell "working…" spinner without blocking the UI.
    #[serde(default)]
    pub shell_running: bool,
    pub cost_usd: f64,
    pub tokens: u64,
    pub context_pct: f32,
    pub gate: Option<marsad_gates::Gate>,
    /// Conversation transcript, in arrival order (derived from the stream).
    pub transcript: Vec<marsad_events::TranscriptEntry>,
    /// Working-tree changes in the session's project dir (from a git probe).
    pub changes: Vec<FileChange>,
    /// Per-model token + cost breakdown, sorted by model name for determinism.
    /// Empty when no `result` event with `modelUsage` has been received.
    pub model_usage: Vec<ModelUsageRow>,
    /// U4 cost truthfulness: `true` when `cost_usd` has never been set by a
    /// live agent-reported `result` event and is backed only by the
    /// litellm/hand pricing table (transcript backfill / usage probe) —
    /// i.e. every row in `model_usage` is `estimated`. `false` once any live
    /// `result` has landed (or before any cost data exists at all). Drives
    /// the Usage tab's "estimated"/"~" affordance on the top Cost tile.
    #[serde(default)]
    pub cost_estimated: bool,
    /// Number of context-compaction events observed this session
    /// (`marsad_events::SessionState::compactions`).
    #[serde(default)]
    pub compactions: u32,
    /// Metadata from the most recent compaction, if any (`None` before the
    /// first one).
    #[serde(default)]
    pub last_compaction: Option<marsad_proto::CompactMetadata>,
    /// Latest rate-limit status observed, if any (`None` before the first
    /// `rate_limit_event`).
    #[serde(default)]
    pub rate_limit: Option<marsad_proto::RateLimitInfo>,
    /// Tools available in this session (from the system/init event).
    #[serde(default)]
    pub tools: Vec<String>,
    /// MCP servers connected in this session (from the system/init event).
    #[serde(default)]
    pub mcp_servers: Vec<marsad_proto::McpServerInfo>,
    /// Slash commands available in this session (from the system/init event).
    #[serde(default)]
    pub slash_commands: Vec<marsad_proto::SlashCommandInfo>,
    /// The agent driver actually running THIS session ("claude"/"codex"/…).
    /// The header must label the session by its own driver, not the host's
    /// first-discovered capability driver (a host can offer several).
    #[serde(default = "default_driver")]
    pub driver: String,
    /// Lifecycle liveness (the two-axis model). `true` = Resumed: the session
    /// process is alive on its host and survives UI close / connection drop /
    /// full app restart. `false` = Paused: not running, for ANY reason — the
    /// user paused it, or the host killed it (reboot / OOM / scheduler / crash).
    /// Authoritative from the agent's `SessionLiveness` signal — NOT inferred
    /// from `state`, so a finished-but-alive session is `state: Done, live:
    /// true`. Drives the Pause↔Resume affordance, the board's Paused grouping,
    /// and spinner suppression.
    #[serde(default = "default_live")]
    pub live: bool,
    /// Per-session CONNECTION health, independent of `live`: "Connected",
    /// "Reconnecting" (stream dropped, auto-reattaching), or "Unstable" (flapping).
    /// Driven by the agent's real stream events (`SessionConn`), NOT the periodic
    /// host poll. The UI shows the true state and HARD-BLOCKS sending when this is
    /// not "Connected" (or when `live` is false). Defaults to "Connected".
    #[serde(default = "default_conn_health")]
    pub conn_health: String,
    /// Why the last turn ended, when noteworthy: a user Cancel (phase Done) or a
    /// real failure (phase Error). Drives the card's reason text so an Error card
    /// is never blank and a cancel reads "Cancelled by the user". `None` for a
    /// normal turn / while working.
    #[serde(default)]
    pub end_reason: Option<marsad_events::EndReason>,
    /// opencode ACP plan-mode: the live plan/todo list entries from the most recent
    /// `system/plan_update` event. Empty before the first plan_update.
    #[serde(default)]
    pub plan_entries: Vec<marsad_proto::PlanEntry>,
    /// opencode ACP: the current session mode id (e.g. "plan", "build") from the
    /// most recent `system/mode_update` event. `None` before the first mode_update.
    #[serde(default)]
    pub current_mode: Option<String>,
    /// The AVAILABLE agent modes for this session, agent-reported (opencode's
    /// configOptions, claude's version-derived list). The UI renders these instead
    /// of a hardcoded posture list; `current_mode` marks the active one.
    #[serde(default)]
    pub available_modes: Vec<marsad_proto::SessionMode>,
}

fn default_driver() -> String {
    "claude".to_string()
}

fn default_live() -> bool {
    true
}

fn default_conn_health() -> String {
    "Connected".to_string()
}

// ---------------------------------------------------------------------------
// SubagentView — one row in the running/done subagents list
// ---------------------------------------------------------------------------

/// Display-ready view of a single subagent (Task/Agent tool-use).
/// Serialises as `{"name":"...", "model":"...", "status":"Running"|"Done"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentView {
    pub name: String,
    pub model: Option<String>,
    /// "Running" while the subagent's tool_result has not yet closed it;
    /// "Done" once the matching tool_result arrives.
    pub status: String,
}

// ---------------------------------------------------------------------------
// ModelUsageRow — one row of the per-model usage breakdown
// ---------------------------------------------------------------------------

/// One row of the per-model token + cost breakdown in the Usage tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageRow {
    /// Model name (e.g. `"claude-sonnet-4.6[1m]"`).
    pub model: String,
    /// Cumulative input tokens for this model.
    pub input_tokens: u64,
    /// Cumulative output tokens for this model.
    pub output_tokens: u64,
    /// Cumulative cost in USD for this model.
    pub cost_usd: f64,
    /// U4 cost truthfulness: `true` when this row's cost was recomputed from
    /// the local litellm/hand pricing table (transcript backfill / usage
    /// probe) rather than copied verbatim from a live, agent-reported
    /// `result.modelUsage` entry. Drives the Usage tab's per-row
    /// "estimated"/"~" affordance.
    pub estimated: bool,
}

// ---------------------------------------------------------------------------
// FileChange — one changed file in a session's working tree
// ---------------------------------------------------------------------------

/// A single changed file, as reported by a git probe over the project dir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// Path relative to the project root.
    pub path: String,
    /// Porcelain status code, e.g. "M", "A", "D", "??".
    pub status: String,
    /// Lines added (best-effort; 0 when unknown, e.g. untracked).
    pub added: u32,
    /// Lines removed (best-effort; 0 when unknown).
    pub removed: u32,
}

// ---------------------------------------------------------------------------
// CapabilitiesView — display-ready mirror of marsad_proto::Capabilities
// (U1 dynamic capability discovery, docs/specs/dynamic-capabilities.md §3.6)
// ---------------------------------------------------------------------------

/// Display-ready mirror of `marsad_proto::Capabilities`. Ids are verbatim —
/// no label synthesis (`modelLabel()` has no successor). Pure `From`
/// mapping; the proto types are already wire/display-ready so the fields are
/// reused directly rather than duplicated into parallel view types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilitiesView {
    pub driver: String,
    pub host_id: String,
    pub cli_available: bool,
    /// Whether Marsad has a launch driver for this agent (static per-driver
    /// truth: claude/codex → true, gemini → false). What the launch UI gates
    /// on — carried verbatim from `marsad_proto::Capabilities::launchable`.
    pub launchable: bool,
    pub models: marsad_proto::ModelCatalog,
    /// The session modes this backend truly offers, discovered from the backend
    /// (never a hardcoded Marsad list). claude fills it from its version; others
    /// leave it empty (modes only knowable from a live session). Carried verbatim
    /// from `marsad_proto::Capabilities::available_modes`.
    #[serde(default)]
    pub available_modes: Vec<marsad_proto::SessionMode>,
    pub session_scope: Option<marsad_proto::SessionScope>,
    pub sessions: Vec<marsad_proto::DiscoveredSession>,
    pub fetched_at_ms: u64,
    pub errors: Vec<String>,
}

impl From<marsad_proto::Capabilities> for CapabilitiesView {
    fn from(c: marsad_proto::Capabilities) -> Self {
        CapabilitiesView {
            driver: c.driver,
            host_id: c.host_id,
            cli_available: c.cli_available,
            launchable: c.launchable,
            models: c.models,
            available_modes: c.available_modes,
            session_scope: c.session_scope,
            sessions: c.sessions,
            fetched_at_ms: c.fetched_at_ms,
            errors: c.errors,
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub session_ids: Vec<String>,
    pub saved_at: String,
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    All,
    Needs,
    Working,
    Error,
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Tab {
    Activity,
    Changes,
    Config,
    Usage,
    Terminal,
}

// ---------------------------------------------------------------------------
// AppSnapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub sessions: Vec<SessionView>,
    pub hosts: Vec<Host>,
    pub workspaces: Vec<Workspace>,
    pub active_ws: Option<String>,
    pub selected_id: Option<String>,
    pub filter: Filter,
    pub search: String,
    pub tab: Tab,
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub total: u32,
    pub needs: u32,
    pub errors: u32,
    pub working: u32,
    pub cost: String,
    pub tokens: String,
    pub host_count: u32,
}

/// Compute dashboard counts from a snapshot.
pub fn counts(s: &AppSnapshot) -> Counts {
    let total = s.sessions.len() as u32;
    let needs = s
        .sessions
        .iter()
        .filter(|sv| sv.state == Phase::Needs)
        .count() as u32;
    let errors = s
        .sessions
        .iter()
        .filter(|sv| sv.state == Phase::Error)
        .count() as u32;
    let working = s
        .sessions
        .iter()
        .filter(|sv| sv.state == Phase::Working)
        .count() as u32;

    let cost_sum: f64 = s.sessions.iter().map(|sv| sv.cost_usd).sum::<f64>();
    // Use abs() to guard against -0.0 from certain float sums.
    let cost = format!("${:.2}", cost_sum.abs());

    let token_sum: u64 = s.sessions.iter().map(|sv| sv.tokens).sum();
    let tokens = marsad_format::fmt_tokens(token_sum);

    // Count distinct host IDs referenced by any session.
    let mut seen_hosts: Vec<&str> = Vec::new();
    for sv in &s.sessions {
        if !seen_hosts.contains(&sv.host_id.as_str()) {
            seen_hosts.push(sv.host_id.as_str());
        }
    }
    let host_count = seen_hosts.len() as u32;

    Counts {
        total,
        needs,
        errors,
        working,
        cost,
        tokens,
        host_count,
    }
}

// ---------------------------------------------------------------------------
// SessionCard / SessionRow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCard {
    pub id: String,
    pub name: String,
    pub state: Phase,
    pub host_label: String,
    pub project: String,
    pub branch: String,
    pub gate_kind: Option<String>,
    pub gate_summary: Option<String>,
    pub last_action: Option<String>,
    pub context_pct: f32,
    pub tokens_label: String,
    pub elapsed_label: String,
}

/// Sidebar rows and overview cards share the shape.
pub type SessionRow = SessionCard;

/// Look up the display label for a host id; falls back to the raw id string.
fn host_label(host_id: &str, hosts: &[Host]) -> String {
    hosts
        .iter()
        .find(|h| h.id == host_id)
        .map(|h| h.label.as_str())
        .unwrap_or(host_id)
        .to_string()
}

/// Build a `SessionCard` from a `SessionView` and the host list.
fn session_card(sv: &SessionView, hosts: &[Host]) -> SessionCard {
    SessionCard {
        id: sv.id.clone(),
        name: sv.name.clone(),
        state: sv.state.clone(),
        host_label: host_label(&sv.host_id, hosts),
        project: sv.project.clone(),
        branch: sv.branch.clone(),
        gate_kind: None,
        gate_summary: None,
        last_action: None,
        context_pct: sv.context_pct,
        tokens_label: marsad_format::fmt_tokens(sv.tokens),
        elapsed_label: String::new(),
    }
}

// ---------------------------------------------------------------------------
// OverviewGroups
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverviewGroups {
    /// needs + error sessions (needs first)
    pub attention: Vec<SessionCard>,
    /// working + starting sessions
    pub active: Vec<SessionCard>,
    /// done / stopped sessions (Done, Stale, Detached)
    pub idle: Vec<SessionCard>,
}

/// Group sessions into attention/active/idle buckets for the overview panel.
pub fn group_overview(s: &AppSnapshot) -> OverviewGroups {
    let mut attention: Vec<SessionCard> = Vec::new();
    let mut active: Vec<SessionCard> = Vec::new();
    let mut idle: Vec<SessionCard> = Vec::new();

    // Mirrors marsad_workstream::severity (liveness-first) so the overview and the
    // board agree. A NOT-running session is always idle/Paused regardless of its
    // last phase — pausing kills the process, so a stale Needs/Error is wasted and
    // must NOT surface as attention. Attention (Needs/Error) + active (Working) are
    // therefore LIVE-only; `idle` holds live-Done plus every paused/dormant session.
    // Needs first in attention bucket.
    for sv in s.sessions.iter().filter(|sv| sv.live && sv.state == Phase::Needs) {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s.sessions.iter().filter(|sv| sv.live && sv.state == Phase::Error) {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s
        .sessions
        .iter()
        .filter(|sv| sv.live && sv.state == Phase::Working)
    {
        active.push(session_card(sv, &s.hosts));
    }
    for sv in s.sessions.iter().filter(|sv| {
        // Everything not already in attention/active: a LIVE resting session (Done,
        // or Starting folded into Done — mirrors severity), OR ANY not-live session
        // (Paused, regardless of its last phase — its stale Needs/Error is wasted).
        (sv.live && !matches!(sv.state, Phase::Needs | Phase::Error | Phase::Working))
            || !sv.live
    }) {
        idle.push(session_card(sv, &s.hosts));
    }

    OverviewGroups {
        attention,
        active,
        idle,
    }
}

// ---------------------------------------------------------------------------
// State sort key
// ---------------------------------------------------------------------------

fn state_order(p: &Phase) -> u8 {
    match p {
        Phase::Needs => 0,
        Phase::Error => 1,
        Phase::Working => 2,
        Phase::Starting => 3,
        // Done and any other phases (Stale, Detached) sort last.
        _ => 4,
    }
}

// ---------------------------------------------------------------------------
// sidebar_list
// ---------------------------------------------------------------------------

/// Apply filter, then search, then state-sort and return sidebar rows.
pub fn sidebar_list(s: &AppSnapshot) -> Vec<SessionRow> {
    let search_term = s.search.trim().to_lowercase();

    let mut rows: Vec<SessionRow> = s
        .sessions
        .iter()
        .filter(|sv| match &s.filter {
            Filter::All => true,
            Filter::Needs => sv.state == Phase::Needs,
            Filter::Working => sv.state == Phase::Working,
            Filter::Error => sv.state == Phase::Error,
        })
        .filter(|sv| {
            if search_term.is_empty() {
                return true;
            }
            let hl = host_label(&sv.host_id, &s.hosts).to_lowercase();
            sv.name.to_lowercase().contains(&search_term)
                || sv.project.to_lowercase().contains(&search_term)
                || hl.contains(&search_term)
                || sv.branch.to_lowercase().contains(&search_term)
        })
        .map(|sv| session_card(sv, &s.hosts))
        .collect();

    rows.sort_by_key(|r| state_order(&r.state));
    rows
}

// ---------------------------------------------------------------------------
// rescope
// ---------------------------------------------------------------------------

/// Restrict the snapshot to sessions belonging to the given workspace.
/// Unknown workspace ID → snapshot unchanged.
pub fn rescope(workspace_id: &str, s: &AppSnapshot) -> AppSnapshot {
    let ws = s.workspaces.iter().find(|w| w.id == workspace_id);
    match ws {
        None => s.clone(),
        Some(ws) => {
            let allowed: std::collections::HashSet<&str> =
                ws.session_ids.iter().map(String::as_str).collect();
            let sessions = s
                .sessions
                .iter()
                .filter(|sv| allowed.contains(sv.id.as_str()))
                .cloned()
                .collect();
            AppSnapshot {
                sessions,
                ..s.clone()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SessionDetailVM
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetailVM {
    pub name: String,
    pub host_label: String,
    pub branch: String,
    pub model: String,
    pub ctx_warn: bool,
    pub context_pct: f32,
    pub tokens_label: String,
    pub cost_label: String,
    pub elapsed_label: String,
}

/// Return a detail view-model for the currently selected session, or `None`.
pub fn session_detail(s: &AppSnapshot) -> Option<SessionDetailVM> {
    let sv = s
        .sessions
        .iter()
        .find(|sv| s.selected_id.as_deref() == Some(sv.id.as_str()))?;

    Some(SessionDetailVM {
        name: sv.name.clone(),
        host_label: host_label(&sv.host_id, &s.hosts),
        branch: sv.branch.clone(),
        model: sv.model.clone(),
        ctx_warn: sv.context_pct >= 75.0,
        context_pct: sv.context_pct,
        tokens_label: marsad_format::fmt_tokens(sv.tokens),
        cost_label: format!("${:.2}", sv.cost_usd),
        elapsed_label: String::new(),
    })
}

// ---------------------------------------------------------------------------
// GateVM
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateVM {
    pub kind: String,
    pub icon: String,
    pub lead: String,
    pub summary: String,
    pub is_permission: bool,
    pub is_elicitation: bool,
    pub is_plan: bool,
    pub is_edit: bool,
    pub is_question: bool,
    pub diff_rows: Vec<marsad_diff::SideRow>,
}

/// Map a gate variant to its dashboard icon codepoint.
///
/// The viewmodel layer owns these icon assignments (U+2318 ⌘, U+25A1 □, U+00B1 ±, U+2753 ❓).
/// They differ from the literal glyphs stored in marsad-format's GateMeta — the
/// viewmodel test suite is the authority for this crate's icon strings.
fn gate_icon(gate: &marsad_gates::Gate) -> &'static str {
    match gate {
        marsad_gates::Gate::Permission { .. } => "\u{2318}", // ⌘
        marsad_gates::Gate::Plan { .. } => "\u{25a1}",       // □
        marsad_gates::Gate::Edit { .. } => "\u{00b1}",       // ±
        marsad_gates::Gate::Question { .. } => "\u{2753}",   // ❓
        // X1/X2 mechanical compile-fix (docs/specs/wasl-connector.md §5.1):
        // Gate::Handoff is additive in marsad-gates; this one-line arm keeps
        // gate_icon's exhaustive match compiling. Minimal, single-line touch
        // only — the rest of this file is owned by a concurrent worker (U2).
        marsad_gates::Gate::Handoff { .. } => "\u{21c4}", // ⇄
    }
}

/// Return a gate view-model for the currently selected session, or `None`.
pub fn gate_vm(s: &AppSnapshot) -> Option<GateVM> {
    let sv = s
        .sessions
        .iter()
        .find(|sv| s.selected_id.as_deref() == Some(sv.id.as_str()))?;
    let gate = sv.gate.as_ref()?;

    let meta = marsad_format::gate_meta(gate);
    let is_permission = matches!(gate, marsad_gates::Gate::Permission { .. });
    let is_elicitation = matches!(gate, marsad_gates::Gate::Permission { is_elicitation: true, .. });
    let is_plan = matches!(gate, marsad_gates::Gate::Plan { .. });
    let is_edit = matches!(gate, marsad_gates::Gate::Edit { .. });
    let is_question = matches!(gate, marsad_gates::Gate::Question { .. });

    let diff_rows = if let marsad_gates::Gate::Edit { diff, .. } = gate {
        marsad_diff::split_side_by_side(diff)
    } else {
        Vec::new()
    };

    Some(GateVM {
        kind: meta.kind.to_string(),
        icon: gate_icon(gate).to_string(),
        lead: meta.lead.to_string(),
        summary: meta.summary,
        is_permission,
        is_elicitation,
        is_plan,
        is_edit,
        is_question,
        diff_rows,
    })
}

// ---------------------------------------------------------------------------
// SlashCmd
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SlashCmd {
    Clear,
    Compact,
    Cost,
    Model,
    Config,
    Init,
    Review,
    Agents,
    Resume,
    Terminal,
    Help,
    Unknown(String),
}

/// Parse a slash command string into a `SlashCmd` variant.
pub fn parse_slash(input: &str) -> SlashCmd {
    match input {
        "/clear" => SlashCmd::Clear,
        "/compact" => SlashCmd::Compact,
        "/cost" => SlashCmd::Cost,
        "/model" => SlashCmd::Model,
        "/config" => SlashCmd::Config,
        "/init" => SlashCmd::Init,
        "/review" => SlashCmd::Review,
        "/agents" => SlashCmd::Agents,
        "/resume" => SlashCmd::Resume,
        "/terminal" => SlashCmd::Terminal,
        "/help" => SlashCmd::Help,
        _ => SlashCmd::Unknown(input.to_string()),
    }
}

// ---------------------------------------------------------------------------
// B11 — Workstreams board + graph (additive extension)
// ---------------------------------------------------------------------------

use std::collections::HashMap;

use marsad_workstream::{
    conns_of, dependency_depth, driver_dots, head_state, host_chips, nodes_of, state_counts,
    ConnKind, Connection, Driver, HeadState, Node, StateCounts, Workstream,
};


// ---------------------------------------------------------------------------
// B11/B12 workstream view projections (split into submodules)
// ---------------------------------------------------------------------------
mod board;
pub use board::*;
mod cost;
pub use cost::*;
mod graph;
pub use graph::*;
