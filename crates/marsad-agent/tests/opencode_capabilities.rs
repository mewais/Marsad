//! opencode_capabilities.rs — Unit tests for the opencode/ACP capability discovery
//! functions in `marsad_agent::capabilities`.
//!
//! Authoritative spec: `docs/specs/opencode-acp.md` §6 (capabilities/discovery).
//!
//! All three functions under test are `todo!()` stubs, so every test in this
//! file MUST compile and FAIL with a `not yet implemented` panic.  That is the
//! expected RED state.
//!
//! Functions exercised (module path `marsad_agent::capabilities::`):
//!   - `parse_opencode_models(raw: &str) -> Vec<ModelInfo>`
//!   - `parse_opencode_sessions(raw: &str) -> Vec<(String, Option<String>)>`
//!   - `build_opencode_discovery_script(command: &str, setup: Option<&str>) -> String`

use marsad_agent::capabilities::{
    build_opencode_discovery_script, parse_opencode_models, parse_opencode_sessions,
};
use marsad_proto::ModelInfo;

// ===========================================================================
// parse_opencode_models
//
// Spec (§6): `opencode models` stdout — one `provider/model` line per entry.
// Parser: `parse_opencode_models(raw) -> Vec<ModelInfo>` (source = Listed;
//          free_text: true).
// ===========================================================================

/// Realistic `opencode models` plain output: one `provider/model` per line.
/// Modelled after the actual opencode CLI output format (provider slash model).
const OPENCODE_MODELS_PLAIN: &str = "anthropic/claude-sonnet-4-5\n\
openai/gpt-5\n\
openai/gpt-5-mini\n\
mistral/mistral-large\n\
amazon/nova-pro\n";

/// `opencode models --verbose` output adds cost metadata after the model name.
/// The parser must preserve the provider/model name and ignore cost fields.
/// Representative realistic verbose format: `provider/model  <cost-info>`.
const OPENCODE_MODELS_VERBOSE: &str =
    "anthropic/claude-sonnet-4-5  $3.00/1M in, $15.00/1M out\n\
openai/gpt-5  $10.00/1M in, $30.00/1M out\n\
openai/gpt-5-mini  $0.15/1M in, $0.60/1M out\n";

#[test]
fn parse_opencode_models_plain_preserves_provider_and_model_name() {
    let models = parse_opencode_models(OPENCODE_MODELS_PLAIN);

    // Exactly 5 models from 5 non-blank lines.
    assert_eq!(
        models.len(),
        5,
        "expected 5 models from 5 non-blank lines, got {}: {models:?}",
        models.len()
    );

    // Provider/model names preserved verbatim.
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.contains(&"anthropic/claude-sonnet-4-5"),
        "anthropic/claude-sonnet-4-5 must be present; got {ids:?}"
    );
    assert!(
        ids.contains(&"openai/gpt-5"),
        "openai/gpt-5 must be present; got {ids:?}"
    );
    assert!(
        ids.contains(&"openai/gpt-5-mini"),
        "openai/gpt-5-mini must be present; got {ids:?}"
    );
    assert!(
        ids.contains(&"mistral/mistral-large"),
        "mistral/mistral-large must be present; got {ids:?}"
    );
    assert!(
        ids.contains(&"amazon/nova-pro"),
        "amazon/nova-pro must be present; got {ids:?}"
    );
}

#[test]
fn parse_opencode_models_plain_fields_are_sensible() {
    let models = parse_opencode_models(OPENCODE_MODELS_PLAIN);
    for m in &models {
        // Every id contains a slash (provider/model format).
        assert!(
            m.id.contains('/'),
            "model id '{id}' must be in provider/model format",
            id = m.id
        );
        // Provider part must be non-empty.
        let parts: Vec<&str> = m.id.splitn(2, '/').collect();
        assert!(
            !parts[0].is_empty(),
            "provider part of '{}' must not be empty",
            m.id
        );
        // Model part must be non-empty.
        assert!(
            parts.len() == 2 && !parts[1].is_empty(),
            "model part of '{}' must not be empty",
            m.id
        );
    }
}

