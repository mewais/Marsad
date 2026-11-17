//! capabilities_ipc.rs — golden-JSON regression suite for the U1 dynamic
//! capability-discovery wire surface.
//!
//! Authoritative spec: `docs/specs/dynamic-capabilities.md` §1 (the
//! `Capabilities`/`ModelCatalog`/`ModelInfo`/`ModelListSource`/`DiscoveredSession`/
//! `SessionScope` domain types), §2 (the three new `IpcMsg` variants +
//! their golden wire forms), and §6 ("marsad-proto" test plan). This file
//! is the additive companion to `tests/ipc_typed.rs` (M2, FROZEN) — it does
//! not touch or duplicate that file's 29-golden table; it follows the exact
//! same per-variant contract convention (golden parse / encode-parse
//! identity / re-encode stability / golden-encode-as-Value / unknown-field
//! tolerance) for the 3 NEW tags, plus the domain-type-level tests §6 calls
//! out explicitly (defaults, schema errors, `SessionScope` both arms).
//!
//! This file WILL fail to compile until the code-agent lands `Capabilities`,
//! `ModelCatalog`, `ModelInfo`, `ModelListSource`, `DiscoveredSession`,
//! `SessionScope`, and the `RefreshCapabilities`/`CapabilitiesRefreshQueued`/
//! `CapabilitiesUpdate` `IpcMsg` variants (§1-§2). That is the expected
//! tests-first compile-failure signal.

use marsad_proto::{
    encode_ipc, parse_ipc, Capabilities, DiscoveredSession, IpcMsg, ModelCatalog, ModelInfo,
    ModelListSource, ProtoError, SessionScope,
};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors tests/ipc_typed.rs's `val`/`inject_future_field`/
// `assert_variant_contract` exactly — duplicated here because each file in
// tests/ is compiled as an independent crate).
// ---------------------------------------------------------------------------

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

/// Inject an unknown field into a golden JSON object literal, mimicking a
/// newer sender adding an additive field this build doesn't know about yet
/// (§6: "cross-version tolerance").
fn inject_future_field(golden: &str) -> String {
    let mut v = val(golden);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    v.to_string()
}

/// The full per-variant contract (typed-ipc.md §6 items 1-4, extended to the
/// 3 new U1 tags): golden parse, encode→parse identity + re-encode
/// stability, golden encode as `Value`, and unknown-field tolerance.
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
// RefreshCapabilities (§2 golden forms — minimal + maximal)
// ---------------------------------------------------------------------------

const REFRESH_CAPABILITIES_MINIMAL: &str = r#"{"type":"RefreshCapabilities"}"#;

const REFRESH_CAPABILITIES_MAXIMAL: &str = r#"{"type":"RefreshCapabilities","host_id":"h1","driver":"claude","project":"/work/mewais/OpenSource/Marsad","host_target":"user@box","setup":"module load cuda","agent_commands":[["codex","codex --sandbox"]],"claude_bin":"genie"}"#;

#[test]
fn refresh_capabilities_minimal_golden_contract() {
    assert_variant_contract(
        REFRESH_CAPABILITIES_MINIMAL,
        &IpcMsg::RefreshCapabilities {
            host_id: None,
            driver: None,
            project: None,
            host_target: None,
            setup: None,
            agent_commands: vec![],
            claude_bin: None,
        },
    );
}

/// §6: "minimal `{"type":"RefreshCapabilities"}` parses to all-None."
#[test]
fn refresh_capabilities_minimal_parses_to_all_none() {
    match parse_ipc(REFRESH_CAPABILITIES_MINIMAL).expect("must parse") {
        IpcMsg::RefreshCapabilities {
            host_id,
            driver,
            project,
            host_target,
            setup,
            agent_commands,
            claude_bin,
        } => {
            assert_eq!(host_id, None);
            assert_eq!(driver, None);
            assert_eq!(project, None);
            assert_eq!(host_target, None);
            assert_eq!(setup, None);
            assert_eq!(agent_commands, Vec::<(String, String)>::new());
            assert_eq!(claude_bin, None);
        }
        other => panic!("expected RefreshCapabilities, got {other:?}"),
    }
}

