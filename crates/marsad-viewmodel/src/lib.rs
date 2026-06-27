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
    /// Tools available in this session (from the system/init event).
    #[serde(default)]
    pub tools: Vec<String>,
    /// MCP servers connected in this session (from the system/init event).
    #[serde(default)]
    pub mcp_servers: Vec<marsad_proto::McpServerInfo>,
    /// Slash commands available in this session (from the system/init event).
    #[serde(default)]
    pub slash_commands: Vec<marsad_proto::SlashCommandInfo>,
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

    // Needs first in attention bucket
    for sv in s.sessions.iter().filter(|sv| sv.state == Phase::Needs) {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s.sessions.iter().filter(|sv| sv.state == Phase::Error) {
        attention.push(session_card(sv, &s.hosts));
    }
    for sv in s
        .sessions
        .iter()
        .filter(|sv| matches!(sv.state, Phase::Working | Phase::Starting))
    {
        active.push(session_card(sv, &s.hosts));
    }
    // Done plus the not-live phases (Stale = stopped, Detached) so a stopped
    // session still appears in the overview instead of vanishing from it.
    for sv in s
        .sessions
        .iter()
        .filter(|sv| matches!(sv.state, Phase::Done | Phase::Stale | Phase::Detached))
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
// SlashEntry + slash_catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlashEntry {
    /// Command name including the leading '/'.
    pub name: String,
    /// Human-readable description.
    pub desc: String,
}

/// The canonical 11-command catalog in spec order.
fn all_slash_entries() -> [SlashEntry; 11] {
    [
        SlashEntry {
            name: "/clear".to_string(),
            desc: "Clear conversation history".to_string(),
        },
        SlashEntry {
            name: "/compact".to_string(),
            desc: "Compact context to save tokens".to_string(),
        },
        SlashEntry {
            name: "/cost".to_string(),
            desc: "Show token usage and cost".to_string(),
        },
        SlashEntry {
            name: "/model".to_string(),
            desc: "Switch the active model".to_string(),
        },
        SlashEntry {
            name: "/config".to_string(),
            desc: "Open configuration".to_string(),
        },
        SlashEntry {
            name: "/init".to_string(),
            desc: "Initialize CLAUDE.md for this project".to_string(),
        },
        SlashEntry {
            name: "/review".to_string(),
            desc: "Review recent changes".to_string(),
        },
        SlashEntry {
            name: "/agents".to_string(),
            desc: "List running sub-agents".to_string(),
        },
        SlashEntry {
            name: "/resume".to_string(),
            desc: "Resume a previous conversation".to_string(),
        },
        SlashEntry {
            name: "/terminal".to_string(),
            desc: "Open integrated terminal".to_string(),
        },
        SlashEntry {
            name: "/help".to_string(),
            desc: "Show available commands".to_string(),
        },
    ]
}

/// Return a filtered slash catalog matching the given query prefix.
///
/// - Empty query → all 11 entries in spec order (top item first).
/// - Non-empty query → prefix-match on name, preserving spec order.
pub fn slash_catalog(query: &str) -> Vec<SlashEntry> {
    let entries = all_slash_entries();
    if query.is_empty() {
        return entries.into_iter().collect();
    }
    entries
        .into_iter()
        .filter(|e| e.name.starts_with(query))
        .collect()
}
