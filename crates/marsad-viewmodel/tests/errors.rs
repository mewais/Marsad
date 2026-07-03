/// errors.rs — empty snapshot; unknown workspace in rescope; no selection in
/// session_detail and gate_vm.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_viewmodel`.
/// That is the expected tests-first signal.
use marsad_events::Phase;
use marsad_viewmodel::{
    counts, gate_vm, group_overview, rescope, session_detail, sidebar_list, AppSnapshot, Filter,
    Host, HostStatus, SessionView, Tab, Workspace,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_snapshot() -> AppSnapshot {
    AppSnapshot {
        sessions: vec![],
        hosts: vec![],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    }
}

fn single_session_snapshot(phase: Phase) -> AppSnapshot {
    AppSnapshot {
        sessions: vec![SessionView {
            id: "s1".to_string(),
            name: "test-session".to_string(),
            host_id: "local".to_string(),
            project: "proj".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: phase,
            changed_files: 0,
            subagents: 0,
            running_subagents: 0,
            cost_usd: 0.0,
            tokens: 0,
            context_pct: 0.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],

            cost_estimated: false,

            compactions: 0,

            last_compaction: None,

            rate_limit: None,
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
            posture: "ask".to_string(),
        }],
        hosts: vec![Host {
            id: "local".to_string(),
            label: "localhost".to_string(),
            target: "this machine".to_string(),
            status: HostStatus::Connected,
            session_count: 1,
            is_local: true,
            removable: false,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
        }],
        workspaces: vec![],
        active_ws: None,
        selected_id: None, // intentionally no selection
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    }
}

// ---------------------------------------------------------------------------
// Empty snapshot — counts
// ---------------------------------------------------------------------------

#[test]
fn counts_empty_snapshot_all_zero() {
    let snap = empty_snapshot();
    let c = counts(&snap);
    assert_eq!(c.total, 0);
    assert_eq!(c.needs, 0);
    assert_eq!(c.errors, 0);
    assert_eq!(c.working, 0);
    assert_eq!(c.host_count, 0);
}

#[test]
fn counts_empty_snapshot_cost_is_zero_formatted() {
    let snap = empty_snapshot();
    let c = counts(&snap);
    assert_eq!(c.cost, "$0.00");
}

#[test]
fn counts_empty_snapshot_tokens_formatted() {
    let snap = empty_snapshot();
    let c = counts(&snap);
    // 0 tokens -> "0" (plain, not "0k")
    assert_eq!(c.tokens, "0");
}

// ---------------------------------------------------------------------------
// Empty snapshot — group_overview
// ---------------------------------------------------------------------------

#[test]
fn group_overview_empty_snapshot_all_groups_empty() {
    let snap = empty_snapshot();
    let groups = group_overview(&snap);
    assert!(groups.attention.is_empty());
    assert!(groups.active.is_empty());
    assert!(groups.idle.is_empty());
}

// ---------------------------------------------------------------------------
// Empty snapshot — sidebar_list
// ---------------------------------------------------------------------------

#[test]
fn sidebar_list_empty_snapshot_returns_empty() {
    let snap = empty_snapshot();
    let list = sidebar_list(&snap);
    assert!(list.is_empty());
}

// ---------------------------------------------------------------------------
// session_detail — no selection returns None
// ---------------------------------------------------------------------------

#[test]
fn session_detail_no_selection_returns_none() {
    let snap = empty_snapshot();
    let detail = session_detail(&snap);
    assert!(
        detail.is_none(),
        "session_detail should return None when no session is selected"
    );
}

#[test]
fn session_detail_no_selection_with_sessions_present_returns_none() {
    // selected_id = None even though sessions exist
    let snap = single_session_snapshot(Phase::Done);
    let detail = session_detail(&snap);
    assert!(detail.is_none());
}

// ---------------------------------------------------------------------------
// gate_vm — no selection returns None
// ---------------------------------------------------------------------------

#[test]
fn gate_vm_no_selection_returns_none() {
    let snap = empty_snapshot();
    let vm = gate_vm(&snap);
    assert!(
        vm.is_none(),
        "gate_vm should return None when no session is selected"
    );
}

