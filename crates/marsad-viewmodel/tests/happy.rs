/// happy.rs — nominal paths for counts, group_overview, sidebar_list, session_detail,
/// gate_vm, rescope, parse_slash, and slash_catalog on a representative snapshot.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_viewmodel`.
/// That is the expected tests-first signal.
use marsad_diff::{DiffLine, Sig};
use marsad_events::Phase;
use marsad_gates::{Gate, PlanStep};
use marsad_viewmodel::{
    counts, gate_vm, group_overview, parse_slash, rescope, session_detail, sidebar_list,
    slash_catalog, AppSnapshot, Filter, Host, HostStatus, SessionView, SlashCmd, Tab, Workspace,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn host_local() -> Host {
    Host {
        id: "local".to_string(),
        label: "localhost".to_string(),
        target: "this machine".to_string(),
        status: HostStatus::Connected,
        session_count: 3,
        is_local: true,
        removable: false,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
    }
}

fn host_remote() -> Host {
    Host {
        id: "hetzner".to_string(),
        label: "hetzner-fra1".to_string(),
        target: "user@hetzner-fra1".to_string(),
        status: HostStatus::Connected,
        session_count: 1,
        is_local: false,
        removable: true,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
    }
}

/// Build a representative snapshot with sessions in every state: needs, error, working,
/// starting, done. Two distinct hosts. Costs and tokens set for easy arithmetic.
fn representative_snapshot() -> AppSnapshot {
    let sessions = vec![
        SessionView {
            id: "s1".to_string(),
            name: "api-gateway".to_string(),
            host_id: "local".to_string(),
            project: "backend".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Needs,
            changed_files: 2,
            subagents: 0,
            cost_usd: 1.00,
            tokens: 1000,
            context_pct: 10.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
        },
        SessionView {
            id: "s2".to_string(),
            name: "payments-svc".to_string(),
            host_id: "local".to_string(),
            project: "backend".to_string(),
            branch: "fix/bug".to_string(),
            model: "claude-sonnet-4.6".to_string(),
            state: Phase::Error,
            changed_files: 0,
            subagents: 1,
            cost_usd: 0.50,
            tokens: 2000,
            context_pct: 50.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
        },
        SessionView {
            id: "s3".to_string(),
            name: "etl-pipeline".to_string(),
            host_id: "hetzner".to_string(),
            project: "data".to_string(),
            branch: "develop".to_string(),
            model: "claude-haiku-4".to_string(),
            state: Phase::Working,
            changed_files: 5,
            subagents: 0,
            cost_usd: 0.25,
            tokens: 500,
            context_pct: 80.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
        },
        SessionView {
            id: "s4".to_string(),
            name: "ml-trainer".to_string(),
            host_id: "hetzner".to_string(),
            project: "ml".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4.6".to_string(),
            state: Phase::Starting,
            changed_files: 0,
            subagents: 0,
            cost_usd: 0.00,
            tokens: 0,
            context_pct: 0.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
        },
        SessionView {
            id: "s5".to_string(),
            name: "auth-service".to_string(),
            host_id: "local".to_string(),
            project: "backend".to_string(),
            branch: "main".to_string(),
            model: "claude-sonnet-4.6".to_string(),
            state: Phase::Done,
            changed_files: 3,
            subagents: 2,
            cost_usd: 2.25,
            tokens: 3500,
            context_pct: 60.0,
            gate: None,
            transcript: vec![],
            changes: vec![],

            model_usage: vec![],
        },
    ];

    AppSnapshot {
        sessions,
        hosts: vec![host_local(), host_remote()],
        workspaces: vec![],
        active_ws: None,
        selected_id: Some("s1".to_string()),
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    }
}

// ---------------------------------------------------------------------------
// counts
// ---------------------------------------------------------------------------

#[test]
fn counts_tallies_needs_errors_working() {
    let snap = representative_snapshot();
    let c = counts(&snap);
    assert_eq!(c.needs, 1, "one needs session");
    assert_eq!(c.errors, 1, "one error session");
    assert_eq!(c.working, 1, "one working session");
}

#[test]
fn counts_total_is_all_sessions() {
    let snap = representative_snapshot();
    let c = counts(&snap);
    assert_eq!(c.total, 5, "total = all 5 sessions");
}

#[test]
fn counts_cost_formatted_as_dollar_fixed_two() {
    let snap = representative_snapshot();
    let c = counts(&snap);
    // 1.00 + 0.50 + 0.25 + 0.00 + 2.25 = 4.00
    assert_eq!(c.cost, "$4.00");
}

#[test]
fn counts_tokens_via_fmt_tokens() {
    let snap = representative_snapshot();
    let c = counts(&snap);
    // 1000 + 2000 + 500 + 0 + 3500 = 7000 -> "7k"
    assert_eq!(c.tokens, "7k");
}

#[test]
fn counts_host_count_is_distinct_hosts() {
    let snap = representative_snapshot();
    let c = counts(&snap);
    assert_eq!(c.host_count, 2, "two distinct hosts: local + hetzner");
}

// ---------------------------------------------------------------------------
// group_overview
// ---------------------------------------------------------------------------

#[test]
fn group_overview_attention_is_needs_plus_error() {
    let snap = representative_snapshot();
    let groups = group_overview(&snap);
    assert_eq!(groups.attention.len(), 2);
}

#[test]
fn group_overview_needs_comes_before_error_in_attention() {
    let snap = representative_snapshot();
    let groups = group_overview(&snap);
    // needs must appear before error
    let first_state = &groups.attention[0].state;
    let second_state = &groups.attention[1].state;
    assert_eq!(
        *first_state,
        Phase::Needs,
        "needs must be first in attention"
    );
    assert_eq!(
        *second_state,
        Phase::Error,
        "error must be second in attention"
    );
}

#[test]
fn group_overview_active_is_working_plus_starting() {
    let snap = representative_snapshot();
    let groups = group_overview(&snap);
    assert_eq!(groups.active.len(), 2);
    let states: Vec<Phase> = groups.active.iter().map(|c| c.state.clone()).collect();
    assert!(states.contains(&Phase::Working));
    assert!(states.contains(&Phase::Starting));
}

#[test]
fn group_overview_idle_is_done_only() {
    let snap = representative_snapshot();
    let groups = group_overview(&snap);
    assert_eq!(groups.idle.len(), 1);
    assert_eq!(groups.idle[0].state, Phase::Done);
}

#[test]
fn group_overview_session_card_name_matches() {
    let snap = representative_snapshot();
    let groups = group_overview(&snap);
    // first attention card (needs) is "api-gateway"
    assert_eq!(groups.attention[0].name, "api-gateway");
    // second attention card (error) is "payments-svc"
    assert_eq!(groups.attention[1].name, "payments-svc");
}

// ---------------------------------------------------------------------------
// sidebar_list — Filter::All with no search gives all sessions sorted
// ---------------------------------------------------------------------------

#[test]
fn sidebar_list_all_filter_returns_all_sessions() {
    let snap = representative_snapshot();
    let list = sidebar_list(&snap);
    assert_eq!(list.len(), 5);
}

#[test]
fn sidebar_list_sorted_by_state_order() {
    let snap = representative_snapshot();
    let list = sidebar_list(&snap);
    // expected order: needs(0), error(1), working(2), starting(3), done(4)
    let states: Vec<Phase> = list.iter().map(|r| r.state.clone()).collect();
    assert_eq!(states[0], Phase::Needs);
    assert_eq!(states[1], Phase::Error);
    assert_eq!(states[2], Phase::Working);
    assert_eq!(states[3], Phase::Starting);
    assert_eq!(states[4], Phase::Done);
}

// ---------------------------------------------------------------------------
// session_detail — selected session
// ---------------------------------------------------------------------------

#[test]
fn session_detail_returns_some_when_session_selected() {
    let snap = representative_snapshot();
    let detail = session_detail(&snap);
    assert!(
        detail.is_some(),
        "should return Some when a session is selected"
    );
}

#[test]
fn session_detail_has_correct_name_for_selection() {
    let snap = representative_snapshot();
    let detail = session_detail(&snap).unwrap();
    assert_eq!(detail.name, "api-gateway");
}

#[test]
fn session_detail_ctx_warn_false_below_75() {
    // s1 has context_pct = 10.0 — below 75
    let snap = representative_snapshot();
    let detail = session_detail(&snap).unwrap();
    assert!(
        !detail.ctx_warn,
        "ctx_warn should be false when context_pct < 75"
    );
}

#[test]
fn session_detail_ctx_warn_true_at_75() {
    // Build a snap selecting s3 (context_pct = 80.0 >= 75)
    let mut snap = representative_snapshot();
    snap.selected_id = Some("s3".to_string());
    let detail = session_detail(&snap).unwrap();
    assert!(
        detail.ctx_warn,
        "ctx_warn should be true when context_pct >= 75"
    );
}

// ---------------------------------------------------------------------------
// gate_vm — selected session with a gate
// ---------------------------------------------------------------------------

#[test]
fn gate_vm_returns_some_for_permission_gate() {
    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "npx prisma migrate deploy".to_string(),
        cwd: "/work".to_string(),
        risk: "medium".to_string(),
        explanation: "needs DB access".to_string(),
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap);
    assert!(
        vm.is_some(),
        "gate_vm should return Some when selected session has a gate"
    );
}