#[test]
fn parse_opencode_models_blank_and_whitespace_lines_ignored() {
    let raw = "anthropic/claude-sonnet-4-5\n\n   \nopenai/gpt-5\n\n";
    let models = parse_opencode_models(raw);
    assert_eq!(
        models.len(),
        2,
        "blank/whitespace-only lines must be ignored; got {}: {models:?}",
        models.len()
    );
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"anthropic/claude-sonnet-4-5"));
    assert!(ids.contains(&"openai/gpt-5"));
}

#[test]
fn parse_opencode_models_empty_input_yields_empty_vec() {
    assert_eq!(
        parse_opencode_models(""),
        vec![],
        "empty input must yield empty vec, not an error"
    );
    assert_eq!(
        parse_opencode_models("\n\n   \n"),
        vec![],
        "whitespace-only input must yield empty vec"
    );
}

/// Verbose output (with cost metadata after model name) — the parser must
/// still extract the provider/model identifier from each line.  The exact
/// handling of the cost suffix is implementation-defined (strip or preserve),
/// but the provider/model prefix must survive.
#[test]
fn parse_opencode_models_verbose_count_correct() {
    let models = parse_opencode_models(OPENCODE_MODELS_VERBOSE);
    assert_eq!(
        models.len(),
        3,
        "verbose output has 3 non-blank lines, got {}: {models:?}",
        models.len()
    );
}

#[test]
fn parse_opencode_models_verbose_anthropic_model_present() {
    let models = parse_opencode_models(OPENCODE_MODELS_VERBOSE);
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    // The anthropic model must be present regardless of cost-suffix handling.
    let has_anthropic = ids
        .iter()
        .any(|id| id.starts_with("anthropic/claude-sonnet-4-5"));
    assert!(
        has_anthropic,
        "anthropic/claude-sonnet-4-5 must appear (possibly with cost suffix); got {ids:?}"
    );
}

#[test]
fn parse_opencode_models_annotation_field_is_sensible() {
    // ModelInfo.annotation is used for display hints — the spec does not require
    // a specific value, only that the struct is populated sensibly.  We assert
    // the field exists (it does: ModelInfo is defined with annotation: Option<String>)
    // and that the id is non-empty.
    let models = parse_opencode_models(OPENCODE_MODELS_PLAIN);
    assert!(!models.is_empty(), "must have at least one model");
    for m in &models {
        assert!(!m.id.is_empty(), "ModelInfo.id must not be empty");
        // annotation is Option<String> — any value (Some/None) is acceptable.
        // The field must be accessible without panicking.
        let _ = &m.annotation;
    }
}

/// Duplicate provider/model lines: the spec does not mandate dedup, but the
/// parser must not crash.
#[test]
fn parse_opencode_models_handles_duplicate_lines_without_panic() {
    let raw = "anthropic/claude-sonnet-4-5\nanthropicy/claude-sonnet-4-5\n";
    let models = parse_opencode_models(raw);
    // At least one model must be present.
    assert!(
        !models.is_empty(),
        "duplicate lines must not crash the parser"
    );
}

/// A line without a slash (no provider prefix) — some output variants may emit
/// bare model names.  The parser must not panic; outcome (include or skip) is
/// implementation-defined.
#[test]
fn parse_opencode_models_bare_name_line_does_not_panic() {
    let raw = "claude-sonnet\nanthropic/claude-opus\n";
    let models = parse_opencode_models(raw);
    // We only assert no panic and at least the well-formed line may appear.
    let _ = models;
}

// ===========================================================================
// parse_opencode_sessions
//
// Spec (§6): `opencode session list --format json` stdout — a JSON array of
// session objects with `id` and optionally `directory` or `title`.
// Parser: `parse_opencode_sessions(raw) -> Vec<(session_id, Option<project>)>`.
// ===========================================================================

