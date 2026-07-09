/// ipc_typed.rs — golden-JSON regression suite for the M2 typed IPC surface.
///
/// Authoritative spec: `docs/specs/typed-ipc.md` §2 ("Golden wire forms
/// (normative, quoted from today's producers)") and §6 ("Test plan"). Every
/// JSON literal below is copied **verbatim** from §2 — these are today's
/// ACTUAL wire bytes captured from the hand-rolled `json!`/mutation call
/// sites this block replaces, not invented shapes. A green run here is the
/// wire-compatibility proof required by §2's "Wire compatibility is REQUIRED
/// both directions" and the backward-compat matrix.
///
/// This file WILL fail to compile until the code-agent lands the 19 new
/// `IpcMsg` variants / the retyped `LaunchSession` struct and the
/// `IpcMsg::launch_mode()` helper from §2-§3. That is the expected
/// tests-first signal for M2.
///
/// Per §2: "JSON key order is NOT normative... Golden tests therefore
/// compare parsed values, never raw bytes (except the encode→parse→re-encode
/// stability check, which compares the crate's own output to itself)." Every
/// comparison below follows that rule.
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, LaunchMode, Posture, ProtoError};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse a golden literal to a `Value` for order-insensitive comparison.
fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

/// Inject an unknown field into a golden JSON object literal, mimicking a
/// newer sender adding an additive field this build doesn't know about yet
/// (§6 item 4a: "golden + injected `x_future_field:123` still parses to the
/// same typed value").
fn inject_future_field(golden: &str) -> String {
    let mut v = val(golden);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    v.to_string()
}

/// The full per-variant contract (§6 items 1-4) for one (golden JSON,
/// expected typed value) pair, for every variant whose wire shape is
/// symmetric (re-encoding the constructed value reproduces the golden
/// exactly — true for every tag except `LaunchSession`'s minimal/continue
/// forms, which are PARSE-only compat goldens; see the dedicated
/// `launch_session_minimal_*` / `launch_session_continue_*` tests below):
///
/// 1. golden parse — `parse_ipc(golden)` equals `expected` (full field eq).
/// 2. encode→parse identity — `encode_ipc(&expected)` reparses to `expected`.
/// 3. re-encode stability — re-encoding the reparsed value is byte-identical
///    to the first encode.
/// 4. golden encode — `encode_ipc(&expected)` and `golden` are equal as
///    `Value` (key order non-normative, §2).
/// 5. cross-version tolerance — golden + an injected unknown field still
///    parses to `expected`.
fn assert_variant_contract(golden: &str, expected: &IpcMsg) {
    let parsed = parse_ipc(golden).unwrap_or_else(|e| panic!("golden must parse: {e:?}\n{golden}"));
    assert_eq!(&parsed, expected, "golden parse mismatch for {golden}");

    let encoded = encode_ipc(expected);
    let reparsed = parse_ipc(&encoded).expect("self-encoded IpcMsg must reparse");
    assert_eq!(
        &reparsed, expected,
        "encode->parse identity failed for {golden}"
    );
    assert_eq!(
        encode_ipc(&reparsed),
        encoded,
        "re-encode must be stable for {golden}"
    );

    assert_eq!(
        val(&encoded),
        val(golden),
        "golden encode Value mismatch for {golden}"
    );

    let tolerant = inject_future_field(golden);
    let parsed_tolerant = parse_ipc(&tolerant)
        .unwrap_or_else(|e| panic!("unknown-field-tolerant parse failed: {e:?}"));
    assert_eq!(
        &parsed_tolerant, expected,
        "unknown-field injection must not change the parsed value for {golden}"
    );
}

// ---------------------------------------------------------------------------
// LaunchSession — full retyped struct (§2, §3, §6 item 5)
// ---------------------------------------------------------------------------

const LAUNCH_SESSION_MAXIMAL: &str = r#"{"type":"LaunchSession","session_id":"s-1","project":"/home/u/proj","cwd":"/home/u/proj","host_id":"hpc","host_target":"user@hpc.example.com","claude_bin":"genie","setup":"module load cuda","env":[["FOO","bar"]],"model":"claude-opus-4-8","posture":"Ask","permission_mode":"default","first_prompt":null,"resume_session_id":"uuid-xyz","name":"My run","disallowed_tools":["Bash"]}"#;