#[test]
fn gate_vm_selection_but_no_gate_returns_none() {
    // s1 exists and is selected, but has no gate
    let mut snap = single_session_snapshot(Phase::Needs);
    snap.selected_id = Some("s1".to_string());
    // gate is None on the session (set by single_session_snapshot)
    let vm = gate_vm(&snap);
    assert!(
        vm.is_none(),
        "gate_vm should return None when selected session has no gate"
    );
}

// ---------------------------------------------------------------------------
// rescope — unknown workspace leaves snapshot unchanged
// ---------------------------------------------------------------------------

#[test]
fn rescope_unknown_workspace_returns_unchanged_snapshot() {
    let snap = AppSnapshot {
        sessions: vec![
            SessionView {
                id: "s1".to_string(),
                name: "a".to_string(),
                host_id: "local".to_string(),
                project: "p".to_string(),
                branch: "main".to_string(),
                model: "claude-opus-4.6".to_string(),
                state: Phase::Done,
                changed_files: 0,
                subagents: 0,
                running_subagents: 0,
                cost_usd: 0.0,
                tokens: 0,
                context_pct: 0.0,
                gate: None,
                transcript: vec![],
                changes: vec![],

                model_usage: vec![],

                cost_estimated: false,

                compactions: 0,

                last_compaction: None,

                rate_limit: None,
                tools: vec![],
                mcp_servers: vec![],
                slash_commands: vec![],
                posture: "ask".to_string(),
            },
            SessionView {
                id: "s2".to_string(),
                name: "b".to_string(),
                host_id: "local".to_string(),
                project: "p".to_string(),
                branch: "dev".to_string(),
                model: "claude-opus-4.6".to_string(),
                state: Phase::Working,
                changed_files: 1,
                subagents: 0,
                running_subagents: 0,
                cost_usd: 0.5,
                tokens: 100,
                context_pct: 20.0,
                gate: None,
                transcript: vec![],
                changes: vec![],

                model_usage: vec![],

                cost_estimated: false,

                compactions: 0,

                last_compaction: None,

                rate_limit: None,
                tools: vec![],
                mcp_servers: vec![],
                slash_commands: vec![],
                posture: "ask".to_string(),
            },
        ],
        hosts: vec![Host {
            id: "local".to_string(),
            label: "localhost".to_string(),
            target: "this machine".to_string(),
            status: HostStatus::Connected,
            session_count: 2,
            is_local: true,
            removable: false,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
        }],
        workspaces: vec![Workspace {
            id: "ws-real".to_string(),
            name: "Real WS".to_string(),
            session_ids: vec!["s1".to_string()],
            saved_at: String::new(),
        }],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };

    let result = rescope("ws-does-not-exist", &snap);
    // Unknown workspace: sessions unchanged
    assert_eq!(result.sessions.len(), snap.sessions.len());
    assert_eq!(result.sessions[0].id, "s1");
    assert_eq!(result.sessions[1].id, "s2");
}

#[test]
fn rescope_empty_workspace_session_ids_gives_no_sessions() {
    let snap = AppSnapshot {
        sessions: vec![SessionView {
            id: "s1".to_string(),
            name: "a".to_string(),
            host_id: "local".to_string(),
            project: "p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Done,
            changed_files: 0,
            subagents: 0,
            running_subagents: 0,
            cost_usd: 0.0,
            tokens: 0,
            context_pct: 0.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],

            cost_estimated: false,

            compactions: 0,

            last_compaction: None,

            rate_limit: None,
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
            posture: "ask".to_string(),
        }],
        hosts: vec![],
        workspaces: vec![Workspace {
            id: "empty-ws".to_string(),
            name: "Empty".to_string(),
            session_ids: vec![],
            saved_at: String::new(),
        }],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };

    let result = rescope("empty-ws", &snap);
    assert_eq!(
        result.sessions.len(),
        0,
        "workspace with no session_ids should produce empty sessions"
    );
}