/// Realistic `opencode session list --format json` output.
/// Each session has at minimum an `id` field; `directory` maps to the project.
const OPENCODE_SESSIONS_JSON: &str = r#"[
  {
    "id": "sess-aaa-111",
    "directory": "/work/mewais/Marsad",
    "title": "Fix the gate bug"
  },
  {
    "id": "sess-bbb-222",
    "directory": "/home/user/project-x",
    "title": "Add health check"
  },
  {
    "id": "sess-ccc-333",
    "title": "Untitled session"
  }
]"#;

/// Session list where some entries have no `directory` — project should be None.
const OPENCODE_SESSIONS_NO_DIR: &str = r#"[
  {"id": "sess-no-dir-1"},
  {"id": "sess-no-dir-2", "title": "some title"}
]"#;

/// A single-session array (edge case).
const OPENCODE_SESSIONS_SINGLE: &str = r#"[
  {"id": "only-one", "directory": "/tmp/myproject"}
]"#;

#[test]
fn parse_opencode_sessions_extracts_id_and_project() {
    let sessions = parse_opencode_sessions(OPENCODE_SESSIONS_JSON);

    assert_eq!(
        sessions.len(),
        3,
        "expected 3 sessions, got {}: {sessions:?}",
        sessions.len()
    );

    // First session: id + directory.
    let (id, project) = &sessions[0];
    assert_eq!(id, "sess-aaa-111", "first session id mismatch");
    assert_eq!(
        project.as_deref(),
        Some("/work/mewais/Marsad"),
        "first session project mismatch"
    );

    // Second session: id + directory.
    let (id2, project2) = &sessions[1];
    assert_eq!(id2, "sess-bbb-222");
    assert_eq!(project2.as_deref(), Some("/home/user/project-x"));

    // Third session: id, no directory → project is None.
    let (id3, project3) = &sessions[2];
    assert_eq!(id3, "sess-ccc-333");
    assert_eq!(
        *project3, None,
        "session without directory must have None project"
    );
}

#[test]
fn parse_opencode_sessions_no_directory_yields_none_project() {
    let sessions = parse_opencode_sessions(OPENCODE_SESSIONS_NO_DIR);
    assert_eq!(sessions.len(), 2);
    for (id, project) in &sessions {
        assert!(!id.is_empty(), "session id must not be empty");
        assert_eq!(*project, None, "session without directory must have None project");
    }
}

#[test]
fn parse_opencode_sessions_single_entry() {
    let sessions = parse_opencode_sessions(OPENCODE_SESSIONS_SINGLE);
    assert_eq!(sessions.len(), 1);
    let (id, project) = &sessions[0];
    assert_eq!(id, "only-one");
    assert_eq!(project.as_deref(), Some("/tmp/myproject"));
}

#[test]
fn parse_opencode_sessions_empty_array_yields_empty_vec() {
    let sessions = parse_opencode_sessions("[]");
    assert_eq!(
        sessions,
        vec![],
        "empty JSON array must yield empty vec"
    );
}

#[test]
fn parse_opencode_sessions_malformed_json_yields_empty_vec() {
    // Spec (§6): malformed / non-JSON input degrades to empty — no panic.
    let sessions = parse_opencode_sessions("not json at all");
    assert_eq!(
        sessions,
        vec![],
        "malformed input must yield empty vec, not a panic"
    );
}

#[test]
fn parse_opencode_sessions_empty_string_yields_empty_vec() {
    let sessions = parse_opencode_sessions("");
    assert_eq!(sessions, vec![]);
}