const LAUNCH_SESSION_MINIMAL: &str = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;

const LAUNCH_SESSION_CONTINUE: &str = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","continue":true}"#;

fn launch_session_maximal() -> IpcMsg {
    IpcMsg::LaunchSession {
        session_id: Some("s-1".to_string()),
        project: "/home/u/proj".to_string(),
        cwd: "/home/u/proj".to_string(),
        host_id: Some("hpc".to_string()),
        host_target: Some("user@hpc.example.com".to_string()),
        claude_bin: Some("genie".to_string()),
        setup: Some("module load cuda".to_string()),
        env: vec![("FOO".to_string(), "bar".to_string())],
        model: "claude-opus-4-8".to_string(),
        posture: Posture::Ask,
        permission_mode: "default".to_string(),
        first_prompt: None,
        continue_: None,
        resume_session_id: Some("uuid-xyz".to_string()),
        name: Some("My run".to_string()),
        disallowed_tools: vec!["Bash".to_string()],
        wasl: false,
        driver: "claude".to_string(),
    }
}

fn launch_session_minimal() -> IpcMsg {
    IpcMsg::LaunchSession {
        session_id: None,
        project: "/home/user/proj".to_string(),
        cwd: String::new(),
        host_id: Some("local".to_string()),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        permission_mode: String::new(),
        first_prompt: None,
        continue_: None,
        resume_session_id: None,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

fn launch_session_continue() -> IpcMsg {
    IpcMsg::LaunchSession {
        session_id: None,
        project: "/home/user/proj".to_string(),
        cwd: String::new(),
        host_id: Some("local".to_string()),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "claude-opus-4-8".to_string(),
        posture: Posture::Ask,
        permission_mode: String::new(),
        first_prompt: None,
        continue_: Some(true),
        resume_session_id: None,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

/// The maximal golden is exactly what core produces today (§3's
/// `launch_session_with_id` construction) — full round-trip contract applies.
#[test]
fn launch_session_maximal_golden_contract() {
    assert_variant_contract(LAUNCH_SESSION_MAXIMAL, &launch_session_maximal());
}

/// Lines 2-3 of §2 are "the agent-test minimal launch corpus — they MUST
/// keep parsing" (§2, note after the golden list): a PARSE-only golden (old
/// / raw clients omit these keys). It is NOT what core's encoder reproduces
/// byte-for-byte (`cwd`/`permission_mode` have no `skip_serializing_if`, so
/// they are always emitted, even as `""`) — so this is asserted directly by
/// parse equality rather than through `assert_variant_contract`'s
/// golden-encode-Value-eq check.
#[test]
fn launch_session_minimal_golden_parses_to_defaults() {
    let parsed = parse_ipc(LAUNCH_SESSION_MINIMAL).expect("minimal LaunchSession must parse");
    assert_eq!(parsed, launch_session_minimal());
}

#[test]
fn launch_session_minimal_tolerates_injected_unknown_field() {
    let tolerant = inject_future_field(LAUNCH_SESSION_MINIMAL);
    let parsed = parse_ipc(&tolerant).expect("minimal + unknown field must still parse");
    assert_eq!(parsed, launch_session_minimal());
}

#[test]
fn launch_session_minimal_encode_parse_roundtrips_and_is_stable() {
    let msg = launch_session_minimal();
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("encoded minimal LaunchSession must reparse");
    assert_eq!(reparsed, msg);
    assert_eq!(encode_ipc(&reparsed), encoded);
}

#[test]
fn launch_session_continue_golden_parses() {
    let parsed = parse_ipc(LAUNCH_SESSION_CONTINUE).expect("continue LaunchSession must parse");
    assert_eq!(parsed, launch_session_continue());
}

#[test]
fn launch_session_continue_tolerates_injected_unknown_field() {
    let tolerant = inject_future_field(LAUNCH_SESSION_CONTINUE);
    let parsed = parse_ipc(&tolerant).expect("continue + unknown field must still parse");
    assert_eq!(parsed, launch_session_continue());
}

#[test]
fn launch_session_continue_encode_parse_roundtrips_and_is_stable() {
    let msg = launch_session_continue();
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("encoded continue LaunchSession must reparse");
    assert_eq!(reparsed, msg);
    assert_eq!(encode_ipc(&reparsed), encoded);
}

/// §6 item 5's `skip_serializing_if` proof: encode the minimal value and
/// assert the JSON object has none of the `skip_serializing_if`-guarded keys
/// (never set) but DOES carry `first_prompt: null` (§2: "ALWAYS serialized,
/// `null` when absent — today core unconditionally inserts it").
#[test]
fn launch_session_minimal_reencode_omits_absent_optionals_but_keeps_first_prompt_null() {
    let encoded = encode_ipc(&launch_session_minimal());
    let v = val(&encoded);
    let obj = v
        .as_object()
        .expect("encoded LaunchSession must be an object");
    for absent_key in [
        "session_id",
        "host_target",
        "claude_bin",
        "setup",
        "env",
        "continue",
        "resume_session_id",
        "name",
        "disallowed_tools",
    ] {
        assert!(
            !obj.contains_key(absent_key),
            "key `{absent_key}` must be absent on re-encode of a minimal LaunchSession, got: {encoded}"
        );
    }
    assert_eq!(
        obj.get("first_prompt"),
        Some(&serde_json::Value::Null),
        "first_prompt must ALWAYS be serialized (null when absent), got: {encoded}"
    );
}

// ---------------------------------------------------------------------------
// IpcMsg::launch_mode() — precedence table (§6 item 5; §2's doc comment:
// "non-empty resume_session_id wins, then continue == true, else Fresh")
// ---------------------------------------------------------------------------

fn launch_with(resume: Option<&str>, cont: Option<bool>) -> IpcMsg {
    IpcMsg::LaunchSession {
        session_id: None,
        project: "/p".to_string(),
        cwd: String::new(),
        host_id: Some("local".to_string()),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "m".to_string(),
        posture: Posture::Ask,
        permission_mode: String::new(),
        first_prompt: None,
        continue_: cont,
        resume_session_id: resume.map(|s| s.to_string()),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

#[test]
fn launch_mode_resume_nonempty_wins() {
    let msg = launch_with(Some("uuid-1"), None);
    assert_eq!(
        msg.launch_mode(),
        Some(LaunchMode::Resume("uuid-1".to_string()))
    );
}

#[test]
fn launch_mode_empty_resume_plus_continue_true_is_continue() {
    let msg = launch_with(Some(""), Some(true));
    assert_eq!(msg.launch_mode(), Some(LaunchMode::Continue));
}

#[test]
fn launch_mode_empty_resume_alone_is_fresh() {
    let msg = launch_with(Some(""), None);
    assert_eq!(msg.launch_mode(), Some(LaunchMode::Fresh));
}

#[test]
fn launch_mode_neither_set_is_fresh() {
    let msg = launch_with(None, None);
    assert_eq!(msg.launch_mode(), Some(LaunchMode::Fresh));
}

#[test]
fn launch_mode_both_resume_and_continue_set_resume_wins() {
    let msg = launch_with(Some("uuid-2"), Some(true));
    assert_eq!(
        msg.launch_mode(),
        Some(LaunchMode::Resume("uuid-2".to_string()))
    );
}

/// `launch_mode()` is documented "LaunchSession only" (§2) — for every other
/// variant there is no launch mode to derive.
#[test]
fn launch_mode_is_none_for_non_launch_session_variants() {
    let msg = IpcMsg::ResumeSession {
        session_id: "s-1".to_string(),
        first_prompt: None,
    };
    assert_eq!(msg.launch_mode(), None);
}

// ---------------------------------------------------------------------------
// ResumeSession — additive `first_prompt` field (§2)
// ---------------------------------------------------------------------------

#[test]
fn resume_session_golden_contract() {
    assert_variant_contract(
        r#"{"type":"ResumeSession","session_id":"s-1"}"#,
        &IpcMsg::ResumeSession {
            session_id: "s-1".to_string(),
            first_prompt: None,
        },
    );
}

/// §2: `first_prompt` is ADDITIVE — the agent already reads it from the raw
/// value; core never sends it today. Not part of the pinned golden (which is
/// the shape core actually emits), but it must still round-trip cleanly and
/// be emitted (not silently dropped) when present.
#[test]
fn resume_session_with_first_prompt_additive_field_roundtrips() {
    let msg = IpcMsg::ResumeSession {
        session_id: "s-1".to_string(),
        first_prompt: Some("continue where we left off".to_string()),
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("roundtrip");
    assert_eq!(reparsed, msg);
    assert_eq!(encode_ipc(&reparsed), encoded);
    assert!(
        encoded.contains("first_prompt"),
        "Some(..) must serialize the key, got: {encoded}"
    );
}

// ---------------------------------------------------------------------------
// The remaining 25 simple variants — one golden contract each (§6 item 1-4)
// ---------------------------------------------------------------------------

#[test]
fn attach_session_golden_contract() {
    assert_variant_contract(
        r#"{"type":"AttachSession","session_id":"s-1"}"#,
        &IpcMsg::AttachSession {
            session_id: "s-1".to_string(),
        },
    );
}

#[test]
fn stop_session_golden_contract() {
    assert_variant_contract(
        r#"{"type":"StopSession","session_id":"s-1"}"#,
        &IpcMsg::StopSession {
            session_id: "s-1".to_string(),
        },
    );
}

#[test]
fn send_prompt_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SendPrompt","session_id":"s-1","text":"do the thing"}"#,
        &IpcMsg::SendPrompt {
            session_id: "s-1".to_string(),
            text: "do the thing".to_string(),
        },
    );
}

/// NEW variant (§2/§3): typed form of core:1264 / agent:992's raw `Interrupt` line.
#[test]
fn interrupt_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Interrupt","session_id":"s-1"}"#,
        &IpcMsg::Interrupt {
            session_id: "s-1".to_string(),
        },
    );
}

#[test]
fn answer_gate_golden_contract() {
    assert_variant_contract(
        r#"{"type":"AnswerGate","session_id":"s-1","request_id":"req-1","allow":true,"deny_reason":null}"#,
        &IpcMsg::AnswerGate {
            session_id: "s-1".to_string(),
            request_id: "req-1".to_string(),
            allow: true,
            deny_reason: None,
            elicitation_content: None,        },
    );
}

#[test]
fn answer_question_golden_contract() {
    assert_variant_contract(
        r#"{"type":"AnswerQuestion","session_id":"s-1","request_id":"req-q1","answers":{"Which approach?":"Option A"}}"#,
        &IpcMsg::AnswerQuestion {
            session_id: "s-1".to_string(),
            request_id: "req-q1".to_string(),
            answers: serde_json::json!({"Which approach?": "Option A"}),
        },
    );
}

#[test]
fn subscribe_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Subscribe","session_id":""}"#,
        &IpcMsg::Subscribe {
            session_id: String::new(),
        },
    );
}

/// NEW variant (§2/§3): typed form of the stringly arm agent:784, a no-op today.
#[test]
fn unsubscribe_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Unsubscribe","subscriber_id":"sub-1"}"#,
        &IpcMsg::Unsubscribe {
            subscriber_id: "sub-1".to_string(),
        },
    );
}

