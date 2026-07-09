//! Integration test i02_gate_roundtrip
//!
//! Part A — Deterministic gate round-trip (always runs, no claude needed):
//!   - Build each Gate variant; approve/reject → GateDecision; route yields one session id.
//!   - to_control_response maps Approve→allow, Reject→deny+reason.
//!   - DecisionLedger records once (Ok), re-delivery same id → DuplicateDecision (no-op).
//!   - Over LoopbackFake transport: send control-response line from one end, peer receives it.
//!
//! Part B — Live real gate (skip-guarded; best-effort):
//!   - If `claude` is present, drive the REAL control protocol through Marsad's
//!     proto functions: initialize handshake → elicit a `can_use_tool` gate →
//!     answer allow AND deny → assert the real side effect (a marker file is
//!     created iff approved).
//!   - Skips loudly with eprintln! if claude is missing or no gate appears in time.

use marsad_gates::{
    approve, decision_id, reject, route, to_control_response, Decision, DecisionId, DecisionLedger,
    DiffLine, Gate, GateError, PlanStep, Sig,
};
use marsad_proto::encode_control_response;
use marsad_transport::{LoopbackFake, Transport};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn permission_gate() -> Gate {
    Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /tmp".to_string(),
        cwd: "/tmp".to_string(),
        risk: "low".to_string(),
        explanation: "list temp files".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
    }
}

fn plan_gate() -> Gate {
    Gate::Plan {
        request_id: "req-plan-i02".to_string(),
        summary: "Refactor module".to_string(),
        steps: vec![
            PlanStep {
                title: "Step 1".to_string(),
                detail: "do thing A".to_string(),
            },
            PlanStep {
                title: "Step 2".to_string(),
                detail: "do thing B".to_string(),
            },
        ],
    }
}

