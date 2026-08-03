//! Regression tests for HITL gate raw-id correctness.
//!
//! ## The bug this guards against
//!
//! `capture_pending_gate` used to STRING-MUNGE the routing_id to recover the
//! JSON-RPC id: `strip_prefix("codex:")` / `rsplit_once(":acp:")`. But when the
//! routing_id is a toolCallId (the common case for codex command-approval gates and
//! opencode permission gates), there is NO prefix to strip — so the stored
//! `acp_response_id` ended up as the toolCall id string, not the real numeric
//! JSON-RPC id. The response's `"id"` then didn't match the request → the CLI never
//! resolved the gate → HITL hangs.
//!
//! ## What the fix does
//!
//! The wasit parsers (`wasit_codex::MessageParser` /
//! `wasit_opencode::MessageParser`) store the REAL `raw_id` in their
//! `pending_gates` map. After `on_message` processes a gate line, `pending_gate()`
//! gives us `(raw_id, method)`. We serialise `raw_id` to canonical JSON and store
//! that string in `PendingGate.acp_response_id`. On answer, we parse it back with
//! `serde_json::from_str` — a number stored as `"7"` comes back as `json!(7)`.
//!
//! ## Test structure
//!
//! These tests exercise the parsers' `pending_gate` API directly (pure, no IO)
//! and the canonical-JSON round-trip that `capture_pending_gate` /
//! `recover_codex_raw_id` / `recover_opencode_raw_id` rely on. They do NOT cover
//! the IO-gated `capture_pending_gate` itself (that is `#[cfg(not(coverage))]`),
//! but they PROVE the invariant that matters: for a toolCallId-routed gate the
//! parser stores the numeric wire id, and the canonical-JSON round-trip preserves it.

// ── 1. Codex: toolCallId-routed gate stores the numeric raw_id ────────────────

/// Real codex `item/commandExecution/requestApproval` wire line where
/// `itemId = "item-exec-1"` (the routing_id) but `id = 7` (the real JSON-RPC id).
///
/// Before the fix: `capture_pending_gate` would compute `acp_response_id =
/// "item-exec-1"` (verbatim routing_id, no prefix to strip). The response written
/// to codex would have `"id":"item-exec-1"` → mismatch → hang.
///
/// After the fix: parser's `pending_gate("item-exec-1").raw_id` = `json!(7)`;
/// canonical JSON = `"7"`; round-trip via `serde_json::from_str("7")` = `json!(7)`.
#[test]
fn codex_toolcallid_gate_stores_numeric_raw_id() {
    let line = r#"{"id":7,"method":"item/commandExecution/requestApproval","params":{"itemId":"item-exec-1","command":"rm -rf /tmp","cwd":"/proj"}}"#;
    let mut parser = wasit_codex::MessageParser::new();
    let events = parser.on_message(line);

    // The gate event was produced.
    assert_eq!(events.len(), 1, "expected exactly one gate event");

    // routing_id is the itemId.
    let routing_id = "item-exec-1";
    let gate = parser
        .pending_gate(routing_id)
        .expect("gate must be stored under the itemId routing_id");

    // raw_id must be the NUMERIC value 7, NOT the string "item-exec-1".
    assert_eq!(
        gate.raw_id,
        serde_json::json!(7),
        "codex gate raw_id must be the numeric JSON-RPC id (7), not the toolCallId"
    );

    // method must be the correct approval method.
    assert_eq!(gate.method, "item/commandExecution/requestApproval");

    // Canonical JSON round-trip: serde_json::to_string(7) -> "7" -> from_str -> json!(7).
    let canonical = serde_json::to_string(&gate.raw_id).expect("serializable");
    assert_eq!(canonical, "7", "canonical JSON of numeric 7 is the string '7'");
    let recovered: serde_json::Value =
        serde_json::from_str(&canonical).expect("parseable");
    assert_eq!(
        recovered,
        serde_json::json!(7),
        "round-tripped id must be the NUMBER 7, not a string"
    );
}

/// Legacy codex `execCommandApproval` gate with NO itemId.
/// Routing_id falls back to `"codex:5"` and raw_id is `json!(5)`.
/// This covers the `codex:<n>` prefix case — routing_id has a prefix but
/// raw_id is still stored correctly in the parser.
#[test]
fn codex_no_itemid_gate_stores_numeric_raw_id() {
    let line = r#"{"id":5,"method":"execCommandApproval","params":{"command":"echo hi","cwd":"/"}}"#;
    let mut parser = wasit_codex::MessageParser::new();
    let events = parser.on_message(line);
    assert_eq!(events.len(), 1);

    // routing_id is the fallback "codex:5".
    let gate = parser
        .pending_gate("codex:5")
        .expect("gate must be stored under codex:5 routing_id");

    assert_eq!(
        gate.raw_id,
        serde_json::json!(5),
        "fallback routing gate must still store the numeric raw_id 5"
    );

    // Canonical JSON round-trip.
    let canonical = serde_json::to_string(&gate.raw_id).expect("serializable");
    let recovered: serde_json::Value = serde_json::from_str(&canonical).expect("parseable");
    assert_eq!(recovered, serde_json::json!(5));
}