/// NEW variant (§2/§3): typed form of core:1315-1321's raw `SlashCmd` line.
#[test]
fn slash_cmd_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SlashCmd","session_id":"s-1","cmd":"compact"}"#,
        &IpcMsg::SlashCmd {
            session_id: "s-1".to_string(),
            cmd: "compact".to_string(),
        },
    );
}

/// NEW variant (§2/§3): typed form of core:1331-1337's raw `SetWorkspace` line.
#[test]
fn set_workspace_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SetWorkspace","workspace_id":"ws-1"}"#,
        &IpcMsg::SetWorkspace {
            workspace_id: "ws-1".to_string(),
        },
    );
}

/// Vestigial (§1/§2): in `IpcMsg` and proto tests; never sent by live code.
/// Kept unchanged for wire compat.
#[test]
fn vm_update_golden_contract() {
    assert_variant_contract(
        r#"{"type":"VmUpdate","session_id":"abc-123","update":{}}"#,
        &IpcMsg::VmUpdate {
            session_id: "abc-123".to_string(),
            update: serde_json::json!({}),
        },
    );
}

#[test]
fn agent_event_golden_contract() {
    assert_variant_contract(
        r#"{"type":"AgentEvent","session_id":"s-1","event":{"type":"result","subtype":"success"}}"#,
        &IpcMsg::AgentEvent {
            session_id: "s-1".to_string(),
            event: serde_json::json!({"type":"result","subtype":"success"}),
        },
    );
}