fn edit_gate() -> Gate {
    Gate::Edit {
        request_id: "req-edit-i02".to_string(),
        file: "src/main.rs".to_string(),
        diff: vec![
            DiffLine {
                ln: "1".to_string(),
                sig: Sig::Del,
                text: "let x = 0;".to_string(),
            },
            DiffLine {
                ln: "1".to_string(),
                sig: Sig::Add,
                text: "let x = 1;".to_string(),
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Part A — section 1: approve/reject for each gate variant; route; to_control_response
// ---------------------------------------------------------------------------

#[test]
fn permission_gate_approve() {
    let sid = "sess-perm-approve";
    let gate = permission_gate();

    let decision = approve(sid, &gate);

    assert_eq!(decision.session_id, sid);
    assert_eq!(decision.decision, Decision::Approve);
    // route yields exactly the session id
    assert_eq!(route(&decision), sid);

    // to_control_response: Approve → allow=true, no deny_reason
    let ctrl = to_control_response(&decision);
    assert!(ctrl.allow, "approve should yield allow=true");
    assert!(
        ctrl.deny_reason.is_none(),
        "approve should have no deny_reason"
    );
    assert_eq!(
        ctrl.request_id, decision.id.0,
        "request_id must equal decision id"
    );
}

#[test]
fn permission_gate_reject() {
    let sid = "sess-perm-reject";
    let gate = permission_gate();
    let reason = "too risky";

    let decision = reject(sid, &gate, reason);

    assert_eq!(decision.session_id, sid);
    assert_eq!(
        decision.decision,
        Decision::Reject {
            reason: reason.to_string()
        }
    );
    assert_eq!(route(&decision), sid);

    let ctrl = to_control_response(&decision);
    assert!(!ctrl.allow, "reject should yield allow=false");
    assert_eq!(ctrl.deny_reason.as_deref(), Some(reason));
    assert_eq!(ctrl.request_id, decision.id.0);
}

#[test]
fn plan_gate_approve() {
    let sid = "sess-plan-approve";
    let gate = plan_gate();

    let decision = approve(sid, &gate);

    assert_eq!(route(&decision), sid);
    let ctrl = to_control_response(&decision);
    assert!(ctrl.allow);
    assert!(ctrl.deny_reason.is_none());
}

#[test]
fn plan_gate_reject() {
    let sid = "sess-plan-reject";
    let gate = plan_gate();
    let reason = "plan is unclear";

    let decision = reject(sid, &gate, reason);

    assert_eq!(route(&decision), sid);
    let ctrl = to_control_response(&decision);
    assert!(!ctrl.allow);
    assert_eq!(ctrl.deny_reason.as_deref(), Some(reason));
}

#[test]
fn edit_gate_approve() {
    let sid = "sess-edit-approve";
    let gate = edit_gate();

    let decision = approve(sid, &gate);

    assert_eq!(route(&decision), sid);
    let ctrl = to_control_response(&decision);
    assert!(ctrl.allow);
    assert!(ctrl.deny_reason.is_none());
}

#[test]
fn edit_gate_reject() {
    let sid = "sess-edit-reject";
    let gate = edit_gate();
    let reason = "wrong file";

    let decision = reject(sid, &gate, reason);

    assert_eq!(route(&decision), sid);
    let ctrl = to_control_response(&decision);
    assert!(!ctrl.allow);
    assert_eq!(ctrl.deny_reason.as_deref(), Some(reason));
}

#[test]
fn reject_with_empty_reason() {
    let sid = "sess-empty-reason";
    let gate = permission_gate();
    let decision = reject(sid, &gate, "");
    let ctrl = to_control_response(&decision);
    assert!(!ctrl.allow);
    assert_eq!(ctrl.deny_reason.as_deref(), Some(""));
}

// ---------------------------------------------------------------------------
// Part A — section 2: decision_id stability + uniqueness
// ---------------------------------------------------------------------------

#[test]
fn decision_id_is_stable_same_inputs() {
    let gate = permission_gate();
    let id1 = decision_id("session-abc", &gate);
    let id2 = decision_id("session-abc", &gate);
    assert_eq!(id1, id2, "decision_id must be stable for same inputs");
}

#[test]
fn decision_id_differs_by_session() {
    let gate = permission_gate();
    let id_a = decision_id("session-alpha", &gate);
    let id_b = decision_id("session-beta", &gate);
    assert_ne!(id_a, id_b, "different sessions must yield different ids");
}

#[test]
fn decision_id_differs_by_gate_variant() {
    let sid = "session-same";
    let id_perm = decision_id(sid, &permission_gate());
    let id_plan = decision_id(sid, &plan_gate());
    let id_edit = decision_id(sid, &edit_gate());
    assert_ne!(id_perm, id_plan, "permission vs plan must differ");
    assert_ne!(id_perm, id_edit, "permission vs edit must differ");
    assert_ne!(id_plan, id_edit, "plan vs edit must differ");
}

#[test]
fn decision_id_differs_by_gate_content() {
    let sid = "session-content";
    let gate_a = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /tmp".to_string(),
        cwd: "/tmp".to_string(),
        risk: "low".to_string(),
        explanation: "".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
    };
    let gate_b = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "rm /etc/passwd".to_string(),
        cwd: "/etc".to_string(),
        risk: "high".to_string(),
        explanation: "".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
    };
    assert_ne!(
        decision_id(sid, &gate_a),
        decision_id(sid, &gate_b),
        "different commands must yield different ids",
    );
}

#[test]
fn approve_and_reject_share_same_decision_id() {
    // The id is (session, gate)-derived — not decision type-derived.
    let sid = "session-idshare";
    let gate = permission_gate();
    let id_approve = approve(sid, &gate).id;
    let id_reject = reject(sid, &gate, "nope").id;
    assert_eq!(
        id_approve, id_reject,
        "approve and reject for same gate must share the same id"
    );
}

// ---------------------------------------------------------------------------
// Part A — section 3: DecisionLedger idempotency
// ---------------------------------------------------------------------------

#[test]
fn ledger_records_first_delivery_ok() {
    let mut ledger = DecisionLedger::new();
    let decision = approve("sess-ledger-1", &permission_gate());

    assert!(!ledger.seen(&decision.id), "not yet recorded");
    assert!(
        ledger.record(&decision).is_ok(),
        "first record must succeed"
    );
    assert!(ledger.seen(&decision.id), "must be seen after record");
}

#[test]
fn ledger_rejects_duplicate_delivery() {
    let mut ledger = DecisionLedger::new();
    let decision = approve("sess-ledger-dup", &permission_gate());

    ledger.record(&decision).expect("first record");
    let err = ledger
        .record(&decision)
        .expect_err("second record must be Err");

    match err {
        GateError::DuplicateDecision(ref id) => {
            assert_eq!(
                id, &decision.id,
                "DuplicateDecision must carry the decision's id"
            );
        }
        other => panic!("expected DuplicateDecision, got {other:?}"),
    }
}

#[test]
fn ledger_separate_decisions_both_recorded() {
    let mut ledger = DecisionLedger::new();
    let d1 = approve("sess-a", &permission_gate());
    let d2 = approve("sess-b", &permission_gate());

    assert_ne!(d1.id, d2.id, "different sessions → different ids");
    ledger.record(&d1).expect("d1 first");
    ledger.record(&d2).expect("d2 first");

    assert!(ledger.seen(&d1.id));
    assert!(ledger.seen(&d2.id));
}

#[test]
fn ledger_seen_false_for_unrecorded_id() {
    let ledger = DecisionLedger::new();
    let id = DecisionId("nonexistent".to_string());
    assert!(!ledger.seen(&id));
}

#[test]
fn ledger_duplicate_routes_to_exactly_one_session() {
    // Prove that even after re-delivery attempt, routing is still deterministic:
    // the decision's session_id never changes.
    let sid = "sess-route-idempotent";
    let gate = permission_gate();
    let mut ledger = DecisionLedger::new();

    let decision = approve(sid, &gate);
    ledger.record(&decision).expect("first");
    // re-delivery is a DuplicateDecision; routing still yields the same session
    let _ = ledger.record(&decision); // ignore the error (no-op)
    assert_eq!(
        route(&decision),
        sid,
        "route still yields the original session"
    );
}

// ---------------------------------------------------------------------------
// Part A — section 4: over LoopbackFake transport — single-session routing
// (B03 + B08 cross-block contract)
// ---------------------------------------------------------------------------

#[test]
fn control_response_round_trips_over_loopback() {
    // Build a gate decision and encode its control response as a JSON line.
    let sid = "sess-transport";
    let gate = permission_gate();
    let decision = approve(sid, &gate);
    let ctrl = to_control_response(&decision);
    let wire_line = encode_control_response(&ctrl);

    // LoopbackFake::pair() gives two wired ends.
    let (mut sender, mut receiver) = LoopbackFake::pair();
    sender.connect().expect("sender connect");
    receiver.connect().expect("receiver connect");

    // Send from one end; the peer receives exactly that one line.
    sender.send_line(&wire_line).expect("send_line");

    let received = receiver
        .recv_line()
        .expect("recv_line")
        .expect("expected a line");
    assert_eq!(received, wire_line, "received line must equal sent line");

    // No more lines available.
    let none = receiver.recv_line().expect("second recv_line");
    assert!(none.is_none(), "no extra lines expected");
}

#[test]
fn reject_control_response_round_trips_over_loopback() {
    let sid = "sess-transport-reject";
    let gate = edit_gate();
    let reason = "edit not approved";
    let decision = reject(sid, &gate, reason);
    let ctrl = to_control_response(&decision);

    assert!(!ctrl.allow);
    assert_eq!(ctrl.deny_reason.as_deref(), Some(reason));

    let wire_line = encode_control_response(&ctrl);

    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().unwrap();
    b.connect().unwrap();

    a.send_line(&wire_line).unwrap();
    let received = b.recv_line().unwrap().expect("line");
    assert_eq!(received, wire_line);
}

#[test]
fn loopback_single_session_routing_no_cross_contamination() {
    // Two sessions, two transports. A decision for session A must not appear
    // on session B's transport.
    let (mut a_send, mut a_recv) = LoopbackFake::pair();
    let (mut b_send, mut b_recv) = LoopbackFake::pair();
    a_send.connect().unwrap();
    a_recv.connect().unwrap();
    b_send.connect().unwrap();
    b_recv.connect().unwrap();

    let gate_a = approve("sess-A", &permission_gate());
    let gate_b = approve("sess-B", &plan_gate());

    let line_a = encode_control_response(&to_control_response(&gate_a));
    let line_b = encode_control_response(&to_control_response(&gate_b));

    // Route each decision to the correct transport.
    a_send.send_line(&line_a).unwrap();
    b_send.send_line(&line_b).unwrap();

    // A's receiver sees only A's line.
    let recv_a = a_recv.recv_line().unwrap().expect("A line");
    assert_eq!(recv_a, line_a);
    assert!(a_recv.recv_line().unwrap().is_none(), "no extra lines on A");

    // B's receiver sees only B's line.
    let recv_b = b_recv.recv_line().unwrap().expect("B line");
    assert_eq!(recv_b, line_b);
    assert!(b_recv.recv_line().unwrap().is_none(), "no extra lines on B");
}

#[test]
fn drain_outbound_captures_sent_lines() {
    // The shared outbound log records every send for test inspection.
    let (mut sender, mut _receiver) = LoopbackFake::pair();
    sender.connect().unwrap();
    _receiver.connect().unwrap();

    let gate = approve("sess-drain", &edit_gate());
    let ctrl = to_control_response(&gate);
    let line = encode_control_response(&ctrl);

    sender.send_line(&line).unwrap();

    let drained = sender.drain_outbound();
    assert_eq!(drained.len(), 1, "one line sent");
    assert_eq!(drained[0], line);
}

// ---------------------------------------------------------------------------
// Part B — LIVE gate round-trip against real `claude` (skip-guarded)
// ---------------------------------------------------------------------------
//
// Proves the REAL Claude Code control protocol end-to-end, exercised entirely
// through Marsad's own proto functions:
//   1. launch claude with `--permission-prompt-tool stdio` (routes permission
//      prompts to the control stream) from a temp cwd with `--setting-sources
//      project` (so host/user allow-lists don't silently auto-approve);
//   2. send the `initialize` control_request (encode_control_request_initialize);
//   3. send a user prompt (encode_user_input) forcing a Bash tool use the CLI
//      cannot auto-classify as safe (contains `rm -rf` + a redirect);
//   4. read the `can_use_tool` control_request (parse_stream_line);
//   5. answer with encode_control_response — allow OR deny;
//   6. assert the REAL side effect: a marker file exists iff we allowed.
//
// Skips loudly (eprintln! + early return) if `claude` is missing or no gate is
// elicited within the timeout — Part A proves the gate domain deterministically.

use std::io::{BufRead, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn claude_present() -> bool {
    Command::new("which")
        .arg("claude")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Outcome of one live gate scenario.
struct GateOutcome {
    /// Tool name claude requested permission for (e.g. "Bash").
    tool_name: String,
    /// Whether the marker side effect happened (proves allow ran / deny blocked).
    marker_exists: bool,
}

/// Run one live gate scenario: elicit a real `can_use_tool` gate and answer it
/// `allow` (true) or deny (false). Returns Ok(outcome) or Err(reason) for a loud skip.
fn run_live_gate(allow: bool) -> Result<GateOutcome, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("clock: {e}"))?
        .as_nanos();
    let label = if allow { "allow" } else { "deny" };
    let tmp = std::env::temp_dir().join(format!("marsad-it-i02-{label}-{nanos}"));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("mkdir tmp: {e}"))?;

    struct Guard(std::path::PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _g = Guard(tmp.clone());

    // Exact marker path we control; created only if the gated command runs.
    // (Fresh temp dir each run, so no cleanup/`rm` needed — and avoiding scary
    // commands like `rm -rf` keeps the model from responding cautiously with
    // text instead of actually invoking the tool.)
    let marker_dir = tmp.join("gate_marker_dir");
    let marker = marker_dir.join("marker.txt");
    // The `>` redirect makes the CLI's Bash safety classifier treat this as
    // NOT auto-safe, so the permission decision is "ask" → a real gate fires.
    let command = format!(
        "mkdir -p {md} && echo marsad-allowed > {mk}",
        md = marker_dir.display(),
        mk = marker.display(),
    );

    let mut child = Command::new("claude")
        .args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources",
            "project",
        ])
        .current_dir(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn claude: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("no child stdin")?;
    let stdout = child.stdout.take().ok_or("no child stdout")?;

    // Reader thread → streams non-empty lines over a channel.
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = line.trim_end().to_string();
                    if !t.is_empty() && tx.send(t).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 1) initialize handshake — Marsad proto.
    let init = marsad_proto::encode_control_request_initialize("req_1_init");
    stdin
        .write_all(init.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|e| format!("write initialize: {e}"))?;

    // 2) user prompt forcing the gated Bash tool — Marsad proto.
    let prompt = marsad_proto::encode_user_input(&marsad_proto::UserInput {
        content: vec![marsad_proto::ContentBlock::Text {
            text: format!(
                "Use the Bash tool to run exactly this command: {command} . \
                 You MUST actually invoke the Bash tool with that exact command; do not simulate it."
            ),
        }],
    });
    stdin
        .write_all(prompt.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|e| format!("write prompt: {e}"))?;

    // 3-5) read until claude finishes, answering every can_use_tool gate via the
    // real control protocol (Marsad proto parse + encode).
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut gate_tool: Option<String> = None;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = match rx.recv_timeout(remaining) {
            Ok(l) => l,
            Err(_) => break, // timeout or claude exited (channel closed)
        };
        match marsad_proto::parse_stream_line(&line) {
            Ok(marsad_proto::StreamMsg::ControlRequest(cr)) => {
                let resp = marsad_proto::ControlResponse {
                    request_id: cr.request_id.clone(),
                    allow,
                    deny_reason: if allow {
                        None
                    } else {
                        Some("denied by i02 live test".to_string())
                    },
                    // On approval the CLI USES updatedInput as the tool input —
                    // echo the original verbatim so the right command runs.
                    updated_input: if allow { Some(cr.input.clone()) } else { None },
                };
                let wire = marsad_proto::encode_control_response(&resp);
                stdin
                    .write_all(wire.as_bytes())
                    .and_then(|_| stdin.write_all(b"\n"))
                    .and_then(|_| stdin.flush())
                    .map_err(|e| format!("write control_response: {e}"))?;
                if gate_tool.is_none() {
                    gate_tool = Some(cr.tool_name.clone());
                }
            }
            Ok(marsad_proto::StreamMsg::Result(_)) => break,
            _ => {}
        }
    }

    // In `--input-format stream-json` mode claude keeps stdin open for further
    // turns and will NOT exit on its own after the result — close stdin (EOF)
    // and stop the process so we don't block forever. By the time we saw the
    // `result`, the gated command has already run (or been blocked), so the
    // marker reflects the real outcome.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();

    let tool_name = gate_tool.ok_or_else(|| {
        "no can_use_tool gate elicited within timeout (model answered without tool use)".to_string()
    })?;

    // Brief settle so a just-created marker is visible.
    std::thread::sleep(Duration::from_millis(150));
    Ok(GateOutcome {
        tool_name,
        marker_exists: marker.exists(),
    })
}