#[test]
fn gate_vm_permission_kind_and_icon() {
    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "directory listing".to_string(),
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap).unwrap();
    assert_eq!(vm.kind, "Permission");
    assert_eq!(vm.icon, "\u{2318}"); // ⌘
    assert!(vm.is_permission);
    assert!(!vm.is_plan);
    assert!(!vm.is_edit);
}

#[test]
fn gate_vm_plan_kind_and_icon() {
    let gate = Gate::Plan {
        summary: "Refactor auth module".to_string(),
        steps: vec![
            PlanStep {
                title: "Step 1".to_string(),
                detail: "do this first".to_string(),
            },
            PlanStep {
                title: "Step 2".to_string(),
                detail: "then do this".to_string(),
            },
        ],
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap).unwrap();
    assert_eq!(vm.kind, "Plan review");
    assert_eq!(vm.icon, "\u{25a1}"); // ❑
    assert!(!vm.is_permission);
    assert!(vm.is_plan);
    assert!(!vm.is_edit);
}

#[test]
fn gate_vm_edit_kind_and_icon() {
    let diff_lines = vec![
        DiffLine {
            sig: Sig::Del,
            text: "old code".to_string(),
            ln: "1".to_string(),
        },
        DiffLine {
            sig: Sig::Add,
            text: "new code".to_string(),
            ln: "1".to_string(),
        },
    ];
    let gate = Gate::Edit {
        file: "src/lib.rs".to_string(),
        diff: diff_lines,
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap).unwrap();
    assert_eq!(vm.kind, "File change");
    assert_eq!(vm.icon, "\u{00b1}"); // ±
    assert!(!vm.is_permission);
    assert!(!vm.is_plan);
    assert!(vm.is_edit);
}

