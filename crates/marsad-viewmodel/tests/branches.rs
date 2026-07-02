/// branches.rs — table-driven cases closing remaining regions to 100%.
/// Covers: each Filter variant; case-insensitive search (name/project/host_label/branch);
/// empty search vs match vs no-match; full state-sort ordering; each SlashCmd + Unknown;
/// slash_catalog empty/prefix/no-match; ctx_warn boundary 74 vs 75.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_viewmodel`.
/// That is the expected tests-first signal.
use marsad_events::Phase;
use marsad_viewmodel::{
    parse_slash, session_detail, sidebar_list, slash_catalog, AppSnapshot, Filter, Host,
    HostStatus, SessionView, SlashCmd, Tab,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn make_session(
    id: &str,
    name: &str,
    host_id: &str,
    project: &str,
    branch: &str,
    phase: Phase,
) -> SessionView {
    SessionView {
        id: id.to_string(),
        name: name.to_string(),
        host_id: host_id.to_string(),
        project: project.to_string(),
        branch: branch.to_string(),
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
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        posture: "ask".to_string(),
    }
}

fn host(id: &str, label: &str) -> Host {
    Host {
        id: id.to_string(),
        label: label.to_string(),
        target: "this machine".to_string(),
        status: HostStatus::Connected,
        session_count: 1,
        is_local: true,
        removable: false,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
    }
}

fn all_states_snapshot() -> AppSnapshot {
    AppSnapshot {
        sessions: vec![
            make_session("sN", "needs-sess", "local", "proj", "main", Phase::Needs),
            make_session("sE", "error-sess", "local", "proj", "main", Phase::Error),
            make_session(
                "sW",
                "working-sess",
                "local",
                "proj",
                "main",
                Phase::Working,
            ),
            make_session("sS", "start-sess", "local", "proj", "main", Phase::Starting),
            make_session("sD", "done-sess", "local", "proj", "main", Phase::Done),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    }
}

// ---------------------------------------------------------------------------
// Filter::All — keeps all states
// ---------------------------------------------------------------------------

#[test]
fn filter_all_returns_every_session() {
    let snap = all_states_snapshot();
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 5);
}

// ---------------------------------------------------------------------------
// Filter::Needs — only needs sessions
// ---------------------------------------------------------------------------

#[test]
fn filter_needs_returns_only_needs_sessions() {
    let mut snap = all_states_snapshot();
    snap.filter = Filter::Needs;
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].state, Phase::Needs);
}

#[test]
fn filter_needs_excludes_other_states() {
    let mut snap = all_states_snapshot();
    snap.filter = Filter::Needs;
    let list = sidebar_list(&snap);
    let states: Vec<Phase> = list.iter().map(|r| r.state.clone()).collect();
    assert!(!states.contains(&Phase::Error));
    assert!(!states.contains(&Phase::Working));
    assert!(!states.contains(&Phase::Done));
}

// ---------------------------------------------------------------------------
// Filter::Working — only working sessions
// ---------------------------------------------------------------------------

#[test]
fn filter_working_returns_only_working_sessions() {
    let mut snap = all_states_snapshot();
    snap.filter = Filter::Working;
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].state, Phase::Working);
}

// ---------------------------------------------------------------------------
// Filter::Error — only error sessions
// ---------------------------------------------------------------------------

#[test]
fn filter_error_returns_only_error_sessions() {
    let mut snap = all_states_snapshot();
    snap.filter = Filter::Error;
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].state, Phase::Error);
}

// ---------------------------------------------------------------------------
// Search — case-insensitive over name
// ---------------------------------------------------------------------------

#[test]
fn search_matches_name_case_insensitively() {
    let mut snap = all_states_snapshot();
    snap.search = "NEEDS".to_string(); // uppercase
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "sN");
}

#[test]
fn search_empty_string_returns_all() {
    let mut snap = all_states_snapshot();
    snap.search = String::new();
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 5);
}

#[test]
fn search_no_match_returns_empty() {
    let mut snap = all_states_snapshot();
    snap.search = "xyzzy-no-match".to_string();
    let list = sidebar_list(&snap);
    assert!(list.is_empty());
}

