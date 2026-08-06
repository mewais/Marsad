//! Wave 3 focused tests: handle_agent_event over wasit_core::AgentEvent.
//!
//! Specified requirements:
//!   1. AssistantDelta + ToolCallStarted transcript dedup
//!   2. Compaction appends a Compaction entry
//!   3. SessionInit persists session fields
//!   4. TurnCompleted sets a terminal phase

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, VmUpdate};
use marsad_events::{Phase, TranscriptEntry};
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn make_core() -> Core {
    let (a, _b) = LoopbackFake::pair();
    Core::new(
        CoreConfig {
            agent_endpoint: "loopback://test".to_string(),
            data_root: "/tmp/marsad-wave3-test".to_string(),
        },
        Box::new(a),
    )
    .expect("Core::new")
}

fn launch(core: &mut Core) -> String {
    core.launch_session(LaunchSpec {
        project: "/tmp/w3-proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "claude-sonnet-4-6".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch")
}

fn init_ipc(sid: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"session_init","native_session_id":"{sid}","model":"claude-sonnet-4-6","cwd":"/tmp","tools":[],"mcp_servers":[],"permission_mode":"default"}}}}"#
    )
}

fn assistant_delta_ipc(sid: &str, text: &str, msg_id: &str) -> String {
    let escaped = text.replace('"', "\\\"");
    format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"assistant_delta","text":"{escaped}","message_id":"{msg_id}"}}}}"#
    )
}

fn tool_call_started_ipc(sid: &str, name: &str, call_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"tool_call_started","name":"{name}","call_id":"{call_id}","input":{{"cmd":"ls"}}}}}}"#
    )
}

fn compaction_ipc(sid: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"compaction","info":{{"trigger":"auto","pre_tokens":5000,"post_tokens":1000,"cumulative_dropped_tokens":4000,"duration_ms":200}}}}}}"#
    )
}

fn turn_completed_ipc(sid: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"turn_completed","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.01}}}}}}"#
    )
}

fn last_sv(updates: &[VmUpdate]) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| if let VmUpdate::Session(sv) = u { Some(sv.clone()) } else { None })
        .expect("must have a Session update")
}

// ---------------------------------------------------------------------------
// Test 1: AssistantDelta + ToolCallStarted dedup
// ---------------------------------------------------------------------------

/// Feeding AssistantDelta("hello", msg_id="m1") twice and ToolCallStarted
/// (call_id="c1") twice produces exactly [Assistant("hello"), Tool("Bash")] —
/// dedup by key prevents duplicates, not by message_id alone.
#[test]
fn assistant_delta_and_tool_call_dedup_produces_exact_transcript() {
    let mut core = make_core();
    let sid = launch(&mut core);
    core.on_agent_line(&init_ipc(&sid)).expect("init");

    // First assistant delta — should produce one entry.
    core.on_agent_line(&assistant_delta_ipc(&sid, "hello", "m1"))
        .expect("delta 1");
    // Duplicate — same text + same message_id — must be deduped.
    let u = core
        .on_agent_line(&assistant_delta_ipc(&sid, "hello", "m1"))
        .expect("delta 1 dup");
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 1, "duplicate AssistantDelta must be deduped: {t:?}");
    assert!(
        matches!(&t[0], TranscriptEntry::Assistant { text, .. } if text == "hello"),
        "entry must be Assistant(hello), got: {t:?}"
    );

    // First tool call — produces one entry.
    core.on_agent_line(&tool_call_started_ipc(&sid, "Bash", "c1"))
        .expect("tool 1");
    // Duplicate call_id — must be deduped.
    let u2 = core
        .on_agent_line(&tool_call_started_ipc(&sid, "Bash", "c1"))
        .expect("tool 1 dup");
    let t2 = last_sv(&u2).transcript;
    assert_eq!(
        t2.len(),
        2,
        "duplicate ToolCallStarted must be deduped, expected [Assistant, Tool]: {t2:?}"
    );
    assert!(
        matches!(&t2[1], TranscriptEntry::Tool { name, .. } if name == "Bash"),
        "second entry must be Tool(Bash), got: {t2:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Compaction appends a Compaction entry
// ---------------------------------------------------------------------------

/// After init + one assistant delta, feeding a Compaction event appends a
/// Compaction entry in chronological order (after the Assistant entry).
#[test]
fn compaction_event_appends_compaction_entry_in_sequence() {
    let mut core = make_core();
    let sid = launch(&mut core);
    core.on_agent_line(&init_ipc(&sid)).expect("init");
    core.on_agent_line(&assistant_delta_ipc(&sid, "thinking", "m-c"))
        .expect("assistant before compaction");

    let u = core.on_agent_line(&compaction_ipc(&sid)).expect("compaction");
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 2, "must have [Assistant, Compaction]: {t:?}");
    assert!(
        matches!(&t[0], TranscriptEntry::Assistant { text, .. } if text == "thinking"),
        "first entry must be Assistant, got: {t:?}"
    );
    assert!(
        matches!(
            &t[1],
            TranscriptEntry::Compaction { dropped_tokens, .. } if *dropped_tokens == 4000
        ),
        "second entry must be Compaction with dropped_tokens=4000, got: {t:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: SessionInit persists session fields
// ---------------------------------------------------------------------------

/// A SessionInit event with tools and mcp_servers causes the session state
/// to reflect those fields and does not panic.
#[test]
fn session_init_sets_session_fields_without_panic() {
    let mut core = make_core();
    let sid = launch(&mut core);

    let init_with_fields = format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"session_init","native_session_id":"claude-uuid-w3","model":"claude-sonnet-4-6","cwd":"/tmp/w3","tools":["Read","Bash"],"mcp_servers":[{{"name":"hares","status":"connected"}}],"permission_mode":"default"}}}}"#
    );
    let updates = core
        .on_agent_line(&init_with_fields)
        .expect("SessionInit must not error");

    let sv = last_sv(&updates);
    // This session was launched with NO first prompt (empty transcript), so this
    // SessionInit is a SESSION-ESTABLISHMENT init (opencode's `session/new`
    // shape), not a turn start. An established-but-idle session is `Starting`
    // (ready for input), NOT `Working` — a phantom "working…" before any prompt
    // was the opencode #6 bug. See `session_init_after_prompt_stays_working` for
    // the turn-in-flight case (transcript non-empty ⇒ Working preserved).
    assert_eq!(
        sv.state,
        Phase::Starting,
        "an establishment SessionInit (empty transcript) is idle/ready, not Working"
    );
    // Tools and mcp_servers must be reflected in the view.
    assert_eq!(
        sv.tools,
        vec!["Read".to_string(), "Bash".to_string()],
        "tools must match SessionInit tools"
    );
    assert_eq!(sv.mcp_servers.len(), 1, "mcp_servers must have 1 entry");
    assert_eq!(sv.mcp_servers[0].name, "hares");
    // Model from SessionInit must appear in the view.
    assert_eq!(sv.model, "claude-sonnet-4-6", "model must match SessionInit model");
}