#[test]
fn refresh_capabilities_maximal_golden_contract() {
    assert_variant_contract(
        REFRESH_CAPABILITIES_MAXIMAL,
        &IpcMsg::RefreshCapabilities {
            host_id: Some("h1".to_string()),
            driver: Some("claude".to_string()),
            project: Some("/work/mewais/OpenSource/Marsad".to_string()),
            host_target: Some("user@box".to_string()),
            setup: Some("module load cuda".to_string()),
            agent_commands: vec![("codex".to_string(), "codex --sandbox".to_string())],
            claude_bin: Some("genie".to_string()),
        },
    );
}

/// The minimal form must re-encode WITHOUT `host_id`/`driver`/`project` keys
/// (they are all `skip_serializing_if = "Option::is_none"` per §2) — proves
/// the `#[serde(default, skip_serializing_if = "Option::is_none")]`
/// attributes are wired, not just parse-tolerant.
#[test]
fn refresh_capabilities_minimal_reencode_omits_all_optionals() {
    let encoded = encode_ipc(&IpcMsg::RefreshCapabilities {
        host_id: None,
        driver: None,
        project: None,
        host_target: None,
        setup: None,
        agent_commands: vec![],
        claude_bin: None,
    });
    let obj = val(&encoded);
    let obj = obj.as_object().expect("must be a JSON object");
    for key in [
        "host_id",
        "driver",
        "project",
        "host_target",
        "setup",
        "agent_commands",
        "claude_bin",
    ] {
        assert!(
            !obj.contains_key(key),
            "minimal RefreshCapabilities must omit `{key}` on re-encode, got: {encoded}"
        );
    }
}

// ---------------------------------------------------------------------------
// CapabilitiesRefreshQueued (§2 golden form)
// ---------------------------------------------------------------------------

#[test]
fn capabilities_refresh_queued_golden_contract() {
    assert_variant_contract(
        r#"{"type":"CapabilitiesRefreshQueued","host_id":"local"}"#,
        &IpcMsg::CapabilitiesRefreshQueued {
            host_id: Some("local".to_string()),
        },
    );
}

#[test]
fn capabilities_refresh_queued_absent_host_id_parses_to_none_and_omits_on_reencode() {
    let line = r#"{"type":"CapabilitiesRefreshQueued"}"#;
    let parsed = parse_ipc(line).expect("must parse");
    assert_eq!(parsed, IpcMsg::CapabilitiesRefreshQueued { host_id: None });
    let encoded = encode_ipc(&parsed);
    let obj = val(&encoded);
    assert!(
        !obj.as_object().unwrap().contains_key("host_id"),
        "absent host_id must stay absent on re-encode, got: {encoded}"
    );
}

// ---------------------------------------------------------------------------
// CapabilitiesUpdate (§2 golden forms — "codex"/AllKnown and "claude"/Projects)
// ---------------------------------------------------------------------------

const CAPABILITIES_UPDATE_CODEX: &str = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex","host_id":"local","cli_available":true,"wasl_available":false,"launchable":true,"models":{"models":[{"id":"gpt-5.5"}],"source":"Listed","free_text":false},"session_scope":"AllKnown","sessions":[{"id":"97197d46-2091-40e2-b36f-0dcedd34bcc4","project":"/work/mewais/OpenSource/Marsad","snippet":"x","modified_ms":1783100616000}],"fetched_at_ms":1783100620000,"errors":[],"available_modes":[]}}"#;

const CAPABILITIES_UPDATE_CLAUDE: &str = r#"{"type":"CapabilitiesUpdate","host_id":"hpc","driver":"claude","capabilities":{"driver":"claude","host_id":"hpc","cli_available":true,"wasl_available":false,"launchable":true,"models":{"models":[{"id":"claude-opus-4-8"}],"source":"Observed","free_text":true},"session_scope":{"Projects":["/home/u/proj"]},"sessions":[],"fetched_at_ms":0,"errors":[],"available_modes":[]}}"#;