// ---------------------------------------------------------------------------
// Search — matches on project field
// ---------------------------------------------------------------------------

#[test]
fn search_matches_project_case_insensitively() {
    let snap = AppSnapshot {
        sessions: vec![
            make_session(
                "s1",
                "my-service",
                "local",
                "MyBackend",
                "main",
                Phase::Done,
            ),
            make_session("s2", "other", "local", "frontend", "main", Phase::Done),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: "mybackend".to_string(),
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "s1");
}

// ---------------------------------------------------------------------------
// Search — matches on host_label field
// ---------------------------------------------------------------------------

#[test]
fn search_matches_host_label_case_insensitively() {
    let snap = AppSnapshot {
        sessions: vec![
            make_session("s1", "job1", "hetz", "proj", "main", Phase::Done),
            make_session("s2", "job2", "local", "proj", "main", Phase::Done),
        ],
        hosts: vec![host("hetz", "Hetzner-FRA1"), host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: "hetzner".to_string(),
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "s1");
}

// ---------------------------------------------------------------------------
// Search — matches on branch field
// ---------------------------------------------------------------------------

#[test]
fn search_matches_branch_case_insensitively() {
    let snap = AppSnapshot {
        sessions: vec![
            make_session(
                "s1",
                "svc",
                "local",
                "proj",
                "feature/AUTH-123",
                Phase::Done,
            ),
            make_session("s2", "svc2", "local", "proj", "main", Phase::Done),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: "auth-123".to_string(),
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "s1");
}

// ---------------------------------------------------------------------------
// State-sort order: {needs:0, error:1, working:2, starting:3, done:4}
// Provide all 5 states in reverse order, verify sort is correct.
// ---------------------------------------------------------------------------

#[test]
fn state_sort_full_ordering_all_five_states() {
    // Insert in reverse order (done, starting, working, error, needs) to force sort
    let snap = AppSnapshot {
        sessions: vec![
            make_session("sD", "done", "local", "p", "main", Phase::Done),
            make_session("sS", "start", "local", "p", "main", Phase::Starting),
            make_session("sW", "working", "local", "p", "main", Phase::Working),
            make_session("sE", "error", "local", "p", "main", Phase::Error),
            make_session("sN", "needs", "local", "p", "main", Phase::Needs),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 5);
    assert_eq!(list[0].state, Phase::Needs, "index 0 = needs");
    assert_eq!(list[1].state, Phase::Error, "index 1 = error");
    assert_eq!(list[2].state, Phase::Working, "index 2 = working");
    assert_eq!(list[3].state, Phase::Starting, "index 3 = starting");
    assert_eq!(list[4].state, Phase::Done, "index 4 = done");
}

#[test]
fn state_sort_needs_always_before_error_before_working() {
    let snap = all_states_snapshot();
    let list = sidebar_list(&snap);
    let order: Vec<Phase> = list.iter().map(|r| r.state.clone()).collect();
    let pos = |p: &Phase| order.iter().position(|x| x == p).unwrap();
    assert!(pos(&Phase::Needs) < pos(&Phase::Error));
    assert!(pos(&Phase::Error) < pos(&Phase::Working));
    assert!(pos(&Phase::Working) < pos(&Phase::Starting));
    assert!(pos(&Phase::Starting) < pos(&Phase::Done));
}

// ---------------------------------------------------------------------------
// ctx_warn boundary: exactly 74 vs exactly 75
// ---------------------------------------------------------------------------

#[test]
fn ctx_warn_false_at_74_pct() {
    let snap = AppSnapshot {
        sessions: vec![SessionView {
            id: "s1".to_string(),
            name: "svc".to_string(),
            host_id: "local".to_string(),
            project: "p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Working,
            changed_files: 0,
            subagents: 0,
            running_subagents: 0,
            cost_usd: 0.0,
            tokens: 0,
            context_pct: 74.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
            posture: "ask".to_string(),
        }],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: Some("s1".to_string()),
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };
    let detail = session_detail(&snap).unwrap();
    assert!(!detail.ctx_warn, "ctx_warn must be false at 74%");
}

#[test]
fn ctx_warn_true_at_75_pct() {
    let snap = AppSnapshot {
        sessions: vec![SessionView {
            id: "s1".to_string(),
            name: "svc".to_string(),
            host_id: "local".to_string(),
            project: "p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Working,
            changed_files: 0,
            subagents: 0,
            running_subagents: 0,
            cost_usd: 0.0,
            tokens: 0,
            context_pct: 75.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
            posture: "ask".to_string(),
        }],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: Some("s1".to_string()),
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };
    let detail = session_detail(&snap).unwrap();
    assert!(detail.ctx_warn, "ctx_warn must be true at exactly 75%");
}

// ---------------------------------------------------------------------------
// parse_slash — ALL 11 variants + Unknown
// ---------------------------------------------------------------------------

#[test]
fn parse_slash_all_11_known_commands() {
    let cases = vec![
        ("/clear", SlashCmd::Clear),
        ("/compact", SlashCmd::Compact),
        ("/cost", SlashCmd::Cost),
        ("/model", SlashCmd::Model),
        ("/config", SlashCmd::Config),
        ("/init", SlashCmd::Init),
        ("/review", SlashCmd::Review),
        ("/agents", SlashCmd::Agents),
        ("/resume", SlashCmd::Resume),
        ("/terminal", SlashCmd::Terminal),
        ("/help", SlashCmd::Help),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_slash(input),
            expected,
            "parse_slash({input:?}) should be {expected:?}"
        );
    }
}

#[test]
fn parse_slash_unknown_wraps_string() {
    match parse_slash("/foobar") {
        SlashCmd::Unknown(s) => assert_eq!(s, "/foobar"),
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn parse_slash_unknown_empty_string() {
    match parse_slash("") {
        SlashCmd::Unknown(s) => assert_eq!(s, ""),
        other => panic!("expected Unknown for empty string, got {other:?}"),
    }
}

#[test]
fn parse_slash_unknown_no_slash_prefix() {
    match parse_slash("clear") {
        SlashCmd::Unknown(s) => assert_eq!(s, "clear"),
        other => panic!("expected Unknown for bare word without slash, got {other:?}"),
    }
}

#[test]
fn parse_slash_unknown_garbage() {
    match parse_slash("/notacommand") {
        SlashCmd::Unknown(_) => {}
        other => panic!("expected Unknown, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// slash_catalog — empty query / prefix / no-match / top item
// ---------------------------------------------------------------------------

#[test]
fn slash_catalog_empty_query_returns_11_entries() {
    let entries = slash_catalog("");
    assert_eq!(entries.len(), 11);
}

#[test]
fn slash_catalog_all_entries_have_slash_prefix() {
    let entries = slash_catalog("");
    for entry in &entries {
        assert!(
            entry.name.starts_with('/'),
            "slash entry name must start with '/': {:?}",
            entry.name
        );
    }
}

#[test]
fn slash_catalog_top_item_first_in_empty_query() {
    let entries = slash_catalog("");
    // /clear is the first in spec order
    assert_eq!(entries[0].name, "/clear");
}

#[test]
fn slash_catalog_prefix_co_matches_three() {
    let entries = slash_catalog("/co");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    // /compact, /cost, /config all start with /co
    assert!(names.contains(&"/compact"), "/compact must match /co");
    assert!(names.contains(&"/cost"), "/cost must match /co");
    assert!(names.contains(&"/config"), "/config must match /co");
    // /clear does NOT start with /co
    assert!(!names.contains(&"/clear"), "/clear must not match /co");
}

#[test]
fn slash_catalog_prefix_help_matches_one() {
    let entries = slash_catalog("/h");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"/help"));
}

#[test]
fn slash_catalog_prefix_no_match_empty() {
    let entries = slash_catalog("/zzz");
    assert!(
        entries.is_empty(),
        "unmatched prefix should return empty list"
    );
}

#[test]
fn slash_catalog_prefix_r_matches_review_resume() {
    let entries = slash_catalog("/r");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"/review"));
    assert!(names.contains(&"/resume"));
}

#[test]
fn slash_catalog_entry_has_name_and_description() {
    let entries = slash_catalog("");
    for entry in &entries {
        assert!(
            !entry.name.is_empty(),
            "every entry must have a non-empty name"
        );
        // desc field should exist; we just ensure it's accessible
        let _desc: &str = &entry.desc;
    }
}

#[test]
fn slash_catalog_exact_slash_clear() {
    let entries = slash_catalog("/clear");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "/clear");
}

#[test]
fn slash_catalog_order_stable_for_empty_query() {
    // The 11 commands in spec order:
    let expected_order = [
        "/clear",
        "/compact",
        "/cost",
        "/model",
        "/config",
        "/init",
        "/review",
        "/agents",
        "/resume",
        "/terminal",
        "/help",
    ];
    let entries = slash_catalog("");
    assert_eq!(entries.len(), 11);
    for (i, expected_name) in expected_order.iter().enumerate() {
        assert_eq!(
            entries[i].name, *expected_name,
            "slash_catalog entry {i} should be {expected_name}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filter + search interaction: filter reduces first, then search
// ---------------------------------------------------------------------------

#[test]
fn filter_and_search_combined_narrows_correctly() {
    let snap = AppSnapshot {
        sessions: vec![
            make_session("s1", "api-needs", "local", "proj", "main", Phase::Needs),
            make_session("s2", "api-working", "local", "proj", "main", Phase::Working),
            make_session("s3", "other-needs", "local", "proj", "main", Phase::Needs),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::Needs,     // only needs
        search: "api".to_string(), // further narrows by name
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    // After filter=Needs: s1, s3. After search="api": s1 only.
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "s1");
}

// ---------------------------------------------------------------------------
// Whitespace-trimmed search
// ---------------------------------------------------------------------------

#[test]
fn search_with_leading_trailing_whitespace_trims_correctly() {
    let snap = AppSnapshot {
        sessions: vec![
            make_session("s1", "trimtest", "local", "proj", "main", Phase::Done),
            make_session("s2", "other", "local", "proj", "main", Phase::Done),
        ],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: "  trimtest  ".to_string(),
        tab: Tab::Activity,
    };
    let list = sidebar_list(&snap);
    assert_eq!(
        list.len(),
        1,
        "whitespace-trimmed search should match 'trimtest'"
    );
    assert_eq!(list[0].id, "s1");
}

// ---------------------------------------------------------------------------
// session_detail — ctx_warn at 100% is also true
// ---------------------------------------------------------------------------

#[test]
fn ctx_warn_true_at_100_pct() {
    let snap = AppSnapshot {
        sessions: vec![SessionView {
            id: "s1".to_string(),
            name: "svc".to_string(),
            host_id: "local".to_string(),
            project: "p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Working,
            changed_files: 0,
            subagents: 0,
            running_subagents: 0,
            cost_usd: 0.0,
            tokens: 0,
            context_pct: 100.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
            posture: "ask".to_string(),
        }],
        hosts: vec![host("local", "localhost")],
        workspaces: vec![],
        active_ws: None,
        selected_id: Some("s1".to_string()),
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };
    let detail = session_detail(&snap).unwrap();
    assert!(detail.ctx_warn, "ctx_warn must be true at 100%");
}

// ---------------------------------------------------------------------------
// AppSnapshot and SessionView derive Clone + PartialEq + Debug
// ---------------------------------------------------------------------------

#[test]
fn snapshot_derives_clone_and_partial_eq() {
    let snap = AppSnapshot {
        sessions: vec![],
        hosts: vec![],
        workspaces: vec![],
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };
    let cloned = snap.clone();
    assert_eq!(snap, cloned);
}

#[test]
fn filter_derives_partial_eq_and_clone() {
    let f1 = Filter::Needs;
    let f2 = f1.clone();
    assert_eq!(f1, f2);
    assert_ne!(Filter::All, Filter::Error);
}

#[test]
fn slash_cmd_unknown_derives_partial_eq_with_same_string() {
    let a = SlashCmd::Unknown("x".to_string());
    let b = SlashCmd::Unknown("x".to_string());
    assert_eq!(a, b);
    let c = SlashCmd::Unknown("y".to_string());
    assert_ne!(a, c);
}
