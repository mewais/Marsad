/// fixtures.rs — parse every line of recorded real-world stream captures.
///
/// `qa.ndjson` is fully captured and runs unconditionally.
/// Other scenarios (permission, plan, edit, subagent, compaction) are TODO fixtures;
/// those tests are `#[ignore]`-d with a path-exists gate comment pointing at
/// fixtures/README.md, so the suite is honest about what has been captured.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_proto`.
/// That is the expected tests-first signal.
use marsad_proto::{parse_stream_line, ContentBlock, ResultSubtype, StreamMsg};

// Path helpers — resolved relative to CARGO_MANIFEST_DIR so they always point
// to the right location regardless of where cargo is invoked from.
fn fixture_path(filename: &str) -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../fixtures/streams")
        .join(filename)
}

fn hook_path(filename: &str) -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../fixtures/hooks")
        .join(filename)
}

// ---------------------------------------------------------------------------
// qa.ndjson — init → assistant → result/success (CAPTURED)
// ---------------------------------------------------------------------------

/// Parse every line of the Q&A fixture. Zero ProtoErrors expected.
#[test]
fn qa_fixture_parses_all_lines_without_errors() {
    let path = fixture_path("qa.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));

    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        parse_stream_line(line)
            .unwrap_or_else(|e| panic!("line {} failed to parse: {e:?}\nline: {line}", i + 1));
    }
}

/// Verify the variant sequence in qa.ndjson: SystemInit, Assistant, Result/success.
#[test]
fn qa_fixture_variant_sequence() {
    let path = fixture_path("qa.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));

    let msgs: Vec<StreamMsg> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            parse_stream_line(line).unwrap_or_else(|e| panic!("parse failed: {e:?}\n{line}"))
        })
        .collect();

    assert_eq!(msgs.len(), 3, "qa.ndjson must have exactly 3 messages");

    assert!(
        matches!(&msgs[0], StreamMsg::SystemInit(_)),
        "first message must be SystemInit"
    );
    assert!(
        matches!(&msgs[1], StreamMsg::Assistant(_)),
        "second message must be Assistant"
    );
    assert!(
        matches!(&msgs[2], StreamMsg::Result(_)),
        "third message must be Result"
    );
}

/// Verify the SystemInit fields from the real capture.
#[test]
fn qa_fixture_system_init_fields() {
    let path = fixture_path("qa.ndjson");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let first_line = content.lines().next().expect("qa.ndjson must not be empty");
    let msg = parse_stream_line(first_line).expect("SystemInit must parse");

    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(
                init.session_id, "59215d34-0f57-4b68-9419-3bb8e27d6fab",
                "session_id must match capture"
            );
            assert_eq!(init.cwd, "/work/mewais/Marsad", "cwd must match capture");
            // model may vary between captures; just assert non-empty
            assert!(!init.model.is_empty(), "model must be non-empty");
            // tools list must be non-empty
            assert!(!init.tools.is_empty(), "tools list must be non-empty");
            // permissionMode must be present
            assert_eq!(init.permission_mode, "default");
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

/// Verify the Assistant message from the real capture — content block + usage.
#[test]
fn qa_fixture_assistant_message_fields() {
    let path = fixture_path("qa.ndjson");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let second_line = content
        .lines()
        .nth(1)
        .expect("qa.ndjson must have 2+ lines");
    let msg = parse_stream_line(second_line).expect("Assistant must parse");

    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.id, "msg_vrtx_019BtVq34GVCztT9N466pLQd");
            assert_eq!(
                a.message.content.len(),
                1,
                "must have exactly one content block"
            );
            match &a.message.content[0] {
                ContentBlock::Text { text } => {
                    assert!(
                        text.contains("4"),
                        "assistant reply must mention the number 4 (2+2=4)"
                    );
                }
                other => panic!("expected Text block, got {other:?}"),
            }
            // usage from capture
            assert_eq!(a.message.usage.input_tokens, 2);
            assert_eq!(a.message.usage.output_tokens, 1);
            assert_eq!(a.message.usage.cache_creation_input_tokens, 42448);
            assert_eq!(a.message.usage.cache_read_input_tokens, 0);
        }
        other => panic!("expected Assistant, got {other:?}"),
    }
}

/// Verify the Result/success message from the real capture.
#[test]
fn qa_fixture_result_message_fields() {
    let path = fixture_path("qa.ndjson");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let third_line = content.lines().nth(2).expect("qa.ndjson must have 3 lines");
    let msg = parse_stream_line(third_line).expect("Result must parse");

    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.subtype, ResultSubtype::Success);
            assert!(!r.is_error);
            assert_eq!(r.num_turns, 1);
            // cost from the real capture
            assert!(
                (r.total_cost_usd - 0.796905).abs() < 1e-4,
                "total_cost_usd must match capture (got {})",
                r.total_cost_usd
            );
            assert!(r.permission_denials.is_empty());
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