/// NEW variant (§2/§3): typed form of agent:2835's raw `TranscriptBackfill` line.
#[test]
fn transcript_backfill_golden_contract() {
    assert_variant_contract(
        r#"{"type":"TranscriptBackfill","session_id":"s-1","jsonl":"{\"type\":\"user\"}\n{\"type\":\"assistant\"}"}"#,
        &IpcMsg::TranscriptBackfill {
            session_id: "s-1".to_string(),
            jsonl: "{\"type\":\"user\"}\n{\"type\":\"assistant\"}".to_string(),
        },
    );
}

/// NEW variant (§2/§3): typed form of agent:3009's raw `ChangesUpdate` line.
/// `changes` is kept as `Value` ON PURPOSE (§2: proto must not depend on
/// `marsad-viewmodel`'s `FileChange`) — asserted as an opaque `Value` here.
#[test]
fn changes_update_golden_contract() {
    assert_variant_contract(
        r#"{"type":"ChangesUpdate","session_id":"s-1","changes":[{"path":"a.rs","status":"M","added":3,"removed":1}]}"#,
        &IpcMsg::ChangesUpdate {
            session_id: "s-1".to_string(),
            changes: serde_json::json!([{"path":"a.rs","status":"M","added":3,"removed":1}]),
        },
    );
}

