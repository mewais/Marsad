//! wasl_launch_attach.rs — X4 ("launch-time Wasl attach") additive proto
//! surface: `LaunchSpec.wasl` and `IpcMsg::LaunchSession`'s wire-level
//! `wasl` flag.
//!
//! Authoritative spec: NONE FOUND. `docs/specs/wasl-connector.md` defines no
//! "X4 — launch-time Wasl attach" section; its only "X4" references (lines
//! 549, 651-660, 808, 829-833) name a *different*, already-planned feature —
//! the GUI Wasl-approval modal/graph/observation-feed that consumes the
//! `VmUpdate::Wasl*` variants defined in that same doc's §5.2, reusing
//! `adopt_or_spawn_agent` UNCHANGED (i.e. explicitly *no* new process-spawn
//! plumbing). There is also no `PLAN.md` anywhere in this repo to fall back
//! on. This file therefore encodes the API surface described directly in the
//! X4 task brief (LaunchSpec.wasl bool; IpcMsg::LaunchSession carrying the
//! same flag on the wire) as its own working assumption — see the sibling
//! `crates/marsad-agent/tests/wasl_launch_attach.rs` header for the rest of
//! the assumed surface (claude_argv's `--mcp-config` injection and the
//! adopt-or-spawn helper) and the test-agent's report for the full list of
//! spec/PLAN.md gaps this surfaces.
//!
//! ASSUMED API (does not exist yet — expected to fail to compile until the
//! code-agent lands it):
//!   - `LaunchSpec` gains `pub wasl: bool` with `#[serde(default)]` (absent
//!     on the wire => false; every pre-X4 producer keeps working unchanged).
//!   - `IpcMsg::LaunchSession` gains the same `wasl: bool` field (also
//!     `#[serde(default)]` on parse), so a GUI/core sender can request the
//!     agent inject a Wasl MCP-server entry at launch time.
//!
//! This file WILL fail to compile until that field exists. That is the
//! expected tests-first signal for X4a (see crate map, ../AGENTS.md).

use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, LaunchMode, LaunchSpec, Posture};

// ---------------------------------------------------------------------------
// Shared helpers (duplicated per this crate's existing per-file convention —
// see tests/wasl_ipc.rs's header note: each file in tests/ is its own crate).
// ---------------------------------------------------------------------------

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

/// A fully-populated `LaunchSpec` (every field explicit — required because
/// Rust struct-literal construction, unlike `#[serde(default)]` parsing,
/// offers no "leave the rest at their default" shorthand for a plain struct
/// without a `Default` impl).
fn full_launch_spec(wasl: bool) -> LaunchSpec {
    LaunchSpec {
        project: "/home/user/proj".to_string(),
        host_id: "local".to_string(),
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        name: None,
        disallowed_tools: vec![],
        wasl,
    }
}

// ---------------------------------------------------------------------------
// LaunchSpec.wasl — default false, backward compat, serde round-trip
// ---------------------------------------------------------------------------

/// A pre-X4 wire line: only the fields `LaunchSpec` has ALWAYS required at
/// parse time (no `#[serde(default)]`) are present — `project`, `host_id`,
/// `model`, `posture`, `first_prompt`. No `wasl` key at all. Every existing
/// producer (old core builds, hand-written fixtures) emits exactly this
/// shape today.
const LAUNCH_SPEC_NO_WASL_KEY: &str = r#"{"project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask","first_prompt":null}"#;

#[test]
fn launch_spec_wasl_absent_from_wire_defaults_to_false() {
    let spec: LaunchSpec = serde_json::from_str(LAUNCH_SPEC_NO_WASL_KEY)
        .expect("a LaunchSpec wire line without a `wasl` key must still deserialize");
    assert!(
        !spec.wasl,
        "wasl must default to false when absent from the wire (backward compat)"
    );
}

#[test]
fn launch_spec_wasl_explicit_true_parses() {
    let json = r#"{"project":"/p","host_id":"local","model":"m","posture":"Ask","first_prompt":null,"wasl":true}"#;
    let spec: LaunchSpec =
        serde_json::from_str(json).expect("LaunchSpec with wasl:true must parse");
    assert!(
        spec.wasl,
        "explicit wasl:true on the wire must parse to true"
    );
}

#[test]
fn launch_spec_wasl_explicit_false_parses() {
    let json = r#"{"project":"/p","host_id":"local","model":"m","posture":"Ask","first_prompt":null,"wasl":false}"#;
    let spec: LaunchSpec =
        serde_json::from_str(json).expect("LaunchSpec with wasl:false must parse");
    assert!(!spec.wasl);
}

#[test]
fn launch_spec_wasl_true_serde_round_trips() {
    let spec = full_launch_spec(true);
    let json = serde_json::to_string(&spec).expect("LaunchSpec must serialize");
    let back: LaunchSpec = serde_json::from_str(&json).expect("LaunchSpec must deserialize");
    assert_eq!(back, spec, "wasl:true must survive a full serde round trip");
    assert!(back.wasl);
}

#[test]
fn launch_spec_wasl_false_serde_round_trips() {
    let spec = full_launch_spec(false);
    let json = serde_json::to_string(&spec).expect("LaunchSpec must serialize");
    let back: LaunchSpec = serde_json::from_str(&json).expect("LaunchSpec must deserialize");
    assert_eq!(back, spec);
    assert!(!back.wasl);
}