/// Codex `item/fileChange/requestApproval` gate — another common toolCallId case.
/// `itemId` absent on this shape; raw_id is the wire id.
#[test]
fn codex_file_change_gate_stores_numeric_raw_id() {
    let line = r#"{"id":6,"method":"item/fileChange/requestApproval","params":{"changes":{"path":"/proj/foo.rs"}}}"#;
    let mut parser = wasit_codex::MessageParser::new();
    let _events = parser.on_message(line);

    // No itemId → routing_id is "codex:6".
    let gate = parser
        .pending_gate("codex:6")
        .expect("gate must be stored under codex:6");

    assert_eq!(gate.raw_id, serde_json::json!(6));
    assert_eq!(gate.method, "item/fileChange/requestApproval");
}

/// Codex method is correctly stored alongside the raw_id.
/// Ensures that `WasitParser::pending_gate_raw` can return the real method.
#[test]
fn codex_gate_method_matches_wire_method() {
    let line = r#"{"id":9,"method":"item/permissions/requestApproval","params":{"itemId":"item-perm-2","permissions":["read","write"]}}"#;
    let mut parser = wasit_codex::MessageParser::new();
    parser.on_message(line);

    let gate = parser
        .pending_gate("item-perm-2")
        .expect("gate must be stored");

    assert_eq!(gate.raw_id, serde_json::json!(9));
    assert_eq!(gate.method, "item/permissions/requestApproval");
}

// ── 2. Opencode: toolCallId-routed gate stores the numeric raw_id ─────────────

/// Real opencode `session/request_permission` wire line where
/// `toolCallId = "toolu_perm"` (the routing_id) but `id = 9` (the real JSON-RPC id).
///
/// Before the fix: the stored `acp_response_id` would be `"toolu_perm"` (verbatim
/// routing_id, no `:acp:` segment). The response written to opencode would have
/// `"id":"toolu_perm"` → mismatch → hang.
///
/// After the fix: parser's `pending_gate("toolu_perm").raw_id` = `json!(9)`;
/// canonical JSON = `"9"`; round-trip = `json!(9)`.
#[test]
fn opencode_toolcallid_gate_stores_numeric_raw_id() {
    let line = r#"{"jsonrpc":"2.0","id":9,"method":"session/request_permission","params":{"sessionId":"ses_X","toolCall":{"toolCallId":"toolu_perm","title":"read file","kind":"read"},"options":[]}}"#;
    let mut parser = wasit_opencode::MessageParser::new();
    let events = parser.on_message(line);

    // The gate event was produced.
    assert_eq!(events.len(), 1, "expected exactly one gate event");

    // routing_id is the toolCallId.
    let routing_id = "toolu_perm";
    let gate = parser
        .pending_gate(routing_id)
        .expect("gate must be stored under the toolCallId routing_id");

    // raw_id must be numeric 9, NOT the string "toolu_perm".
    assert_eq!(
        gate.raw_id,
        serde_json::json!(9),
        "opencode gate raw_id must be the numeric JSON-RPC id (9), not the toolCallId"
    );

    // Canonical JSON round-trip.
    let canonical = serde_json::to_string(&gate.raw_id).expect("serializable");
    assert_eq!(canonical, "9");
    let recovered: serde_json::Value = serde_json::from_str(&canonical).expect("parseable");
    assert_eq!(
        recovered,
        serde_json::json!(9),
        "round-tripped id must be the NUMBER 9, not a string"
    );
}

/// Opencode gate with no toolCallId (falls back to `"<sid>:acp:<id>"`).
/// raw_id is still the numeric wire id.
#[test]
fn opencode_no_toolcallid_gate_stores_numeric_raw_id() {
    // No toolCallId in toolCall → routing falls back to "ses_B:acp:0".
    let line = r#"{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{"sessionId":"ses_B","toolCall":{"title":"write file","kind":"write"},"options":[]}}"#;
    let mut parser = wasit_opencode::MessageParser::new();
    let events = parser.on_message(line);
    assert_eq!(events.len(), 1);

    let gate = parser
        .pending_gate("ses_B:acp:0")
        .expect("gate must be stored under ses_B:acp:0");

    // The raw_id is json!(0) — the wire id, NOT derived from the routing_id string.
    assert_eq!(
        gate.raw_id,
        serde_json::json!(0),
        "opencode no-toolcallid gate must store the numeric wire id 0"
    );
}