/// A SessionInit that arrives with a turn already in flight (the transcript
/// carries a prompt/output) is a genuine turn-start init (claude/codex
/// `system/init`) and MUST keep phase `Working`. We simulate a turn in flight by
/// feeding an assistant delta (non-empty transcript) before the init — proving
/// the E fix corrects ONLY the empty-transcript establishment case and leaves the
/// claude/codex turn-start contract intact.
#[test]
fn session_init_after_prompt_stays_working() {
    let mut core = make_core();
    let sid = launch(&mut core);
    // Turn in flight: an assistant delta lands first → transcript is non-empty.
    core.on_agent_line(&assistant_delta_ipc(&sid, "on it", "m-w"))
        .expect("delta");
    let updates = core.on_agent_line(&init_ipc(&sid)).expect("init");
    let sv = last_sv(&updates);
    assert_eq!(
        sv.state,
        Phase::Working,
        "a turn-start SessionInit (non-empty transcript) must stay Working"
    );
}

/// A just-launched session must have immediate launch-time VmUpdates (Session +
/// Overview + Board) so the bridge can emit them and the new session appears at
/// once on every subscriber — the fix for a fresh session that emitted no stream
/// event until prompted (claude/codex), leaving it invisible + "loading" forever
/// (#2/#4/#8). The Session view carries the launched id in `Starting` phase.
#[test]
fn session_launch_updates_yields_starting_session_view() {
    let mut core = make_core();
    let sid = launch(&mut core);
    let updates = core.session_launch_updates(&sid);
    let sv = last_sv(&updates);
    assert_eq!(sv.id, sid, "launch updates must carry the launched session id");
    assert_eq!(sv.state, Phase::Starting, "a fresh launch is Starting/ready");
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, VmUpdate::Overview(_))),
        "launch updates must include an Overview so the session shows on the dashboard"
    );
    // An unknown id is a defensive no-op, never a panic.
    assert!(core.session_launch_updates("no-such-id").is_empty());
}

// ---------------------------------------------------------------------------
// Test 4: TurnCompleted sets a terminal phase
// ---------------------------------------------------------------------------

/// After init + one assistant delta, a TurnCompleted event moves the session
/// to Done phase (normal completion).
#[test]
fn turn_completed_sets_done_phase() {
    let mut core = make_core();
    let sid = launch(&mut core);
    core.on_agent_line(&init_ipc(&sid)).expect("init");
    core.on_agent_line(&assistant_delta_ipc(&sid, "result", "m-done"))
        .expect("delta");

    let u = core.on_agent_line(&turn_completed_ipc(&sid)).expect("turn_completed");
    let sv = last_sv(&u);
    assert_eq!(
        sv.state,
        Phase::Done,
        "TurnCompleted must set phase to Done, got: {:?}",
        sv.state
    );
}

/// A TurnCompleted with end_reason execution_error sets Error phase.
#[test]
fn turn_completed_with_execution_error_sets_error_phase() {
    let mut core = make_core();
    let sid = launch(&mut core);
    core.on_agent_line(&init_ipc(&sid)).expect("init");

    let err_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{sid}","event":{{"type":"turn_completed","end_reason":"execution_error","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.01}}}}}}"#
    );
    let u = core.on_agent_line(&err_line).expect("turn_completed error");
    let sv = last_sv(&u);
    assert_eq!(
        sv.state,
        Phase::Error,
        "TurnCompleted(execution_error) must set phase to Error, got: {:?}",
        sv.state
    );
}