fn capabilities_update_codex() -> IpcMsg {
    IpcMsg::CapabilitiesUpdate {
        host_id: "local".to_string(),
        driver: "codex".to_string(),
        capabilities: Capabilities {
            driver: "codex".to_string(),
            host_id: "local".to_string(),
            cli_available: true,
            wasl_available: false,
            launchable: true,
            models: ModelCatalog {
                models: vec![ModelInfo {
                    id: "gpt-5.5".to_string(),
                    annotation: None,
                }],
                source: ModelListSource::Listed,
                free_text: false,
            },
            session_scope: Some(SessionScope::AllKnown),
            sessions: vec![DiscoveredSession {
                id: "97197d46-2091-40e2-b36f-0dcedd34bcc4".to_string(),
                project: Some("/work/mewais/OpenSource/Marsad".to_string()),
                snippet: "x".to_string(),
                modified_ms: 1783100616000,
            }],
            fetched_at_ms: 1783100620000,
            errors: vec![],
            available_modes: vec![],
        },
    }
}

fn capabilities_update_claude() -> IpcMsg {
    IpcMsg::CapabilitiesUpdate {
        host_id: "hpc".to_string(),
        driver: "claude".to_string(),
        capabilities: Capabilities {
            driver: "claude".to_string(),
            host_id: "hpc".to_string(),
            cli_available: true,
            wasl_available: false,
            launchable: true,
            models: ModelCatalog {
                models: vec![ModelInfo {
                    id: "claude-opus-4-8".to_string(),
                    annotation: None,
                }],
                source: ModelListSource::Observed,
                free_text: true,
            },
            session_scope: Some(SessionScope::Projects(vec!["/home/u/proj".to_string()])),
            sessions: vec![],
            fetched_at_ms: 0,
            errors: vec![],
            available_modes: vec![],
        },
    }
}

#[test]
fn capabilities_update_codex_golden_contract() {
    assert_variant_contract(CAPABILITIES_UPDATE_CODEX, &capabilities_update_codex());
}

#[test]
fn capabilities_update_claude_golden_contract() {
    assert_variant_contract(CAPABILITIES_UPDATE_CLAUDE, &capabilities_update_claude());
}