/// Count variants in qa.ndjson: expect 1 SystemInit, 1 Assistant, 1 Result.
#[test]
fn qa_fixture_variant_counts() {
    let path = fixture_path("qa.ndjson");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));

    let (mut inits, mut assistants, mut results, mut unknowns) = (0u32, 0u32, 0u32, 0u32);
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        match parse_stream_line(line).expect("all lines must parse") {
            StreamMsg::SystemInit(_) => inits += 1,
            StreamMsg::Assistant(_) => assistants += 1,
            StreamMsg::Result(_) => results += 1,
            StreamMsg::Unknown(_) => unknowns += 1,
            _ => {}
        }
    }
    assert_eq!(inits, 1, "qa.ndjson: 1 SystemInit");
    assert_eq!(assistants, 1, "qa.ndjson: 1 Assistant");
    assert_eq!(results, 1, "qa.ndjson: 1 Result");
    assert_eq!(
        unknowns, 0,
        "qa.ndjson: 0 Unknown (all lines are recognized)"
    );
}

// ---------------------------------------------------------------------------
// permission.ndjson — can_use_tool control request (TODO fixture)
// ---------------------------------------------------------------------------

/// Parse all lines of the permission fixture, including a ControlRequest.
///
/// IGNORED: fixture not yet captured.
/// See fixtures/README.md — run `scripts/capture_fixtures.sh` on a toolchain machine
/// to produce `fixtures/streams/permission.ndjson`, then un-ignore this test.
#[test]
#[ignore = "fixture not yet captured — see fixtures/README.md"]
fn permission_fixture_parses_all_lines() {
    let path = fixture_path("permission.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    let mut control_requests = 0u32;
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let msg =
            parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
        if matches!(msg, StreamMsg::ControlRequest(_)) {
            control_requests += 1;
        }
    }
    assert!(
        control_requests >= 1,
        "permission fixture must contain at least one ControlRequest"
    );
}

// ---------------------------------------------------------------------------
// plan.ndjson — plan-mode gate (TODO fixture)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "fixture not yet captured — see fixtures/README.md"]
fn plan_fixture_parses_all_lines() {
    let path = fixture_path("plan.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
    }
}

// ---------------------------------------------------------------------------
// edit.ndjson — edit-approval gate (TODO fixture)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "fixture not yet captured — see fixtures/README.md"]
fn edit_fixture_parses_all_lines() {
    let path = fixture_path("edit.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
    }
}

// ---------------------------------------------------------------------------
// subagent.ndjson — subagent spawn (TODO fixture)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "fixture not yet captured — see fixtures/README.md"]
fn subagent_fixture_parses_all_lines() {
    let path = fixture_path("subagent.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
    }
}

// ---------------------------------------------------------------------------
// compaction.ndjson — compaction event (TODO fixture)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "fixture not yet captured — see fixtures/README.md"]
fn compaction_fixture_parses_all_lines() {
    let path = fixture_path("compaction.ndjson");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
    }
}

// ---------------------------------------------------------------------------
// hooks/*.json — hook payload samples (TODO fixtures)
//
// The hooks directory is created by the capture script. Each file is a JSON
// object (not NDJSON) representing a single hook event payload.  The hook
// payloads are NOT stream-json messages; they are parsed by B02 (marsad-events).
// We include path-gated smoke tests here so that once the captures exist they
// are automatically exercised.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hook fixtures not yet captured — see fixtures/README.md"]
fn hook_notification_is_valid_json() {
    let path = hook_path("Notification.json");
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {:?}: {e}", path));
    let _v: serde_json::Value =
        serde_json::from_str(&content).expect("Notification.json must be valid JSON");
}

#[test]
#[ignore = "hook fixtures not yet captured — see fixtures/README.md"]
fn hook_stop_is_valid_json() {
    let path = hook_path("Stop.json");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let _v: serde_json::Value =
        serde_json::from_str(&content).expect("Stop.json must be valid JSON");
}

#[test]
#[ignore = "hook fixtures not yet captured — see fixtures/README.md"]
fn hook_pre_tool_use_is_valid_json() {
    let path = hook_path("PreToolUse.json");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let _v: serde_json::Value =
        serde_json::from_str(&content).expect("PreToolUse.json must be valid JSON");
}

#[test]
#[ignore = "hook fixtures not yet captured — see fixtures/README.md"]
fn hook_post_tool_use_is_valid_json() {
    let path = hook_path("PostToolUse.json");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let _v: serde_json::Value =
        serde_json::from_str(&content).expect("PostToolUse.json must be valid JSON");
}