// ---------------------------------------------------------------------------
// IpcMsg::LaunchSession.wasl — backward compat against the FROZEN
// tests/ipc_typed.rs (M2) goldens, plus a wasl:true wire line.
// ---------------------------------------------------------------------------

/// Verbatim copy of `tests/ipc_typed.rs`'s `LAUNCH_SESSION_MAXIMAL` const
/// (`docs/specs/typed-ipc.md` §2) — duplicated, not imported (each file
/// under tests/ compiles as its own crate; ipc_typed.rs is FROZEN and must
/// not be edited). Re-pinning the exact bytes here proves the X4 `wasl`
/// field addition does not disturb this pre-existing, byte-pinned oracle.
const LAUNCH_SESSION_MAXIMAL: &str = r#"{"type":"LaunchSession","session_id":"s-1","project":"/home/u/proj","cwd":"/home/u/proj","host_id":"hpc","host_target":"user@hpc.example.com","claude_bin":"genie","setup":"module load cuda","env":[["FOO","bar"]],"model":"claude-opus-4-8","posture":"Ask","permission_mode":"default","first_prompt":null,"resume_session_id":"uuid-xyz","name":"My run","disallowed_tools":["Bash"]}"#;

const LAUNCH_SESSION_MINIMAL: &str = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;

const LAUNCH_SESSION_CONTINUE: &str = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","continue":true}"#;

#[test]
fn frozen_launch_session_maximal_golden_still_parses_wasl_defaults_false() {
    let parsed = parse_ipc(LAUNCH_SESSION_MAXIMAL)
        .expect("the frozen ipc_typed.rs maximal golden must still parse once `wasl` is added");
    match parsed {
        IpcMsg::LaunchSession { wasl, project, .. } => {
            assert!(!wasl, "a wire line predating X4 must default wasl to false");
            assert_eq!(
                project, "/home/u/proj",
                "unrelated fields must be unaffected"
            );
        }
        other => panic!("expected LaunchSession, got {other:?}"),
    }
}

#[test]
fn frozen_launch_session_minimal_golden_still_parses_wasl_defaults_false() {
    let parsed = parse_ipc(LAUNCH_SESSION_MINIMAL)
        .expect("the frozen ipc_typed.rs minimal golden must still parse once `wasl` is added");
    match parsed {
        IpcMsg::LaunchSession { wasl, .. } => assert!(!wasl),
        other => panic!("expected LaunchSession, got {other:?}"),
    }
}

#[test]
fn frozen_launch_session_continue_golden_still_parses_wasl_defaults_false() {
    let parsed = parse_ipc(LAUNCH_SESSION_CONTINUE)
        .expect("the frozen ipc_typed.rs continue golden must still parse once `wasl` is added");
    match parsed {
        IpcMsg::LaunchSession { wasl, .. } => assert!(!wasl),
        other => panic!("expected LaunchSession, got {other:?}"),
    }
}

/// The maximal golden plus an injected `"wasl":true` key — the shape a
/// wasl-aware core would send. Must parse, and the flag must survive an
/// encode->parse round trip.
#[test]
fn launch_session_golden_plus_wasl_true_parses_and_round_trips() {
    let mut v = val(LAUNCH_SESSION_MAXIMAL);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("wasl".to_string(), serde_json::json!(true));
    let golden = v.to_string();

    let parsed = parse_ipc(&golden).expect("maximal golden + wasl:true must parse");
    match &parsed {
        IpcMsg::LaunchSession { wasl, .. } => {
            assert!(*wasl, "wasl:true on the wire must parse to true")
        }
        other => panic!("expected LaunchSession, got {other:?}"),
    }

    let encoded = encode_ipc(&parsed);
    let reparsed = parse_ipc(&encoded).expect("re-encoded wasl:true LaunchSession must reparse");
    match reparsed {
        IpcMsg::LaunchSession { wasl, .. } => {
            assert!(wasl, "wasl:true must survive an encode->parse round trip")
        }
        other => panic!("expected LaunchSession, got {other:?}"),
    }
}

/// Re-encoding a `wasl:false` value must never silently flip it to true, and
/// if the field is emitted at all it must be the literal `false` — never
/// omitted-as-if-true. (Whether `false` is omitted via `skip_serializing_if`
/// or always emitted is left to the code-agent; both are acceptable here.)
#[test]
fn launch_session_minimal_reencode_never_sets_wasl_true() {
    let parsed = parse_ipc(LAUNCH_SESSION_MINIMAL).expect("parse minimal golden");
    let encoded = encode_ipc(&parsed);
    let v = val(&encoded);
    if let Some(w) = v.get("wasl") {
        assert_eq!(
            w,
            &serde_json::json!(false),
            "if `wasl` is present on re-encode of a false value it must be literal `false`, got: {encoded}"
        );
    }
}

/// Constructing `IpcMsg::LaunchSession` directly with `wasl: true` (the
/// programmatic path core would use, not raw JSON) must encode and reparse
/// losslessly.
#[test]
fn launch_session_constructed_with_wasl_true_round_trips() {
    let msg = IpcMsg::LaunchSession {
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
        wasl: true,
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("encoded wasl:true LaunchSession must reparse");
    assert_eq!(reparsed, msg);
}