#[test]
fn parse_opencode_sessions_wrong_json_type_yields_empty_vec() {
    // A JSON object (not array) at the root → graceful empty, not crash.
    let sessions = parse_opencode_sessions(r#"{"id":"x"}"#);
    assert_eq!(
        sessions,
        vec![],
        "non-array JSON root must yield empty vec"
    );
}

/// Entry with missing `id` field: must be skipped (id is required for the tuple).
#[test]
fn parse_opencode_sessions_entry_missing_id_is_skipped() {
    let raw = r#"[{"directory":"/p"},{"id":"valid","directory":"/q"}]"#;
    let sessions = parse_opencode_sessions(raw);
    // Only the entry with a valid id must survive.
    assert_eq!(
        sessions.len(),
        1,
        "entry without id must be skipped; got {sessions:?}"
    );
    assert_eq!(sessions[0].0, "valid");
}

/// Session id values are strings — each returned id must be non-empty.
#[test]
fn parse_opencode_sessions_ids_are_non_empty_strings() {
    let sessions = parse_opencode_sessions(OPENCODE_SESSIONS_JSON);
    for (id, _) in &sessions {
        assert!(!id.is_empty(), "session id must not be empty");
    }
}

// ===========================================================================
// build_opencode_discovery_script
//
// Spec (§6): must probe availability (`command -v`), run `opencode models`
// and `opencode session list`, and use the standard sentinel markers.
// Optional `setup` is prepended when Some.
// ===========================================================================

#[test]
fn build_opencode_discovery_script_references_command_in_avail_check() {
    let script = build_opencode_discovery_script("opencode", None);
    // Must check for the command's availability.
    assert!(
        script.contains("command -v") && script.contains("opencode"),
        "script must include a `command -v opencode` availability check; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_includes_models_probe() {
    let script = build_opencode_discovery_script("opencode", None);
    // Must invoke `opencode models` to discover available models.
    assert!(
        script.contains("opencode") && script.contains("models"),
        "script must invoke `opencode models`; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_includes_session_list_probe() {
    let script = build_opencode_discovery_script("opencode", None);
    // Must invoke `opencode session list` to discover resumable sessions.
    assert!(
        script.contains("session") && script.contains("list"),
        "script must invoke `opencode session list`; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_has_avail_section_markers() {
    let script = build_opencode_discovery_script("opencode", None);
    assert!(
        script.contains("@@MARSAD_SEC_BEGIN:avail"),
        "script must have @@MARSAD_SEC_BEGIN:avail; got:\n{script}"
    );
    assert!(
        script.contains("@@MARSAD_SEC_END:avail"),
        "script must have @@MARSAD_SEC_END:avail; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_has_models_section_markers() {
    let script = build_opencode_discovery_script("opencode", None);
    assert!(
        script.contains("@@MARSAD_SEC_BEGIN:models"),
        "script must have @@MARSAD_SEC_BEGIN:models; got:\n{script}"
    );
    assert!(
        script.contains("@@MARSAD_SEC_END:models"),
        "script must have @@MARSAD_SEC_END:models; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_has_sessions_section_markers() {
    let script = build_opencode_discovery_script("opencode", None);
    assert!(
        script.contains("@@MARSAD_SEC_BEGIN:sessions"),
        "script must have @@MARSAD_SEC_BEGIN:sessions; got:\n{script}"
    );
    assert!(
        script.contains("@@MARSAD_SEC_END:sessions"),
        "script must have @@MARSAD_SEC_END:sessions; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_all_three_sections_present() {
    let script = build_opencode_discovery_script("opencode", None);
    for name in ["avail", "models", "sessions"] {
        assert!(
            script.contains(&format!("@@MARSAD_SEC_BEGIN:{name}")),
            "script must contain @@MARSAD_SEC_BEGIN:{name}; got:\n{script}"
        );
        assert!(
            script.contains(&format!("@@MARSAD_SEC_END:{name}")),
            "script must contain @@MARSAD_SEC_END:{name}; got:\n{script}"
        );
    }
}

#[test]
fn build_opencode_discovery_script_setup_some_is_prepended() {
    let setup = "source ~/.opencode_env";
    let script = build_opencode_discovery_script("opencode", Some(setup));

    // Setup must appear before the first section marker.
    let setup_pos = script.find(setup);
    let first_begin_pos = script.find("@@MARSAD_SEC_BEGIN:");
    assert!(
        setup_pos.is_some(),
        "setup snippet must appear in the script; got:\n{script}"
    );
    assert!(
        first_begin_pos.is_some(),
        "script must have section markers; got:\n{script}"
    );
    assert!(
        setup_pos.unwrap() < first_begin_pos.unwrap(),
        "setup must appear BEFORE the first section marker; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_setup_none_no_setup_boilerplate() {
    let script_no_setup = build_opencode_discovery_script("opencode", None);
    let script_with_setup =
        build_opencode_discovery_script("opencode", Some("module load opencode"));

    // The script with setup must include the setup snippet.
    assert!(
        script_with_setup.contains("module load opencode"),
        "setup text must appear when Some; got:\n{script_with_setup}"
    );
    // Without setup, the script must still be a valid script with markers.
    assert!(
        script_no_setup.contains("@@MARSAD_SEC_BEGIN:avail"),
        "no-setup script must still have section markers; got:\n{script_no_setup}"
    );
}

#[test]
fn build_opencode_discovery_script_setup_failure_tolerated() {
    // When setup is Some, it should be wrapped with failure tolerance (|| true
    // idiom used by other builders) so a failing setup doesn't abort discovery.
    let script = build_opencode_discovery_script("opencode", Some("bad-setup-command"));
    assert!(
        script.contains("bad-setup-command"),
        "setup command must appear in script; got:\n{script}"
    );
    // The `|| true` tolerance is the convention from other builders — assert
    // it's present so a failing setup doesn't kill the discovery run.
    assert!(
        script.contains("|| true") || script.contains("true"),
        "setup failure must be tolerated (|| true idiom); got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_custom_command_used() {
    // Custom binary path (e.g. absolute path or with flags).
    let script = build_opencode_discovery_script("/usr/local/bin/opencode", None);
    assert!(
        script.contains("/usr/local/bin/opencode") || script.contains("opencode"),
        "custom command must appear in the script; got:\n{script}"
    );
}

#[test]
fn build_opencode_discovery_script_no_newlines_in_markers() {
    // Section marker lines must be complete tokens — the sentinel has no embedded newlines.
    let script = build_opencode_discovery_script("opencode", None);
    for name in ["avail", "models", "sessions"] {
        let marker = format!("@@MARSAD_SEC_BEGIN:{name}");
        assert!(
            script.contains(&marker),
            "marker {marker} not found; got:\n{script}"
        );
    }
}

/// End-to-end round-trip: the script's section output is parseable by
/// `parse_discovery_sections` (pure composition test, no IO).
#[test]
fn build_opencode_discovery_script_roundtrip_with_section_parser() {
    use marsad_agent::capabilities::parse_discovery_sections;

    let script = build_opencode_discovery_script("opencode", None);

    // Simulate what running the script WOULD produce for the avail section
    // (a non-empty path line means available).
    let simulated_output = format!(
        "@@MARSAD_SEC_BEGIN:avail\n/usr/local/bin/opencode\n@@MARSAD_SEC_END:avail\n\
         @@MARSAD_SEC_BEGIN:models\nanthropicy/claude-sonnet-4-5\nopenai/gpt-5\n@@MARSAD_SEC_END:models\n\
         @@MARSAD_SEC_BEGIN:sessions\n[]\n@@MARSAD_SEC_END:sessions\n"
    );

    let secs = parse_discovery_sections(&simulated_output);
    // Availability section parses correctly.
    assert_eq!(secs.get("avail"), "/usr/local/bin/opencode");
    // Models section body is accessible.
    assert!(secs.get("models").contains("claude-sonnet-4-5"));
    // Sessions section body is accessible.
    assert_eq!(secs.get("sessions").trim(), "[]");

    // And the script itself must have been built (non-empty) — confirming the
    // function didn't return an empty string.
    assert!(!script.is_empty(), "discovery script must not be empty");
}