/// §2: "`driver` is the plain wire string (`"claude"` | `"codex"` | `"opencode"`)".
#[test]
fn capabilities_update_driver_field_is_plain_wire_string_not_an_enum() {
    // If `driver` were a typed enum, an arbitrary future string like
    // "some-future-cli" would fail to parse. It must not — proto must not
    // know the closed set of drivers (DAG constraint, §1).
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"some-future-cli","capabilities":{"driver":"some-future-cli","host_id":"local"}}"#;
    let parsed =
        parse_ipc(line).expect("an arbitrary driver string must parse — proto is driver-agnostic");
    match parsed {
        IpcMsg::CapabilitiesUpdate { driver, .. } => assert_eq!(driver, "some-future-cli"),
        other => panic!("expected CapabilitiesUpdate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Table-driven sweep over the 3 new §2 golden lines + LaunchSession-style
// unknown-field tolerance loop (§6: "unknown-field tolerance loop included").
// ---------------------------------------------------------------------------

fn all_new_goldens() -> Vec<(&'static str, &'static str, IpcMsg)> {
    vec![
        (
            "RefreshCapabilities (minimal)",
            REFRESH_CAPABILITIES_MINIMAL,
            IpcMsg::RefreshCapabilities {
                host_id: None,
                driver: None,
                project: None,
                host_target: None,
                setup: None,
                agent_commands: vec![],
                claude_bin: None,
            },
        ),
        (
            "RefreshCapabilities (maximal)",
            REFRESH_CAPABILITIES_MAXIMAL,
            IpcMsg::RefreshCapabilities {
                host_id: Some("h1".to_string()),
                driver: Some("claude".to_string()),
                project: Some("/work/mewais/OpenSource/Marsad".to_string()),
                host_target: Some("user@box".to_string()),
                setup: Some("module load cuda".to_string()),
                agent_commands: vec![("codex".to_string(), "codex --sandbox".to_string())],
                claude_bin: Some("genie".to_string()),
            },
        ),
        (
            "CapabilitiesRefreshQueued",
            r#"{"type":"CapabilitiesRefreshQueued","host_id":"local"}"#,
            IpcMsg::CapabilitiesRefreshQueued {
                host_id: Some("local".to_string()),
            },
        ),
        (
            "CapabilitiesUpdate (codex/AllKnown)",
            CAPABILITIES_UPDATE_CODEX,
            capabilities_update_codex(),
        ),
        (
            "CapabilitiesUpdate (claude/Projects)",
            CAPABILITIES_UPDATE_CLAUDE,
            capabilities_update_claude(),
        ),
    ]
}

#[test]
fn all_new_goldens_table_covers_all_5_lines() {
    assert_eq!(
        all_new_goldens().len(),
        5,
        "§2 lists exactly 5 new golden wire lines (2 RefreshCapabilities forms + 1 each of the other 2 tags)"
    );
}

#[test]
fn every_new_golden_parses_to_its_expected_typed_value() {
    for (name, golden, expected) in &all_new_goldens() {
        let parsed =
            parse_ipc(golden).unwrap_or_else(|e| panic!("{name}: golden must parse: {e:?}"));
        assert_eq!(&parsed, expected, "{name}: golden parse mismatch");
    }
}

/// §6: "unknown-field injection must not change the parsed value — every
/// variant, loop-driven" (the tolerance loop the task explicitly calls out).
#[test]
fn every_new_golden_tolerates_an_injected_unknown_field() {
    for (name, golden, expected) in &all_new_goldens() {
        let tolerant = inject_future_field(golden);
        let parsed = parse_ipc(&tolerant)
            .unwrap_or_else(|e| panic!("{name}: unknown-field-tolerant parse failed: {e:?}"));
        assert_eq!(
            &parsed, expected,
            "{name}: injected unknown field must not change the parsed value"
        );
    }
}

/// `parse_ipc`'s `KNOWN` list must grow to include the 3 new tags (§2: "the
/// three new IpcMsg variants"); an old build's `UnknownDiscriminant` on these
/// exact tag strings is the compat scenario this proves is CLOSED now.
#[test]
fn new_tags_are_no_longer_unknown_discriminants() {
    for tag in [
        "RefreshCapabilities",
        "CapabilitiesRefreshQueued",
        "CapabilitiesUpdate",
    ] {
        let line = format!(r#"{{"type":"{tag}"}}"#);
        let err = parse_ipc(&line);
        // RefreshCapabilities/CapabilitiesRefreshQueued parse fine with no
        // other fields; CapabilitiesUpdate is missing required fields, so it
        // must be a SchemaMismatch, NOT an UnknownDiscriminant — either way,
        // the discriminant itself must be recognized.
        if let Err(ProtoError::UnknownDiscriminant(t)) = err {
            panic!("tag {tag:?} must be known to parse_ipc, got UnknownDiscriminant({t:?})");
        }
    }
}

// ---------------------------------------------------------------------------
// Domain-type defaults (§6: "Capabilities with absent models/sessions/
// session_scope/fetched_at_ms/errors parses to defaults")
// ---------------------------------------------------------------------------

#[test]
fn capabilities_absent_optionals_parse_to_documented_defaults() {
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex","host_id":"local"}}"#;
    let parsed =
        parse_ipc(line).expect("Capabilities must tolerate every optional field being absent");
    match parsed {
        IpcMsg::CapabilitiesUpdate { capabilities, .. } => {
            assert!(
                !capabilities.cli_available,
                "cli_available defaults to false"
            );
            assert_eq!(
                capabilities.models,
                ModelCatalog::default(),
                "absent `models` must equal ModelCatalog::default()"
            );
            assert_eq!(capabilities.session_scope, None);
            assert_eq!(capabilities.sessions, Vec::<DiscoveredSession>::new());
            assert_eq!(capabilities.fetched_at_ms, 0);
            assert_eq!(capabilities.errors, Vec::<String>::new());
        }
        other => panic!("expected CapabilitiesUpdate, got {other:?}"),
    }
}

/// §1: "The honest empty state: nothing observed yet, free text allowed" —
/// `ModelCatalog::default()` must be exactly `{models: [], source: Observed,
/// free_text: true}`, pinned field-by-field so a future edit to the Default
/// impl (e.g. accidentally defaulting `source` to `Listed`) is caught here,
/// not just via equality against a second `Default::default()` call.
#[test]
fn model_catalog_default_is_the_documented_honest_empty_state() {
    let default = ModelCatalog::default();
    assert_eq!(default.models, Vec::<ModelInfo>::new());
    assert_eq!(default.source, ModelListSource::Observed);
    assert!(default.free_text, "default free_text must be true");
}

/// `ModelInfo.annotation` is `#[serde(default, skip_serializing_if =
/// "Option::is_none")]` — absent on the wire when `None`, and tolerant on
/// parse when absent.
#[test]
fn model_info_annotation_absent_roundtrips_to_none_and_omits_on_reencode() {
    let line = r#"{"id":"claude-opus-4-8"}"#;
    let parsed: ModelInfo =
        serde_json::from_str(line).expect("ModelInfo must parse without annotation");
    assert_eq!(
        parsed,
        ModelInfo {
            id: "claude-opus-4-8".to_string(),
            annotation: None
        }
    );
    let encoded = serde_json::to_string(&parsed).expect("must encode");
    assert!(
        !val(&encoded)
            .as_object()
            .unwrap()
            .contains_key("annotation"),
        "absent annotation must stay absent on re-encode, got: {encoded}"
    );
}

// ---------------------------------------------------------------------------
// SessionScope — both arms round-trip (§6)
// ---------------------------------------------------------------------------

#[test]
fn session_scope_all_known_roundtrips() {
    let encoded = serde_json::to_string(&SessionScope::AllKnown).expect("must encode");
    assert_eq!(val(&encoded), val(r#""AllKnown""#));
    let parsed: SessionScope = serde_json::from_str(&encoded).expect("must parse");
    assert_eq!(parsed, SessionScope::AllKnown);
}

#[test]
fn session_scope_projects_roundtrips() {
    let scope = SessionScope::Projects(vec!["/home/u/proj".to_string(), "/work/other".to_string()]);
    let encoded = serde_json::to_string(&scope).expect("must encode");
    assert_eq!(
        val(&encoded),
        val(r#"{"Projects":["/home/u/proj","/work/other"]}"#)
    );
    let parsed: SessionScope = serde_json::from_str(&encoded).expect("must parse");
    assert_eq!(parsed, scope);
}

#[test]
fn session_scope_projects_empty_vec_roundtrips() {
    let scope = SessionScope::Projects(vec![]);
    let encoded = serde_json::to_string(&scope).expect("must encode");
    let parsed: SessionScope = serde_json::from_str(&encoded).expect("must parse");
    assert_eq!(parsed, scope);
}

// ---------------------------------------------------------------------------
// Schema errors (§6)
// ---------------------------------------------------------------------------

/// §6: "missing `host_id` on `CapabilitiesUpdate` → `SchemaMismatch`."
#[test]
fn capabilities_update_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"CapabilitiesUpdate","driver":"codex","capabilities":{"driver":"codex","host_id":"local"}}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// §6: "missing `host_id` on `CapabilitiesUpdate`" — also cover the NESTED
/// `Capabilities.host_id` (required, no `#[serde(default)]` per §1's struct
/// definition) so a caller supplying the outer envelope's host_id but
/// omitting the nested one is still rejected, not silently defaulted.
#[test]
fn capabilities_update_missing_nested_capabilities_host_id_is_schema_mismatch() {
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex"}}"#;
    let err = parse_ipc(line)
        .expect_err("missing nested Capabilities.host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// §6: "unknown `ModelListSource` value → `SchemaMismatch`."
#[test]
fn capabilities_update_unknown_model_list_source_is_schema_mismatch() {
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex","host_id":"local","models":{"models":[],"source":"Bogus"}}}"#;
    let err = parse_ipc(line).expect_err("unknown ModelListSource must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// A `CapabilitiesUpdate` missing the whole `capabilities` object entirely
/// (not just a field within it) must also be a schema error, not a silent
/// default — `Capabilities` has no `#[serde(default)]` on the field itself
/// (§2: "capabilities: Capabilities" — required).
#[test]
fn capabilities_update_missing_capabilities_object_is_schema_mismatch() {
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex"}"#;
    let err = parse_ipc(line).expect_err("missing capabilities object must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}