/// Retry `run_live_gate` until a real gate is actually elicited. The model
/// occasionally answers a prompt without invoking the tool (no gate fires);
/// that's not a failure of the protocol, just non-determinism — so we retry a
/// few times before giving up and skipping loudly.
fn run_live_gate_retry(allow: bool, attempts: u32) -> Result<GateOutcome, String> {
    let mut last = String::from("no attempts made");
    for i in 0..attempts {
        match run_live_gate(allow) {
            Ok(o) => return Ok(o),
            Err(e) => {
                eprintln!(
                    "[i02 Part B] {} attempt {}/{} did not elicit a gate: {e}",
                    if allow { "allow" } else { "deny" },
                    i + 1,
                    attempts
                );
                last = e;
            }
        }
    }
    Err(last)
}

#[test]
fn live_gate_roundtrip_or_skip() {
    if !claude_present() {
        eprintln!(
            "[i02 Part B] SKIP: `claude` not found on PATH. \
             Part A fully proves the gate domain deterministically."
        );
        return;
    }
    eprintln!(
        "[i02 Part B] claude detected; eliciting REAL permission gates over the control protocol..."
    );

    // Scenario 1: ALLOW → the gated command runs → marker file is created.
    match run_live_gate_retry(true, 3) {
        Ok(o) => {
            assert!(
                !o.tool_name.is_empty(),
                "allow: a real can_use_tool gate must name a tool"
            );
            assert!(
                o.marker_exists,
                "allow: the approved Bash command must have created the marker file (real side effect)"
            );
            eprintln!(
                "[i02 Part B] PASS allow: real {} gate approved via the control protocol; command ran (marker created).",
                o.tool_name
            );
        }
        Err(e) => {
            eprintln!("[i02 Part B] SKIP allow scenario: {e}");
            return;
        }
    }

    // Scenario 2: DENY → the gated command is blocked → marker file is NOT created.
    match run_live_gate_retry(false, 3) {
        Ok(o) => {
            assert!(
                !o.tool_name.is_empty(),
                "deny: a real can_use_tool gate must name a tool"
            );
            assert!(
                !o.marker_exists,
                "deny: the rejected Bash command must NOT have run (no marker file)"
            );
            eprintln!(
                "[i02 Part B] PASS deny: real {} gate denied via the control protocol; command blocked (no marker).",
                o.tool_name
            );
        }
        Err(e) => {
            eprintln!("[i02 Part B] SKIP deny scenario: {e}");
        }
    }
}