#[test]
fn gate_vm_edit_has_diff_rows() {
    let diff_lines = vec![
        DiffLine {
            sig: Sig::Del,
            text: "old".to_string(),
            ln: "1".to_string(),
        },
        DiffLine {
            sig: Sig::Add,
            text: "new".to_string(),
            ln: "1".to_string(),
        },
    ];
    let gate = Gate::Edit {
        file: "foo.rs".to_string(),
        diff: diff_lines,
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap).unwrap();
    // edit gate must produce diff_rows via split_side_by_side
    assert!(!vm.diff_rows.is_empty(), "edit gate must produce diff rows");
}

#[test]
fn gate_vm_question_kind_and_icon() {
    use marsad_gates::{Gate, QuestionOption, QuestionPrompt};
    let gate = Gate::Question {
        request_id: "req-q-vm".to_string(),
        questions: vec![QuestionPrompt {
            question: "Best approach?".to_string(),
            header: "Strategy".to_string(),
            multi_select: false,
            options: vec![QuestionOption {
                label: "A".to_string(),
                description: "first".to_string(),
            }],
        }],
    };
    let mut snap = representative_snapshot();
    snap.sessions[0].gate = Some(gate);
    snap.selected_id = Some("s1".to_string());
    let vm = gate_vm(&snap).unwrap();
    assert_eq!(vm.kind, "question");
    assert_eq!(vm.icon, "\u{2753}"); // ❓
    assert!(!vm.is_permission);
    assert!(!vm.is_plan);
    assert!(!vm.is_edit);
    assert!(vm.is_question);
    assert!(vm.diff_rows.is_empty());
}

// ---------------------------------------------------------------------------
// rescope — restrict to workspace session_ids
// ---------------------------------------------------------------------------