/// NEW variant (§2/§3): typed form of agent:2874/3037's raw `UsageUpdate` line.
#[test]
fn usage_update_golden_contract() {
    assert_variant_contract(
        r#"{"type":"UsageUpdate","session_id":"s-1","usage":{"claude-opus-4-8":{"input":100,"output":50,"cache_read":0,"cache_creation":0}}}"#,
        &IpcMsg::UsageUpdate {
            session_id: "s-1".to_string(),
            usage: serde_json::json!({"claude-opus-4-8":{"input":100,"output":50,"cache_read":0,"cache_creation":0}}),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:818's raw `SessionLaunched` ack.
#[test]
fn session_launched_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SessionLaunched","session_id":"s-1"}"#,
        &IpcMsg::SessionLaunched {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:891's raw `SessionResumed` ack.
#[test]
fn session_resumed_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SessionResumed","session_id":"s-1"}"#,
        &IpcMsg::SessionResumed {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:907's raw `SessionAttached` ack.
#[test]
fn session_attached_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SessionAttached","session_id":"s-1"}"#,
        &IpcMsg::SessionAttached {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:930's raw `SessionStopped` ack.
#[test]
fn session_stopped_golden_contract() {
    assert_variant_contract(
        r#"{"type":"SessionStopped","session_id":"s-1"}"#,
        &IpcMsg::SessionStopped {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:988's raw `PromptSent` ack.
#[test]
fn prompt_sent_golden_contract() {
    assert_variant_contract(
        r#"{"type":"PromptSent","session_id":"s-1"}"#,
        &IpcMsg::PromptSent {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:1019's raw `Interrupted` ack.
#[test]
fn interrupted_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Interrupted","session_id":"s-1"}"#,
        &IpcMsg::Interrupted {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:1076's raw `GateAnswered` ack.
#[test]
fn gate_answered_golden_contract() {
    assert_variant_contract(
        r#"{"type":"GateAnswered","session_id":"s-1"}"#,
        &IpcMsg::GateAnswered {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:1118's raw `QuestionAnswered` ack.
#[test]
fn question_answered_golden_contract() {
    assert_variant_contract(
        r#"{"type":"QuestionAnswered","session_id":"s-1"}"#,
        &IpcMsg::QuestionAnswered {
            session_id: "s-1".to_string(),
        },
    );
}

/// NEW ack variant (§2/§3): typed form of agent:1125's raw `Subscribed` ack.
#[test]
fn subscribed_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Subscribed","subscriber_id":"sub-1"}"#,
        &IpcMsg::Subscribed {
            subscriber_id: "sub-1".to_string(),
        },
    );
}

/// NEW variant (§2/§3): typed form of the serve loop's raw `Error` reply (agent:1614).
#[test]
fn error_golden_contract() {
    assert_variant_contract(
        r#"{"type":"Error","message":"unknown IPC type: Bogus"}"#,
        &IpcMsg::Error {
            message: "unknown IPC type: Bogus".to_string(),
        },
    );
}

// ---------------------------------------------------------------------------
// Table-driven sweep over ALL 29 §2 golden lines (§6 items 1 and 4a:
// "for EVERY variant" / "every variant, loop-driven")
// ---------------------------------------------------------------------------

/// All 29 golden wire lines from §2, paired with their expected typed value.
/// `LaunchSession` contributes 3 goldens (maximal/minimal/continue); every
/// other tag contributes exactly one — 26 single-golden tags + 3
/// `LaunchSession` forms = 29, matching §1/§6's "all 29 tags" count.
fn all_goldens() -> Vec<(&'static str, &'static str, IpcMsg)> {
    vec![
        (
            "LaunchSession (maximal)",
            LAUNCH_SESSION_MAXIMAL,
            launch_session_maximal(),
        ),
        (
            "LaunchSession (minimal)",
            LAUNCH_SESSION_MINIMAL,
            launch_session_minimal(),
        ),
        (
            "LaunchSession (continue)",
            LAUNCH_SESSION_CONTINUE,
            launch_session_continue(),
        ),
        (
            "ResumeSession",
            r#"{"type":"ResumeSession","session_id":"s-1"}"#,
            IpcMsg::ResumeSession {
                session_id: "s-1".to_string(),
                first_prompt: None,
            },
        ),
        (
            "AttachSession",
            r#"{"type":"AttachSession","session_id":"s-1"}"#,
            IpcMsg::AttachSession {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "StopSession",
            r#"{"type":"StopSession","session_id":"s-1"}"#,
            IpcMsg::StopSession {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "SendPrompt",
            r#"{"type":"SendPrompt","session_id":"s-1","text":"do the thing"}"#,
            IpcMsg::SendPrompt {
                session_id: "s-1".to_string(),
                text: "do the thing".to_string(),
            },
        ),
        (
            "Interrupt",
            r#"{"type":"Interrupt","session_id":"s-1"}"#,
            IpcMsg::Interrupt {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "AnswerGate",
            r#"{"type":"AnswerGate","session_id":"s-1","request_id":"req-1","allow":true,"deny_reason":null}"#,
            IpcMsg::AnswerGate {
                session_id: "s-1".to_string(),
                request_id: "req-1".to_string(),
                allow: true,
                deny_reason: None,
                elicitation_content: None,            },
        ),
        (
            "AnswerQuestion",
            r#"{"type":"AnswerQuestion","session_id":"s-1","request_id":"req-q1","answers":{"Which approach?":"Option A"}}"#,
            IpcMsg::AnswerQuestion {
                session_id: "s-1".to_string(),
                request_id: "req-q1".to_string(),
                answers: serde_json::json!({"Which approach?": "Option A"}),
            },
        ),
        (
            "Subscribe",
            r#"{"type":"Subscribe","session_id":""}"#,
            IpcMsg::Subscribe {
                session_id: String::new(),
            },
        ),
        (
            "Unsubscribe",
            r#"{"type":"Unsubscribe","subscriber_id":"sub-1"}"#,
            IpcMsg::Unsubscribe {
                subscriber_id: "sub-1".to_string(),
            },
        ),
        (
            "SlashCmd",
            r#"{"type":"SlashCmd","session_id":"s-1","cmd":"compact"}"#,
            IpcMsg::SlashCmd {
                session_id: "s-1".to_string(),
                cmd: "compact".to_string(),
            },
        ),
        (
            "SetWorkspace",
            r#"{"type":"SetWorkspace","workspace_id":"ws-1"}"#,
            IpcMsg::SetWorkspace {
                workspace_id: "ws-1".to_string(),
            },
        ),
        (
            "AgentEvent",
            r#"{"type":"AgentEvent","session_id":"s-1","event":{"type":"result","subtype":"success"}}"#,
            IpcMsg::AgentEvent {
                session_id: "s-1".to_string(),
                event: serde_json::json!({"type":"result","subtype":"success"}),
            },
        ),
        (
            "TranscriptBackfill",
            r#"{"type":"TranscriptBackfill","session_id":"s-1","jsonl":"{\"type\":\"user\"}\n{\"type\":\"assistant\"}"}"#,
            IpcMsg::TranscriptBackfill {
                session_id: "s-1".to_string(),
                jsonl: "{\"type\":\"user\"}\n{\"type\":\"assistant\"}".to_string(),
            },
        ),
        (
            "ChangesUpdate",
            r#"{"type":"ChangesUpdate","session_id":"s-1","changes":[{"path":"a.rs","status":"M","added":3,"removed":1}]}"#,
            IpcMsg::ChangesUpdate {
                session_id: "s-1".to_string(),
                changes: serde_json::json!([{"path":"a.rs","status":"M","added":3,"removed":1}]),
            },
        ),
        (
            "UsageUpdate",
            r#"{"type":"UsageUpdate","session_id":"s-1","usage":{"claude-opus-4-8":{"input":100,"output":50,"cache_read":0,"cache_creation":0}}}"#,
            IpcMsg::UsageUpdate {
                session_id: "s-1".to_string(),
                usage: serde_json::json!({"claude-opus-4-8":{"input":100,"output":50,"cache_read":0,"cache_creation":0}}),
            },
        ),
        (
            "SessionLaunched",
            r#"{"type":"SessionLaunched","session_id":"s-1"}"#,
            IpcMsg::SessionLaunched {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "SessionResumed",
            r#"{"type":"SessionResumed","session_id":"s-1"}"#,
            IpcMsg::SessionResumed {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "SessionAttached",
            r#"{"type":"SessionAttached","session_id":"s-1"}"#,
            IpcMsg::SessionAttached {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "SessionStopped",
            r#"{"type":"SessionStopped","session_id":"s-1"}"#,
            IpcMsg::SessionStopped {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "PromptSent",
            r#"{"type":"PromptSent","session_id":"s-1"}"#,
            IpcMsg::PromptSent {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "Interrupted",
            r#"{"type":"Interrupted","session_id":"s-1"}"#,
            IpcMsg::Interrupted {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "GateAnswered",
            r#"{"type":"GateAnswered","session_id":"s-1"}"#,
            IpcMsg::GateAnswered {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "QuestionAnswered",
            r#"{"type":"QuestionAnswered","session_id":"s-1"}"#,
            IpcMsg::QuestionAnswered {
                session_id: "s-1".to_string(),
            },
        ),
        (
            "Subscribed",
            r#"{"type":"Subscribed","subscriber_id":"sub-1"}"#,
            IpcMsg::Subscribed {
                subscriber_id: "sub-1".to_string(),
            },
        ),
        (
            "Error",
            r#"{"type":"Error","message":"unknown IPC type: Bogus"}"#,
            IpcMsg::Error {
                message: "unknown IPC type: Bogus".to_string(),
            },
        ),
        (
            "VmUpdate",
            r#"{"type":"VmUpdate","session_id":"abc-123","update":{}}"#,
            IpcMsg::VmUpdate {
                session_id: "abc-123".to_string(),
                update: serde_json::json!({}),
            },
        ),
    ]
}

#[test]
fn all_goldens_table_covers_all_29_lines() {
    // Sanity check on the table itself (§1/§6: "all 29 tags listed in §2 —
    // existing 10 + 19 new").
    assert_eq!(
        all_goldens().len(),
        29,
        "§2 lists exactly 29 golden wire lines"
    );
}

#[test]
fn every_golden_parses_to_its_expected_typed_value() {
    // Loop-driven companion to the per-variant `*_golden_contract` tests
    // above: a single sweep confirming EVERY one of the 29 §2 golden lines
    // parses to its documented typed value (§6 item 1, "for EVERY variant").
    for (name, golden, expected) in &all_goldens() {
        let parsed =
            parse_ipc(golden).unwrap_or_else(|e| panic!("{name}: golden must parse: {e:?}"));
        assert_eq!(&parsed, expected, "{name}: golden parse mismatch");
    }
}

#[test]
fn every_golden_tolerates_an_injected_unknown_field() {
    // §6 item 4a: "golden + injected `x_future_field:123` still parses to
    // the same typed value — every variant, loop-driven."
    for (name, golden, expected) in &all_goldens() {
        let tolerant = inject_future_field(golden);
        let parsed = parse_ipc(&tolerant)
            .unwrap_or_else(|e| panic!("{name}: unknown-field-tolerant parse failed: {e:?}"));
        assert_eq!(
            &parsed, expected,
            "{name}: injected unknown field must not change the parsed value"
        );
    }
}

// ---------------------------------------------------------------------------
// Unknown-type → UnknownDiscriminant (§4, §6 item 4b)
// ---------------------------------------------------------------------------

/// A type string that is NOT one of the 27 known tags (§2) must still be a
/// clean `UnknownDiscriminant`, carrying the exact type string — the
/// closed-protocol contract §4 depends on for core's tolerant-skip
/// disposition of a genuinely future inbound type.
#[test]
fn unknown_ipc_type_is_unknown_discriminant_with_exact_type_string() {
    let line = r#"{"type":"IpcTypedProbeNotARealVariant","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("unrecognized IPC type must error");
    match err {
        ProtoError::UnknownDiscriminant(s) => assert_eq!(s, "IpcTypedProbeNotARealVariant"),
        other => panic!("expected UnknownDiscriminant, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Schema errors (§6 item 6)
// ---------------------------------------------------------------------------

#[test]
fn launch_session_posture_as_number_is_schema_mismatch() {
    let line = r#"{"type":"LaunchSession","project":"/p","model":"m","posture":99}"#;
    let err = parse_ipc(line).expect_err("posture:99 must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn launch_session_posture_unknown_string_is_schema_mismatch() {
    let line = r#"{"type":"LaunchSession","project":"/p","model":"m","posture":"Nonsense"}"#;
    let err = parse_ipc(line).expect_err("posture:\"Nonsense\" must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn launch_session_missing_project_is_schema_mismatch() {
    // §2: "Required on parse: `project`, `model`, `posture`".
    let line = r#"{"type":"LaunchSession","model":"m","posture":"Ask"}"#;
    let err = parse_ipc(line).expect_err("missing project must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn launch_session_missing_model_is_schema_mismatch() {
    let line = r#"{"type":"LaunchSession","project":"/p","posture":"Ask"}"#;
    let err = parse_ipc(line).expect_err("missing model must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// §2 AnswerGate doc: "PARSE-TOLERANCE (additive): `#[serde(default)]`
/// preserves the agent's current lenient read
/// `value["allow"].as_bool().unwrap_or(false)` — a missing `allow` means
/// deny, not a schema error."
#[test]
fn answer_gate_missing_allow_parses_tolerant_false() {
    let line = r#"{"type":"AnswerGate","session_id":"s-1","request_id":"req-1"}"#;
    let msg = parse_ipc(line).expect("AnswerGate without `allow` must still parse");
    match msg {
        IpcMsg::AnswerGate {
            allow, deny_reason, ..
        } => {
            assert!(!allow, "missing allow must default to false (deny)");
            assert_eq!(deny_reason, None);
        }
        other => panic!("expected AnswerGate, got {other:?}"),
    }
}