// ── 3. Canonical-JSON round-trip for recover_* helpers ────────────────────────
//
// `recover_codex_raw_id` / `recover_opencode_raw_id` now do:
//   `serde_json::from_str(stored).unwrap_or_else(|_| Value::String(stored))`
// These tests verify the round-trip logic directly (no private fn access needed).

/// Number stored as canonical JSON → parses back as a number.
#[test]
fn canonical_json_numeric_roundtrip() {
    let raw = serde_json::json!(7u64);
    let stored = serde_json::to_string(&raw).unwrap();
    assert_eq!(stored, "7");
    let recovered: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(recovered, serde_json::json!(7u64));
    assert!(recovered.is_number(), "recovered value must be a number, not a string");
}

/// String stored as canonical JSON → parses back as a string (not double-wrapped).
#[test]
fn canonical_json_string_roundtrip() {
    let raw = serde_json::json!("call_abc");
    let stored = serde_json::to_string(&raw).unwrap();
    // Canonical JSON of a string is the quoted form.
    assert_eq!(stored, "\"call_abc\"");
    let recovered: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(recovered, serde_json::json!("call_abc"));
    assert!(recovered.is_string());
}

/// Legacy stored value (bare routing_id, no canonical JSON quoting) — fallback.
/// `serde_json::from_str("item-exec-1")` fails (not valid JSON), so the
/// fallback returns `Value::String("item-exec-1")` — graceful degradation.
#[test]
fn canonical_json_legacy_fallback() {
    let stored = "item-exec-1"; // old-style stored routing_id
    let recovered: serde_json::Value = serde_json::from_str(stored)
        .unwrap_or_else(|_| serde_json::Value::String(stored.to_string()));
    assert_eq!(recovered, serde_json::json!("item-exec-1"));
}

// ── 4. Both parsers: GateRequested routing_id matches pending_gate key ─────────

/// Codex: the routing_id in the GateRequested event equals the pending_gate key.
#[test]
fn codex_gate_requested_routing_id_matches_pending_gate_key() {
    let line = r#"{"id":42,"method":"item/commandExecution/requestApproval","params":{"itemId":"my-item","command":"ls"}}"#;
    let mut parser = wasit_codex::MessageParser::new();
    let events = parser.on_message(line);
    assert_eq!(events.len(), 1);

    let routing_id = match &events[0] {
        wasit_core::AgentEvent::GateRequested {
            gate: wasit_core::GateRequest::Permission { request_id, .. },
        } => request_id.clone(),
        other => panic!("expected GateRequested::Permission, got {other:?}"),
    };

    // The routing_id from the event is the key under which the parser stored the gate.
    let gate = parser
        .pending_gate(&routing_id)
        .expect("pending_gate must be keyed by the same routing_id as GateRequested");

    // And the raw_id must be the wire id, not the routing_id.
    assert_eq!(gate.raw_id, serde_json::json!(42));
    assert_ne!(
        gate.raw_id.as_str(),
        Some("my-item"),
        "raw_id must NOT be the routing_id string"
    );
}

/// Opencode: the routing_id in the GateRequested event equals the pending_gate key.
#[test]
fn opencode_gate_requested_routing_id_matches_pending_gate_key() {
    let line = r#"{"jsonrpc":"2.0","id":3,"method":"session/request_permission","params":{"sessionId":"ses_C","toolCall":{"toolCallId":"toolu_xyz","title":"exec","kind":"exec"},"options":[]}}"#;
    let mut parser = wasit_opencode::MessageParser::new();
    let events = parser.on_message(line);
    assert_eq!(events.len(), 1);

    let routing_id = match &events[0] {
        wasit_core::AgentEvent::GateRequested {
            gate: wasit_core::GateRequest::Permission { request_id, .. },
        } => request_id.clone(),
        other => panic!("expected GateRequested::Permission, got {other:?}"),
    };

    let gate = parser
        .pending_gate(&routing_id)
        .expect("pending_gate must be keyed by the same routing_id as GateRequested");

    assert_eq!(gate.raw_id, serde_json::json!(3));
    assert_ne!(
        gate.raw_id.as_str(),
        Some("toolu_xyz"),
        "raw_id must NOT be the routing_id string"
    );
}
