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
    /// Permission posture for this session, derived from the system/init
    /// `permissionMode` field: "default"→"ask", "acceptEdits"→"auto",
    /// "bypassPermissions"→"full". Falls back to "ask" when unknown or before
    /// the first system/init event.
    #[serde(default = "default_posture")]
    pub posture: String,
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
    /// Why the last turn ended, when noteworthy: a user Cancel (phase Done) or a
    /// real failure (phase Error). Drives the card's reason text so an Error card
    /// is never blank and a cancel reads "Cancelled by the user". `None` for a
    /// normal turn / while working.
    #[serde(default)]
    pub end_reason: Option<marsad_events::EndReason>,
}

fn default_posture() -> String {
    "ask".to_string()
}

fn default_driver() -> String {
    "claude".to_string()
}

fn default_live() -> bool {
    true
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

    // Two-axis lifecycle: attention/active buckets only hold LIVE (Resumed)
    // sessions; a not-live (Paused) session is idle regardless of its last
    // activity phase — except `Done`, which is always idle (finished its job).
    // Mirrors marsad_workstream::severity so the overview and the board agree.
    // Needs first in attention bucket.
    for sv in s
        .sessions
        .iter()
        .filter(|sv| sv.live && sv.state == Phase::Needs)
    {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s
        .sessions
        .iter()
        .filter(|sv| sv.live && sv.state == Phase::Error)
    {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s
        .sessions
        .iter()
        .filter(|sv| sv.live && sv.state == Phase::Working)
    {
        active.push(session_card(sv, &s.hosts));
    }
    // Idle = finished/idle (Done, or Starting folded into Done) any liveness, OR
    // not-live (Paused: user-stopped, or host-killed) — so a stopped/crashed and a
    // launched-but-idle session both appear in the overview instead of vanishing.
    for sv in s
        .sessions
        .iter()
        .filter(|sv| matches!(sv.state, Phase::Done | Phase::Starting) || !sv.live)
    {
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
// HostLink / LinkStatus — new input types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLink {
    pub a: String,
    pub b: String,
    pub status: LinkStatus,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// Board (Level 1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardView {
    pub summary: BoardSummary,
    pub host_chips: Vec<HostChip>,
    pub link_chips: Vec<LinkChip>,
    pub attention: Vec<WorkstreamCard>,
    pub all: Vec<WorkstreamCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSummary {
    pub workstreams: usize,
    pub sessions: usize,
    pub hosts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostChip {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkChip {
    pub a: String,
    pub b: String,
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamCard {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub head_state: Option<HeadState>,
    pub driver_dots: Vec<Driver>,
    pub host_chips: Vec<String>,
    pub session_count: usize,
    pub counts: StateCounts,
    pub needs_attention: bool,
    pub is_single_node: bool,
    /// Attention-card blocker headline — the pending gate's summary when a node
    /// needs the human. `None` → the UI falls back to `goal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Attention-card CTA label. `None` → the UI derives one from head_state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cta_label: Option<String>,
    /// A noteworthy end reason to surface on the card (a real error's reason, or a
    /// user Cancel). Aggregated across the workstream's nodes: a real error wins
    /// over a cancel. `None` when nothing noteworthy. Drives a small reason chip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<marsad_events::EndReason>,
}

/// Lowercase status string for a host, matching `marsad_hosts::HostStatus`.
fn host_status_str(status: &HostStatus) -> &'static str {
    match status {
        HostStatus::Connected => "connected",
        HostStatus::Connecting => "connecting",
        HostStatus::Reconnecting => "reconnecting",
        HostStatus::Unreachable => "unreachable",
    }
}

/// `"local"` if `host.is_local`, else `"wsl"` if the target starts with `"wsl:"`,
/// else `"remote"`.
fn host_kind_str(host: &Host) -> &'static str {
    if host.is_local {
        "local"
    } else if host.target.starts_with("wsl:") {
        "wsl"
    } else {
        "remote"
    }
}

/// Lowercase status string for a non-`Ok` `LinkStatus`. `Ok` links are filtered
/// out by the caller before a `LinkChip` is ever built, so this only covers the
/// two statuses that can actually reach a chip.
fn link_status_str(status: &LinkStatus) -> &'static str {
    if *status == LinkStatus::Down {
        "down"
    } else {
        "degraded"
    }
}

/// Build a `WorkstreamCard` for one workstream, deriving over its nodes/connections.
fn workstream_card(ws: &Workstream, nodes: &[Node], connections: &[Connection]) -> WorkstreamCard {
    let ws_nodes = nodes_of(&ws.id, nodes);
    let owned_nodes: Vec<Node> = ws_nodes.iter().map(|n| (*n).clone()).collect();
    let hs = head_state(&owned_nodes);
    let ws_conns = conns_of(&ws.id, connections);

    let needs_attention = matches!(hs, Some(HeadState::Needs) | Some(HeadState::Error))
        || ws_conns.iter().any(|c| c.gate.is_some());

    // For a single-node workstream with no explicit goal, fall back to the
    // node's name so the card headline is never blank.
    let goal = if ws.goal.is_empty() && owned_nodes.len() == 1 {
        owned_nodes[0].name.clone()
    } else {
        ws.goal.clone()
    };

    // When a node needs the human, surface its gate summary (carried on the
    // node's `status` by core::node_of) as the card's headline + an Answer CTA
    // so the attention card shows WHAT is asked, not just the workstream name.
    let (headline, cta_label) = if matches!(hs, Some(HeadState::Needs)) {
        match owned_nodes
            .iter()
            .find(|n| matches!(n.state, Phase::Needs) && !n.status.is_empty())
        {
            Some(n) => (Some(n.status.clone()), Some("Answer".to_string())),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    // Aggregate a noteworthy end reason for the card chip: a real error's reason
    // (MaxTurns / ExecutionError) wins over a user Cancel, so a genuine failure is
    // never masked by a cancel elsewhere in the workstream.
    let end_reason = owned_nodes
        .iter()
        .filter_map(|n| n.end_reason)
        .max_by_key(|r| match r {
            marsad_events::EndReason::MaxTurns | marsad_events::EndReason::ExecutionError => 1u8,
            marsad_events::EndReason::Cancelled => 0u8,
        });

    WorkstreamCard {
        id: ws.id.clone(),
        name: ws.name.clone(),
        goal,
        head_state: hs,
        driver_dots: driver_dots(&owned_nodes),
        host_chips: host_chips(&owned_nodes),
        session_count: owned_nodes.len(),
        counts: state_counts(&owned_nodes),
        needs_attention,
        is_single_node: owned_nodes.len() == 1,
        headline,
        cta_label,
        end_reason,
    }
}

/// Build the workstreams board: summary, host/link chips, and per-workstream cards.
pub fn board_view(
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
    hosts: &[Host],
    links: &[HostLink],
    active_profile: Option<&str>,
) -> BoardView {
    let filtered: Vec<&Workstream> = workstreams
        .iter()
        .filter(|w| match active_profile {
            Some(p) => w.profile.as_deref() == Some(p),
            None => true,
        })
        .collect();

    let all: Vec<WorkstreamCard> = filtered
        .iter()
        .map(|ws| workstream_card(ws, nodes, connections))
        .collect();

    let attention: Vec<WorkstreamCard> =
        all.iter().filter(|c| c.needs_attention).cloned().collect();

    let sessions: usize = filtered
        .iter()
        .map(|ws| nodes_of(&ws.id, nodes).len())
        .sum();

    let host_chips: Vec<HostChip> = hosts
        .iter()
        .map(|h| HostChip {
            id: h.id.clone(),
            label: h.label.clone(),
            kind: host_kind_str(h).to_string(),
            status: host_status_str(&h.status).to_string(),
        })
        .collect();

    let link_chips: Vec<LinkChip> = links
        .iter()
        .filter(|l| l.status != LinkStatus::Ok)
        .map(|l| LinkChip {
            a: l.a.clone(),
            b: l.b.clone(),
            status: link_status_str(&l.status).to_string(),
            note: l.note.clone(),
        })
        .collect();

    BoardView {
        summary: BoardSummary {
            workstreams: filtered.len(),
            sessions,
            hosts: hosts.len(),
        },
        host_chips,
        link_chips,
        attention,
        all,
    }
}

// ---------------------------------------------------------------------------
// Graph (Level 2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphView {
    pub ws_id: String,
    pub name: String,
    pub goal: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub inspector: Inspector,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    pub host: String,
    pub state: Phase,
    pub status: String,
    pub context_pct: u8,
    pub is_lead: bool,
    pub depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStatus {
    Normal,
    Consulting,
    Needs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: ConnKind,
    pub bi: bool,
    pub status: EdgeStatus,
    pub has_gate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspector {
    pub working: usize,
    pub needs: usize,
    pub pending_handoff: Option<HandoffSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffSummary {
    pub conn_id: String,
    pub from: String,
    pub to: String,
    pub payload: String,
    pub risk: String,
}

/// Build the Level-2 dependency graph for one workstream, or `None` if `ws_id`
/// does not match any workstream.
pub fn graph_view(
    ws_id: &str,
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
) -> Option<GraphView> {
    let ws = workstreams.iter().find(|w| w.id == ws_id)?;

    let ws_nodes = nodes_of(ws_id, nodes);
    let graph_nodes: Vec<GraphNode> = ws_nodes
        .iter()
        .map(|n| GraphNode {
            id: n.id.clone(),
            name: n.name.clone(),
            driver: n.driver,
            host: n.host.clone(),
            state: n.state.clone(),
            status: n.status.clone(),
            context_pct: n.context_pct,
            is_lead: n.lead,
            depth: dependency_depth(&n.id, ws_id, connections),
        })
        .collect();

    let ws_conns = conns_of(ws_id, connections);
    let graph_edges: Vec<GraphEdge> = ws_conns
        .iter()
        .map(|c| {
            let has_gate = c.gate.is_some();
            let status = if has_gate {
                EdgeStatus::Needs
            } else if c.bi {
                EdgeStatus::Consulting
            } else {
                EdgeStatus::Normal
            };
            GraphEdge {
                id: c.id.clone(),
                from: c.from.clone(),
                to: c.to.clone(),
                kind: c.kind,
                bi: c.bi,
                status,
                has_gate,
            }
        })
        .collect();

    let owned_nodes: Vec<Node> = ws_nodes.iter().map(|n| (*n).clone()).collect();
    let counts = state_counts(&owned_nodes);

    let pending_handoff = ws_conns.iter().find(|c| c.gate.is_some()).map(|c| {
        let gate = c.gate.as_ref().expect("filtered by gate.is_some()");
        HandoffSummary {
            conn_id: c.id.clone(),
            from: c.from.clone(),
            to: c.to.clone(),
            payload: gate.payload.clone(),
            risk: gate.risk.clone(),
        }
    });

    Some(GraphView {
        ws_id: ws.id.clone(),
        name: ws.name.clone(),
        goal: ws.goal.clone(),
        nodes: graph_nodes,
        edges: graph_edges,
        inspector: Inspector {
            working: counts.working,
            needs: counts.needs,
            pending_handoff,
        },
    })
}

// ---------------------------------------------------------------------------
// SLICE G — Workstream cost breakdown (Usage tab: "By host" / "By node")
// ---------------------------------------------------------------------------

/// One row of the workstream cost breakdown (Usage tab's "By host" / "By
/// node" tables): a group's total cost, the distinct drivers contributing to
/// it, its share of the workstream total, and whether it is backed only by
/// estimated (litellm/hand-table backfill) cost rather than any live,
/// agent-reported cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRow {
    /// Group identity: a host id (`by_host`) or a node/session id (`by_node`).
    pub key: String,
    /// Display label: the host id (`by_host`) or the node's display name
    /// (`by_node`).
    pub label: String,
    /// Distinct drivers contributing to this group, first-appearance order
    /// (mirrors `marsad_workstream::driver_dots`; a `by_node` row is always a
    /// single-element vec — the one node's driver).
    pub driver_dots: Vec<Driver>,
    /// Summed `cost_usd` of every contributing session (a node with no
    /// matching `SessionView` contributes `0.0`).
    pub cost_usd: f64,
    /// This group's share of the workstream total, `0.0..=100.0`. `0.0` for
    /// every row when the workstream total is `0.0` (avoids a NaN divide).
    pub pct: f32,
    /// U4 cost truthfulness (mirrors `SessionView.cost_estimated` one level
    /// up): `true` only when this group has at least one contributing
    /// session AND every one of them is estimated-only. A group with ANY
    /// live, agent-reported cost — or with no contributing sessions at all —
    /// is never labelled estimated; agent-reported cost is truth and must
    /// never be clobbered by the litellm/hand backfill table.
    pub estimated: bool,
}

/// The full cost breakdown for one workstream's sessions, grouped by host and
/// by node/session, for the Session Usage tab's "Cost breakdown" box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamCostBreakdown {
    /// Sum of `cost_usd` across every session in the workstream.
    pub total_cost_usd: f64,
    /// `true` only when every session in the workstream is estimated-only
    /// (see `CostRow::estimated`); `false` when the workstream has no
    /// sessions at all (no data is not "estimated", it's just empty).
    pub total_estimated: bool,
    /// Grouped by host id, sorted by cost descending.
    pub by_host: Vec<CostRow>,
    /// One row per node/session, sorted by cost descending.
    pub by_node: Vec<CostRow>,
}

/// `cost / total * 100`, or `0.0` when `total == 0.0` (avoids a NaN divide).
fn cost_pct(cost: f64, total: f64) -> f32 {
    if total == 0.0 {
        0.0
    } else {
        (cost / total * 100.0) as f32
    }
}

/// Sort rows by `cost_usd` descending; ties broken by `key` ascending for
/// determinism (grouping is otherwise order-dependent on a `HashMap`).
fn sort_cost_rows_desc(rows: &mut [CostRow]) {
    rows.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// Build the workstream-scoped cost breakdown (Usage tab's "By host" / "By
/// node" tables) for one workstream's sessions, or `None` if `ws_id` does not
/// match any workstream.
///
/// `nodes` and `sessions` are the FULL lists (across all workstreams), like
/// `graph_view`'s `nodes` parameter — this function filters to `ws_id`'s own
/// nodes internally (`nodes_of`) and joins each to the `SessionView` sharing
/// its `id` (a node with no matching session view contributes `0.0`/
/// not-estimated, mirroring `make_session_view`'s rule that no cost data at
/// all is not "estimated", it's just empty).
pub fn workstream_cost_breakdown(
    ws_id: &str,
    workstreams: &[Workstream],
    nodes: &[Node],
    sessions: &[SessionView],
) -> Option<WorkstreamCostBreakdown> {
    workstreams.iter().find(|w| w.id == ws_id)?;

    let rows: Vec<(&Node, f64, bool)> = nodes_of(ws_id, nodes)
        .into_iter()
        .map(|n| {
            let sv = sessions.iter().find(|s| s.id == n.id);
            let cost = sv.map(|s| s.cost_usd).unwrap_or(0.0);
            let estimated = sv.map(|s| s.cost_estimated).unwrap_or(false);
            (n, cost, estimated)
        })
        .collect();

    let total_cost_usd: f64 = rows.iter().map(|(_, cost, _)| cost).sum();
    let total_estimated = !rows.is_empty() && rows.iter().all(|(_, _, estimated)| *estimated);

    // by_node: one row per node, no aggregation.
    let mut by_node: Vec<CostRow> = rows
        .iter()
        .map(|(n, cost, estimated)| CostRow {
            key: n.id.clone(),
            label: n.name.clone(),
            driver_dots: vec![n.driver],
            cost_usd: *cost,
            pct: cost_pct(*cost, total_cost_usd),
            estimated: *estimated,
        })
        .collect();
    sort_cost_rows_desc(&mut by_node);

    // by_host: group rows by host id, first-appearance order.
    let mut host_order: Vec<String> = Vec::new();
    let mut host_cost: HashMap<String, f64> = HashMap::new();
    let mut host_all_estimated: HashMap<String, bool> = HashMap::new();
    let mut host_nodes: HashMap<String, Vec<Node>> = HashMap::new();
    for (n, cost, estimated) in &rows {
        if !host_cost.contains_key(&n.host) {
            host_order.push(n.host.clone());
        }
        *host_cost.entry(n.host.clone()).or_insert(0.0) += cost;
        let all_estimated_so_far = host_all_estimated.entry(n.host.clone()).or_insert(true);
        *all_estimated_so_far = *all_estimated_so_far && *estimated;
        host_nodes
            .entry(n.host.clone())
            .or_default()
            .push((*n).clone());
    }

    let mut by_host: Vec<CostRow> = host_order
        .into_iter()
        .map(|host| {
            let cost = host_cost[&host];
            CostRow {
                estimated: host_all_estimated[&host],
                driver_dots: driver_dots(&host_nodes[&host]),
                pct: cost_pct(cost, total_cost_usd),
                cost_usd: cost,
                label: host.clone(),
                key: host,
            }
        })
        .collect();
    sort_cost_rows_desc(&mut by_host);

    Some(WorkstreamCostBreakdown {
        total_cost_usd,
        total_estimated,
        by_host,
        by_node,
    })
}