#[test]
fn rescope_restricts_sessions_to_workspace_ids() {
    let mut snap = representative_snapshot();
    snap.workspaces = vec![Workspace {
        id: "ws1".to_string(),
        name: "Backend".to_string(),
        session_ids: vec!["s1".to_string(), "s3".to_string()],
        saved_at: String::new(),
    }];
    let scoped = rescope("ws1", &snap);
    assert_eq!(scoped.sessions.len(), 2);
    let ids: Vec<&str> = scoped.sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"s1"));
    assert!(ids.contains(&"s3"));
}

#[test]
fn rescope_counts_reflect_restricted_sessions() {
    let mut snap = representative_snapshot();
    snap.workspaces = vec![Workspace {
        id: "ws1".to_string(),
        name: "Backend".to_string(),
        session_ids: vec!["s1".to_string()], // only s1 (needs)
        saved_at: String::new(),
    }];
    let scoped = rescope("ws1", &snap);
    let c = counts(&scoped);
    assert_eq!(c.total, 1);
    assert_eq!(c.needs, 1);
    assert_eq!(c.errors, 0);
}

// ---------------------------------------------------------------------------
// parse_slash — nominal variants
// ---------------------------------------------------------------------------

#[test]
fn parse_slash_clear() {
    assert_eq!(parse_slash("/clear"), SlashCmd::Clear);
}

#[test]
fn parse_slash_compact() {
    assert_eq!(parse_slash("/compact"), SlashCmd::Compact);
}

#[test]
fn parse_slash_cost() {
    assert_eq!(parse_slash("/cost"), SlashCmd::Cost);
}

#[test]
fn parse_slash_model() {
    assert_eq!(parse_slash("/model"), SlashCmd::Model);
}

#[test]
fn parse_slash_config() {
    assert_eq!(parse_slash("/config"), SlashCmd::Config);
}

#[test]
fn parse_slash_init() {
    assert_eq!(parse_slash("/init"), SlashCmd::Init);
}

#[test]
fn parse_slash_review() {
    assert_eq!(parse_slash("/review"), SlashCmd::Review);
}

#[test]
fn parse_slash_agents() {
    assert_eq!(parse_slash("/agents"), SlashCmd::Agents);
}

#[test]
fn parse_slash_resume() {
    assert_eq!(parse_slash("/resume"), SlashCmd::Resume);
}

#[test]
fn parse_slash_terminal() {
    assert_eq!(parse_slash("/terminal"), SlashCmd::Terminal);
}

#[test]
fn parse_slash_help() {
    assert_eq!(parse_slash("/help"), SlashCmd::Help);
}

// ---------------------------------------------------------------------------
// slash_catalog — happy paths
// ---------------------------------------------------------------------------

#[test]
fn slash_catalog_empty_query_returns_all_11_commands() {
    let catalog = slash_catalog("");
    assert_eq!(
        catalog.len(),
        11,
        "empty query should return all 11 slash commands"
    );
}

#[test]
fn slash_catalog_top_item_is_first_in_list() {
    // Top item should be the first command in spec order (/clear) for empty query
    let catalog = slash_catalog("");
    assert!(!catalog.is_empty());
    assert_eq!(catalog[0].name, "/clear");
}

#[test]
fn slash_catalog_prefix_match_filters_results() {
    let catalog = slash_catalog("/co");
    // matches: /compact, /cost, /config
    let names: Vec<&str> = catalog.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"/compact"));
    assert!(names.contains(&"/cost"));
    assert!(names.contains(&"/config"));
    assert!(!names.contains(&"/clear"), "/co should not match /clear");
}

#[test]
fn slash_catalog_no_match_returns_empty() {
    let catalog = slash_catalog("/zzz");
    assert!(
        catalog.is_empty(),
        "no-match query should return empty list"
    );
}

// ---------------------------------------------------------------------------
// ModelUsageRow — derives and construction
// ---------------------------------------------------------------------------

/// ModelUsageRow derives Debug, Clone, PartialEq.
#[test]
fn model_usage_row_derives_and_constructs() {
    use marsad_viewmodel::ModelUsageRow;
    let row = ModelUsageRow {
        model: "claude-sonnet-4.6[1m]".to_string(),
        input_tokens: 11,
        output_tokens: 155,
        cost_usd: 0.268,
    };
    let row2 = row.clone();
    assert_eq!(row, row2);
    let _ = format!("{row:?}");
    assert_eq!(row.model, "claude-sonnet-4.6[1m]");
    assert_eq!(row.input_tokens, 11);
    assert_eq!(row.output_tokens, 155);
    assert!((row.cost_usd - 0.268).abs() < 1e-9);
}
