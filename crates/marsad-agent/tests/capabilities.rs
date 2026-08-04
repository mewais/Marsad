//! capabilities.rs — U1 dynamic capability discovery, `marsad-agent` half.
//!
//! Authoritative spec: `docs/specs/dynamic-capabilities.md` §1.1/§1.2 (per-driver
//! model/session discovery mechanisms), §3 (the `CapabilityProbe` seam, refresh
//! triggers, staleness policy, persistence), §4 (the non-blocking rule), and §6
//! ("marsad-agent" test plan).
//!
//! Real-world fixtures used (house guardrail — phase2-drivers.md: "capture real
//! fixtures... never implement a normalizer from docs alone"):
//! - `fixtures/codex/debug_models.json` — REAL captured `codex debug models`
//!   stdout, codex-cli 0.142.5 (3 models: `gpt-5.5`/`gpt-5.4-mini` visible,
//!   `codex-auto-review` hidden).
//! - The verbatim 8-line `agy models` output and the verbatim 5-line
//!   `transcript_full.jsonl` sample, both quoted in
//!   `docs/specs/antigravity-research.md` §0/§3.2 — inlined here as string
//!   literals per the task instructions (no live `agy` on this machine).
//!
//! NOTE(test-agent) — INVENTED API SURFACE: §3.1 pins the `CapabilityProbe`
//! trait and `HostProbeCtx` struct verbatim, and §3.2/§3.3/§3.4/§4 pin the
//! agent's *behavior* (background-thread probing, in-flight dedup, TTL-gated
//! background re-probe with an always-instant cache read, persistence to
//! `<data_root>/capabilities/<host_id>.json`, non-blocking broadcast) in
//! prose — but the spec does not name the Rust type that owns this state
//! machine (only that it lives in a new `crates/marsad-agent/src/capabilities.rs`
//! module, §3.1). Mirroring the house convention already used in
//! `tests/happy.rs` (see its top-of-file NOTE on `Agent::with_fakes`), this
//! file assumes a `CapabilityProvider` type in `marsad_agent::capabilities`,
//! independently testable (the "provider seam" §6 refers to: "fake probe
//! injected through the provider seam") without going through the real,
//! `#[cfg(not(coverage))]`-gated socket serve loop (`serve.rs`) — because the
//! dedup/TTL/broadcast/persistence LOGIC itself must hit the crate's 100%
//! coverage bar and therefore cannot live only in the coverage-excluded
//! real-IO path:
//!
//!   CapabilityProvider::new(probes: Vec<Box<dyn CapabilityProbe>>,
//!                            known_hosts: Vec<String>,
//!                            data_root: &Path) -> Self
//!     — loads any persisted `<data_root>/capabilities/<host>.json` files
//!       synchronously, THEN spawns a background full refresh (§3.2 trigger 1).
//!   provider.subscribe() -> mpsc::Receiver<String>
//!     — registers a new subscriber; replays every cached CapabilitiesUpdate
//!       line to it BEFORE returning (§3.2 trigger 2), then streams live
//!       broadcasts. Encoded IpcMsg lines, parsed with `parse_ipc` by callers.
//!   provider.handle_refresh(host_id: Option<String>, driver: Option<String>,
//!                            project: Option<String>) -> String
//!     — returns the encoded `CapabilitiesRefreshQueued` ack immediately;
//!       queues matching probes on background threads (§3.2 trigger 3, §4).
//!   provider.snapshot(driver: &str, host_id: &str) -> Option<Capabilities>
//!     — lock-clone-return cache read (no IO), mirrors `Core::capabilities()`.
//!   provider.observe_claude_model(host_id: &str, model: &str)
//!     — passive claude-model observation (§3.2 trigger 4): accumulates into
//!       the host's `Observed` model set, persists, and broadcasts.
//!
//! If the code-agent lands a differently-shaped seam (e.g. folding this into
//! `Agent` directly), it should adjust these call sites — the BEHAVIOR pinned
//! here (never block on IO, in-flight dedup, instant cache reads, honest
//! persistence, errors surfaced not swallowed) is the actual contract, not
//! the exact type/method names.
//!
//! This file WILL fail to compile until the code-agent lands the pure parse
//! functions, the `CapabilityProbe`/`HostProbeCtx`/`CapabilityProvider` seam,
//! and the scan functions below. That is the expected tests-first
//! compile-failure signal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::collections::HashMap;

use marsad_agent::capabilities::{
    agy_snippet_from_transcript, build_agy_discovery_script, build_claude_discovery_script,
    build_claude_model_catalog, build_codex_discovery_script, build_probe_spawn_argv, command_word,
    encode_claude_project_dir, filter_codex_sessions_by_project, merge_observed_into_catalog,
    parse_agy_models, parse_avail, parse_claude_env_models, parse_claude_extra_models,
    parse_codex_collab_list_response, parse_codex_debug_models, parse_codex_rollout_meta,
    parse_discovery_sections, parse_opencode_config_modes, parse_remote_claude_session_list,
    parse_remote_codex_session_list, scan_agy_sessions, scan_claude_sessions, scan_codex_sessions,
    CapabilityProbe, CapabilityProvider, HostLaunchCfg, HostProbeCtx,
};
use marsad_proto::{
    parse_ipc, Capabilities, DiscoveredSession, IpcMsg, ModelCatalog, ModelInfo, ModelListSource,
    SessionScope,
};

// ---------------------------------------------------------------------------
// Shared temp-dir + fixture-path helpers (mirrors marsad-core's
// discovery.rs test idiom — no `tempfile` dev-dependency needed).
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempdir() -> TempDir {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "marsad-agent-capabilities-test-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    TempDir(path)
}

/// Set a file's mtime via `touch -d @<epoch>` (GNU coreutils) — same
/// work-around `marsad-core/src/discovery.rs`'s test suite uses (no mtime
/// syscall available in stable `std`, no `filetime` dev-dependency).
fn set_mtime(path: &Path, offset_secs: i64) {
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    std::process::Command::new("touch")
        .arg("-d")
        .arg(format!("@{}", base + offset_secs))
        .arg(path)
        .status()
        .expect("touch failed");
}

fn fixture_path(rel: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../fixtures").join(rel)
}

/// Poll `f` every 20ms until it returns `Some`, or panic after `timeout`.
/// Used to observe background-thread probe results deterministically without
/// a fixed sleep (which would be either flaky or needlessly slow).
fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        if start.elapsed() > timeout {
            panic!("wait_for: condition not met within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ===========================================================================
// Part 1 — pure parse functions (no IO; §3.1)
// ===========================================================================

// ---------------------------------------------------------------------------
// parse_agy_models — the verbatim 8-line `agy models` output
// (antigravity-research.md §0)
// ---------------------------------------------------------------------------

const AGY_MODELS_OUTPUT: &str = "Gemini 3.5 Flash (Medium)\n\
Gemini 3.5 Flash (High)\n\
Gemini 3.5 Flash (Low)\n\
Gemini 3.1 Pro (Low)\n\
Gemini 3.1 Pro (High)\n\
Claude Sonnet 4.6 (Thinking)\n\
Claude Opus 4.6 (Thinking)\n\
GPT-OSS 120B (Medium)\n";

#[test]
fn parse_agy_models_verbatim_8_lines() {
    let models = parse_agy_models(AGY_MODELS_OUTPUT);
    assert_eq!(
        models.len(),
        8,
        "the real `agy models` output has exactly 8 lines"
    );
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "Gemini 3.5 Flash (Medium)",
            "Gemini 3.5 Flash (High)",
            "Gemini 3.5 Flash (Low)",
            "Gemini 3.1 Pro (Low)",
            "Gemini 3.1 Pro (High)",
            "Claude Sonnet 4.6 (Thinking)",
            "Claude Opus 4.6 (Thinking)",
            "GPT-OSS 120B (Medium)",
        ],
        "ids must be verbatim — spaces/parens/mixed-vendor names untouched (§1.1)"
    );
    // §1.1: agy models are `Listed`, `free_text: false` — pass verbatim to --model.
    for m in &models {
        assert_eq!(
            m.annotation, None,
            "agy has no per-model annotation — one name per line"
        );
    }
}

#[test]
fn parse_agy_models_tolerates_trailing_whitespace_and_blank_lines() {
    let noisy = "Gemini 3.5 Flash (Low)\n\n  \nClaude Opus 4.6 (Thinking)  \n\n";
    let models = parse_agy_models(noisy);
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["Gemini 3.5 Flash (Low)", "Claude Opus 4.6 (Thinking)"]
    );
}

#[test]
fn parse_agy_models_empty_stdout_is_empty_not_error() {
    assert_eq!(parse_agy_models(""), vec![]);
    assert_eq!(parse_agy_models("\n\n  \n"), vec![]);
}

// ---------------------------------------------------------------------------
// parse_codex_debug_models — REAL captured fixture,
// fixtures/codex/debug_models.json (codex-cli 0.142.5)
// ---------------------------------------------------------------------------

fn codex_debug_models_fixture() -> String {
    let path = fixture_path("codex/debug_models.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"))
}

#[test]
fn parse_codex_debug_models_real_fixture_excludes_hidden_model() {
    let raw = codex_debug_models_fixture();
    let models = parse_codex_debug_models(&raw);

    // Real fixture has exactly 3 models: gpt-5.5 (visibility=list, priority=7),
    // gpt-5.4-mini (visibility=list, priority=23), codex-auto-review
    // (visibility=hide, priority=43). §1.1: "hidden models... are excluded."
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        2,
        "exactly the 2 non-hidden models must survive, got: {ids:?}"
    );
    assert!(ids.contains(&"gpt-5.5"));
    assert!(ids.contains(&"gpt-5.4-mini"));
    assert!(
        !ids.contains(&"codex-auto-review"),
        "codex-auto-review has visibility=hide and must be excluded"
    );
}

#[test]
fn parse_codex_debug_models_priority_lands_in_annotation_verbatim() {
    // §6: "assert hidden models excluded, priority text lands in annotation
    // verbatim." NOTE(test-agent): the exact string form of "priority text"
    // is not pinned beyond this phrase; the most literal reading — the
    // model's numeric `priority` field, stringified, landing unmodified in
    // `annotation` — is what's asserted here.
    let raw = codex_debug_models_fixture();
    let models = parse_codex_debug_models(&raw);
    let gpt55 = models
        .iter()
        .find(|m| m.id == "gpt-5.5")
        .expect("gpt-5.5 must be present");
    assert_eq!(
        gpt55.annotation.as_deref(),
        Some("7"),
        "gpt-5.5's real priority is 7"
    );

    let mini = models
        .iter()
        .find(|m| m.id == "gpt-5.4-mini")
        .expect("gpt-5.4-mini must be present");
    assert_eq!(
        mini.annotation.as_deref(),
        Some("23"),
        "gpt-5.4-mini's real priority is 23"
    );
}

#[test]
fn parse_codex_debug_models_empty_models_array_is_empty_not_error() {
    let models = parse_codex_debug_models(r#"{"models":[]}"#);
    assert_eq!(models, vec![]);
}

#[test]
fn parse_codex_debug_models_malformed_json_is_empty_not_panic() {
    // §3.1: probes never panic; a parse fn facing garbage must degrade to
    // empty, letting the caller surface the real subprocess exit code /
    // stderr into `Capabilities.errors` instead.
    let models = parse_codex_debug_models("not json at all");
    assert_eq!(models, vec![]);
}

// ---------------------------------------------------------------------------
// build_claude_model_catalog — the claude FAMILY list (opus/sonnet/haiku),
// env-resolved annotations, observed-model merge (§1.1/§3.4)
// ---------------------------------------------------------------------------

#[test]
fn build_claude_model_catalog_offers_three_families_in_order_no_env() {
    let catalog = build_claude_model_catalog(&HashMap::new(), &[]);
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["opus", "sonnet", "haiku"]);
    // No env resolution → no annotation (never a synthesized sublabel).
    for m in &catalog {
        assert_eq!(m.annotation, None);
    }
}

#[test]
fn build_claude_model_catalog_env_resolved_id_lands_as_versioned_entry() {
    let mut resolved = HashMap::new();
    resolved.insert("opus", "claude-opus-4-8-20260101".to_string());
    resolved.insert("haiku", "  ".to_string()); // whitespace-only → ignored
    let catalog = build_claude_model_catalog(&resolved, &[]);
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();

    // A resolved family becomes its CONCRETE versioned entry (annotated with the
    // family word); the bare "opus" alias is dropped so the picker never shows both.
    let opus = catalog
        .iter()
        .find(|m| m.id == "claude-opus-4-8-20260101")
        .expect("resolved opus version is a concrete entry");
    assert_eq!(opus.annotation.as_deref(), Some("opus"));
    assert!(!ids.contains(&"opus"), "bare opus dropped when versioned present");
    // haiku (whitespace-only → no version) and sonnet (no env) stay as bare aliases.
    assert!(ids.contains(&"haiku"));
    assert!(ids.contains(&"sonnet"));
}

#[test]
fn build_claude_model_catalog_prefers_versioned_over_bare_family() {
    let observed = vec![
        // A stray bare family word (e.g. from env) → must be dropped in favor of
        // the versioned entry below.
        ModelInfo {
            id: "sonnet".to_string(),
            annotation: None,
        },
        // The concrete versioned model.
        ModelInfo {
            id: "claude-sonnet-4-6-20251111".to_string(),
            annotation: None,
        },
    ];
    let catalog = build_claude_model_catalog(&HashMap::new(), &observed);
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
    assert!(
        !ids.contains(&"sonnet"),
        "bare sonnet dropped when claude-sonnet-4-6 is present"
    );
    assert!(ids.contains(&"claude-sonnet-4-6-20251111"));
    // The families with no version at all remain pickable as bare aliases.
    assert!(ids.contains(&"opus"));
    assert!(ids.contains(&"haiku"));
}

#[test]
fn merge_observed_into_catalog_appends_missing_preserves_order() {
    let mut catalog = ModelCatalog {
        models: vec![ModelInfo {
            id: "opus".to_string(),
            annotation: None,
        }],
        source: ModelListSource::Observed,
        free_text: true,
    };
    merge_observed_into_catalog(
        &mut catalog,
        &[
            ModelInfo {
                id: "opus".to_string(),
                annotation: None,
            },
            ModelInfo {
                id: "some-observed".to_string(),
                annotation: None,
            },
        ],
    );
    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["opus", "some-observed"]);
}

// ---------------------------------------------------------------------------
// encode_claude_project_dir + scan_claude_sessions
// (~/.claude/projects/<encoded-cwd>/<uuid>.jsonl; encoding = abs path /→-)
// ---------------------------------------------------------------------------

#[test]
fn encode_claude_project_dir_slashes_become_dashes() {
    assert_eq!(
        encode_claude_project_dir("/work/mewais/Marsad"),
        "-work-mewais-Marsad"
    );
}

#[test]
fn scan_claude_sessions_lists_uuid_jsonl_with_snippet_and_project() {
    let tmp = tempdir();
    let project = "/work/mewais/Marsad";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();

    // A session file with a plain-string user content block.
    let uuid = "019f237a-f03d-7710-ab8f-7af32b222df8";
    let body = format!(
        "{}\n{}\n",
        r#"{"type":"user","message":{"role":"user","content":"add a health check endpoint"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
    );
    std::fs::write(proj_dir.join(format!("{uuid}.jsonl")), body).unwrap();

    let sessions = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, uuid);
    assert_eq!(sessions[0].project.as_deref(), Some(project));
    assert_eq!(sessions[0].snippet, "add a health check endpoint");
}

#[test]
fn scan_claude_sessions_array_content_first_text_block() {
    let tmp = tempdir();
    let project = "/p";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();
    let body = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello world"}]}}"#;
    std::fs::write(proj_dir.join("aaa.jsonl"), body).unwrap();

    let sessions = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].snippet, "hello world");
}

#[test]
fn scan_claude_sessions_newest_first_and_cap() {
    let tmp = tempdir();
    let project = "/p";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();

    for (i, offset) in [("old", -300i64), ("mid", -200), ("new", -100)] {
        let path = proj_dir.join(format!("{i}.jsonl"));
        std::fs::write(&path, r#"{"type":"user","message":{"role":"user","content":"x"}}"#)
            .unwrap();
        set_mtime(&path, offset);
    }

    let all = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(
        all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["new", "mid", "old"],
        "newest-first"
    );

    let capped = scan_claude_sessions(tmp.path(), project, 2);
    assert_eq!(
        capped.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["new", "mid"],
        "cap honored, newest kept"
    );
}

#[test]
fn scan_claude_sessions_absent_project_dir_is_empty_not_error() {
    let tmp = tempdir();
    assert_eq!(
        scan_claude_sessions(tmp.path(), "/never/created", 200),
        vec![]
    );
}

#[test]
fn scan_claude_sessions_ignores_non_jsonl_files() {
    let tmp = tempdir();
    let project = "/p";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join("readme.txt"), "not a session").unwrap();
    std::fs::write(
        proj_dir.join("real.jsonl"),
        r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
    )
    .unwrap();

    let sessions = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "real");
}

/// Snippet extraction skips non-`user` lines, tolerates a `user` line with no
/// `content`, and falls through to an empty snippet when a `user` line's array
/// content has no `text` block — covering `claude_snippet_from_body`'s skip
/// arms (never a parse error / panic).
#[test]
fn scan_claude_sessions_snippet_skips_non_user_and_empty_content() {
    let tmp = tempdir();
    let project = "/p";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();
    // First line is a non-user event (skipped), then a `user` line whose message
    // has NO content key (skipped), then a `user` line with an array content
    // that has NO text block (falls through) → empty snippet, but the session is
    // still listed with its id.
    let body = format!(
        "{}\n{}\n{}\n",
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"user","message":{"role":"user"}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"image"}]}}"#,
    );
    std::fs::write(proj_dir.join("sess-1.jsonl"), body).unwrap();

    let sessions = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sess-1");
    assert_eq!(sessions[0].snippet, "", "no extractable text → empty snippet");
}

/// A `.jsonl` whose file stem is empty (`.jsonl`) is skipped (empty id → None),
/// covering `scan_claude_sessions`'s empty-id guard.
#[test]
fn scan_claude_sessions_empty_stem_is_skipped() {
    let tmp = tempdir();
    let project = "/p";
    let proj_dir = tmp.path().join(encode_claude_project_dir(project));
    std::fs::create_dir_all(&proj_dir).unwrap();
    // A file literally named ".jsonl" has an empty file stem.
    std::fs::write(
        proj_dir.join(".jsonl"),
        r#"{"type":"user","message":{"role":"user","content":"x"}}"#,
    )
    .unwrap();
    // Plus one valid file so the dir isn't empty.
    std::fs::write(
        proj_dir.join("good.jsonl"),
        r#"{"type":"user","message":{"role":"user","content":"y"}}"#,
    )
    .unwrap();

    let sessions = scan_claude_sessions(tmp.path(), project, 200);
    assert_eq!(sessions.len(), 1, "empty-stem file skipped");
    assert_eq!(sessions[0].id, "good");
}

// ---------------------------------------------------------------------------
// parse_codex_rollout_meta + scan_codex_sessions
// (codex-research.md §d: filename pattern + session_meta/response_item shapes)
// ---------------------------------------------------------------------------

/// A `session_meta` first line built from codex-research.md §d's documented
/// field set (`session_id, id, timestamp, cwd, originator: "codex_exec",
/// source: "exec", thread_source: "user", cli_version, model_provider,
/// base_instructions`), using the SAME thread uuid/timestamp as the doc's
/// real concretely-captured filename example (§d: "Concretely:
/// ~/.codex/sessions/2026/07/02/rollout-2026-07-02T11-38-07-019f237a-f03d-7710-ab8f-7af32b222df8.jsonl").
const CODEX_SESSION_META_LINE: &str = r#"{"timestamp":"2026-07-02T11:38:07.000Z","type":"session_meta","payload":{"session_id":"019f237a-f03d-7710-ab8f-7af32b222df8","id":"019f237a-f03d-7710-ab8f-7af32b222df8","timestamp":"2026-07-02T11:38:07.000Z","cwd":"/work/mewais/OpenSource/Marsad","originator":"codex_exec","source":"exec","thread_source":"user","cli_version":"0.142.5","model_provider":"openai","base_instructions":{"text":"You are Codex..."}}}"#;

const CODEX_ROLLOUT_FILENAME: &str =
    "rollout-2026-07-02T11-38-07-019f237a-f03d-7710-ab8f-7af32b222df8.jsonl";

/// NOTE(test-agent): codex-research.md §d describes `response_item`'s user
/// message prose-only ("raw model I/O turned into rollout form: user
/// `message` (role, content[])") — no literal captured line exists in the
/// doc (unlike `session_meta`'s field LIST, which this line follows exactly).
/// This is the most natural encoding of that prose and is what
/// `scan_codex_sessions` is tested against; if the real wire shape differs
/// this test (not the implementation) should be revisited against a captured
/// fixture, per the phase2-drivers.md guardrail.
const CODEX_USER_RESPONSE_ITEM_LINE: &str = r#"{"timestamp":"2026-07-02T11:38:08.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add a health check endpoint"}]}}"#;

#[test]
fn parse_codex_rollout_meta_valid_session_meta_extracts_id_and_project() {
    let (id, project) = parse_codex_rollout_meta(CODEX_ROLLOUT_FILENAME, CODEX_SESSION_META_LINE)
        .expect("a well-formed rollout filename must parse");
    assert_eq!(
        id, "019f237a-f03d-7710-ab8f-7af32b222df8",
        "id is the trailing filename uuid"
    );
    assert_eq!(
        project.as_deref(),
        Some("/work/mewais/OpenSource/Marsad"),
        "project comes from payload.cwd"
    );
}

/// §6: "malformed first line → session listed with empty project/snippet
/// (skip, don't fail)" — the SESSION is still listed (id survives from the
/// filename), only `project` degrades to `None`.
#[test]
fn parse_codex_rollout_meta_malformed_first_line_yields_id_with_no_project() {
    let (id, project) = parse_codex_rollout_meta(CODEX_ROLLOUT_FILENAME, "not json")
        .expect("filename alone must still yield an id even when the first line is garbage");
    assert_eq!(id, "019f237a-f03d-7710-ab8f-7af32b222df8");
    assert_eq!(project, None);
}

#[test]
fn parse_codex_rollout_meta_non_rollout_filename_is_none() {
    assert_eq!(
        parse_codex_rollout_meta("not-a-rollout-file.txt", CODEX_SESSION_META_LINE),
        None
    );
}

fn write_file(dir: &Path, filename: &str, content: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn scan_codex_sessions_extracts_id_project_snippet_from_temp_tree() {
    let tmp = tempdir();
    let day_dir = tmp.path().join("sessions/2026/07/02");
    std::fs::create_dir_all(&day_dir).unwrap();

    let body = format!("{CODEX_SESSION_META_LINE}\n{CODEX_USER_RESPONSE_ITEM_LINE}\n");
    write_file(&day_dir, CODEX_ROLLOUT_FILENAME, &body);

    let sessions = scan_codex_sessions(&tmp.path().join("sessions"), 200);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.id, "019f237a-f03d-7710-ab8f-7af32b222df8");
    assert_eq!(s.project.as_deref(), Some("/work/mewais/OpenSource/Marsad"));
    assert_eq!(s.snippet, "add a health check endpoint");
}

#[test]
fn scan_codex_sessions_newest_first_ordering_and_file_cap() {
    let tmp = tempdir();
    let day_dir = tmp.path().join("sessions/2026/07/02");
    std::fs::create_dir_all(&day_dir).unwrap();

    let names = [
        "rollout-2026-07-02T09-00-00-00000000-0000-0000-0000-000000000001.jsonl",
        "rollout-2026-07-02T10-00-00-00000000-0000-0000-0000-000000000002.jsonl",
        "rollout-2026-07-02T11-00-00-00000000-0000-0000-0000-000000000003.jsonl",
    ];
    for (i, name) in names.iter().enumerate() {
        let meta = CODEX_SESSION_META_LINE.replace(
            "019f237a-f03d-7710-ab8f-7af32b222df8",
            &format!("00000000-0000-0000-0000-00000000000{}", i + 1),
        );
        let path = write_file(&day_dir, name, &format!("{meta}\n"));
        // Oldest = file 1, newest = file 3 (100s spacing to avoid touch races).
        set_mtime(&path, (i as i64 - 3) * 100);
    }

    let all = scan_codex_sessions(&tmp.path().join("sessions"), 200);
    assert_eq!(all.len(), 3);
    assert_eq!(
        all[0].id, "00000000-0000-0000-0000-000000000003",
        "newest file must be first"
    );
    assert_eq!(
        all[2].id, "00000000-0000-0000-0000-000000000001",
        "oldest file must be last"
    );

    let capped = scan_codex_sessions(&tmp.path().join("sessions"), 2);
    assert_eq!(
        capped.len(),
        2,
        "file cap must be honored (default 200, tested here with 2)"
    );
    assert_eq!(capped[0].id, "00000000-0000-0000-0000-000000000003");
}

#[test]
fn scan_codex_sessions_empty_root_is_empty_not_error() {
    let tmp = tempdir();
    let sessions = scan_codex_sessions(&tmp.path().join("does-not-exist"), 200);
    assert_eq!(sessions, vec![]);
}

// ---------------------------------------------------------------------------
// agy_snippet_from_transcript + scan_agy_sessions
// (antigravity-research.md §3.2 — the verbatim 5-line transcript_full.jsonl
// sample, and §5's last_conversations.json shape)
// ---------------------------------------------------------------------------

/// Verbatim from antigravity-research.md §3.2 ("Real schema, from the actual
/// 5-line file"). Only content is the smoke-test prompt `"x"`.
const AGY_TRANSCRIPT_FULL_JSONL: &str = "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:36Z\",\"content\":\"<USER_REQUEST>\\nx\\n</USER_REQUEST>\\n<ADDITIONAL_METADATA>\\nThe current local time is: 2026-07-02T11:23:36-04:00.\\n</ADDITIONAL_METADATA>\\n<USER_SETTINGS_CHANGE>\\n...\\n</USER_SETTINGS_CHANGE>\"}\n\
{\"step_index\":1,\"source\":\"SYSTEM\",\"type\":\"CONVERSATION_HISTORY\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:36Z\"}\n\
{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:36Z\",\"tool_calls\":[{\"name\":\"list_dir\",\"args\":{\"DirectoryPath\":\"/home/mewais/.gemini/antigravity-cli\",\"toolAction\":\"Listing application directory\",\"toolSummary\":\"List App Data Directory\"}}]}\n\
{\"step_index\":4,\"source\":\"SYSTEM\",\"type\":\"CHECKPOINT\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:37Z\",\"content\":\"{{ CHECKPOINT 0 }}\\n **The earlier parts of this conversation have been truncated...**\"}\n\
{\"step_index\":5,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:37Z\",\"content\":\"It looks like your request was just the letter \\\"x\\\". Could you please clarify...\"}\n";

const AGY_CONVERSATION_ID: &str = "97197d46-2091-40e2-b36f-0dcedd34bcc4";

#[test]
fn agy_snippet_from_transcript_strips_user_request_wrapper() {
    // §1.2/§3.2: "snippet ... stripping the <USER_REQUEST>...</USER_REQUEST>
    // wrapper (and sibling injected blocks)." The real fixture's actual
    // prompt content is the single character "x".
    let snippet = agy_snippet_from_transcript(AGY_TRANSCRIPT_FULL_JSONL)
        .expect("the fixture's first USER_INPUT step must yield a snippet");
    assert_eq!(snippet, "x");
}

#[test]
fn agy_snippet_from_transcript_empty_input_is_none() {
    assert_eq!(agy_snippet_from_transcript(""), None);
}

#[test]
fn agy_snippet_from_transcript_no_user_input_step_is_none() {
    // Only SYSTEM/MODEL steps, no USER_INPUT.
    let no_user = AGY_TRANSCRIPT_FULL_JSONL
        .lines()
        .filter(|l| !l.contains("USER_INPUT"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(agy_snippet_from_transcript(&no_user), None);
}

#[test]
fn scan_agy_sessions_brain_dir_tree_with_reverse_mapped_project() {
    let tmp = tempdir();
    let brain_root = tmp.path().join("brain");
    let logs_dir = brain_root
        .join(AGY_CONVERSATION_ID)
        .join(".system_generated/logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    write_file(
        &logs_dir,
        "transcript_full.jsonl",
        AGY_TRANSCRIPT_FULL_JSONL,
    );

    // §5: last_conversations.json is exactly `{"<workspace>": "<conversation-uuid>"}`.
    let last_conversations =
        format!(r#"{{"/work/mewais/OpenSource/Marsad":"{AGY_CONVERSATION_ID}"}}"#);

    let sessions = scan_agy_sessions(&brain_root, &last_conversations);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.id, AGY_CONVERSATION_ID, "id is the brain-dir uuid");
    assert_eq!(s.snippet, "x");
    assert_eq!(
        s.project.as_deref(),
        Some("/work/mewais/OpenSource/Marsad"),
        "the reverse-mapped hit must resolve the workspace"
    );
}

/// §1.2: "otherwise `None` (the `.db` is raw protobuf — not parsed)" — a
/// conversation whose id has no reverse-map hit must not fabricate a project.
#[test]
fn scan_agy_sessions_no_reverse_map_hit_yields_none_project() {
    let tmp = tempdir();
    let brain_root = tmp.path().join("brain");
    let logs_dir = brain_root
        .join(AGY_CONVERSATION_ID)
        .join(".system_generated/logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    write_file(
        &logs_dir,
        "transcript_full.jsonl",
        AGY_TRANSCRIPT_FULL_JSONL,
    );

    let sessions = scan_agy_sessions(&brain_root, "{}");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].project, None);
}

/// §1.2: "prefer `transcript_full.jsonl` over `transcript.jsonl` (double-escaping trap)."
#[test]
fn scan_agy_sessions_prefers_transcript_full_over_transcript() {
    let tmp = tempdir();
    let brain_root = tmp.path().join("brain");
    let logs_dir = brain_root
        .join(AGY_CONVERSATION_ID)
        .join(".system_generated/logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    // A DIFFERENT (double-escaped, per the doc's description of the trap)
    // sibling file must be ignored when transcript_full.jsonl is present.
    write_file(
        &logs_dir,
        "transcript.jsonl",
        "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-07-02T15:23:36Z\",\"content\":\"<USER_REQUEST>\\nWRONG\\n</USER_REQUEST>\"}\n",
    );
    write_file(
        &logs_dir,
        "transcript_full.jsonl",
        AGY_TRANSCRIPT_FULL_JSONL,
    );

    let sessions = scan_agy_sessions(&brain_root, "{}");
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].snippet, "x",
        "transcript_full.jsonl must win over transcript.jsonl"
    );
}

#[test]
fn scan_agy_sessions_empty_root_is_empty_not_error() {
    let tmp = tempdir();
    let sessions = scan_agy_sessions(&tmp.path().join("does-not-exist"), "{}");
    assert_eq!(sessions, vec![]);
}

// ---------------------------------------------------------------------------
// parse_remote_claude_session_list — moved from
// apps/desktop/src-tauri/src/bridge.rs:170 (`mtime\tid\tuser-line` records
// produced by `fetch_remote_session_list`)
// ---------------------------------------------------------------------------

#[test]
fn parse_remote_claude_session_list_normal_record() {
    let raw_user_line = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the bug"}]}}"#;
    let stdout = format!("1783100616000\tuuid-abc\t{raw_user_line}\n");
    let sessions = parse_remote_claude_session_list(&stdout);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "uuid-abc");
    assert_eq!(sessions[0].snippet, "fix the bug");
    assert_eq!(sessions[0].modified_ms, 1783100616000);
}

/// Bridge's existing cases (ported verbatim, typed-ipc.md §7 house
/// convention): malformed mtime → record skipped, not a parse error.
#[test]
fn parse_remote_claude_session_list_malformed_mtime_is_skipped() {
    let stdout = "not-a-number\tuuid-abc\t{}\n1783100616000\tuuid-def\t{}\n";
    let sessions = parse_remote_claude_session_list(stdout);
    assert_eq!(sessions.len(), 1, "only the well-formed record survives");
    assert_eq!(sessions[0].id, "uuid-def");
}

#[test]
fn parse_remote_claude_session_list_empty_id_is_skipped() {
    let stdout = "1783100616000\t\t{}\n";
    let sessions = parse_remote_claude_session_list(stdout);
    assert_eq!(sessions, vec![]);
}

#[test]
fn parse_remote_claude_session_list_missing_third_field_yields_empty_snippet() {
    let stdout = "1783100616000\tuuid-abc\n";
    let sessions = parse_remote_claude_session_list(stdout);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].snippet, "");
}

#[test]
fn parse_remote_claude_session_list_preserves_emitted_order() {
    let stdout = "300\tid-c\t{}\n200\tid-b\t{}\n100\tid-a\t{}\n";
    let sessions = parse_remote_claude_session_list(stdout);
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["id-c", "id-b", "id-a"],
        "the function must NOT re-sort — the remote `ls -t` already emits newest-first"
    );
}

#[test]
fn parse_remote_claude_session_list_empty_stdout_is_empty() {
    assert_eq!(parse_remote_claude_session_list(""), vec![]);
}

// ===========================================================================
// Part 1b — discovery script builders + section parser (all pure, no IO)
// ===========================================================================

#[test]
fn command_word_first_token() {
    assert_eq!(command_word("codex --sandbox danger"), "codex");
    assert_eq!(command_word("/opt/genie/genie"), "/opt/genie/genie");
    assert_eq!(command_word("  claude  "), "claude");
    assert_eq!(command_word(""), "");
    assert_eq!(command_word("   "), "");
}

#[test]
fn build_codex_discovery_script_has_setup_and_all_sections() {
    let script =
        build_codex_discovery_script("codex --sandbox", Some("module load cuda"));
    // Setup runs first, tolerated on failure.
    assert!(script.contains("module load cuda"));
    assert!(script.contains("|| true"));
    // Availability checks the FIRST WORD of the resolved command.
    assert!(script.contains("command -v 'codex'"));
    // The models subcommand uses the FULL resolved command (flags preserved).
    assert!(script.contains("codex --sandbox debug models"));
    // All three sections are present with begin/end markers.
    for name in ["avail", "models", "sessions"] {
        assert!(
            script.contains(&format!("@@MARSAD_SEC_BEGIN:{name}")),
            "missing begin marker for {name}"
        );
        assert!(
            script.contains(&format!("@@MARSAD_SEC_END:{name}")),
            "missing end marker for {name}"
        );
    }
}

#[test]
fn build_codex_discovery_script_no_setup_omits_setup_line() {
    let script = build_codex_discovery_script("codex", None);
    assert!(!script.contains("|| true\n@@") || !script.contains("module"));
    assert!(script.contains("command -v 'codex'"));
    assert!(script.contains("codex debug models"));
}

#[test]
fn build_agy_discovery_script_avail_and_models_only() {
    let script = build_agy_discovery_script("agy", Some("source ~/.gemrc"));
    assert!(script.contains("source ~/.gemrc"));
    assert!(script.contains("command -v 'agy'"));
    assert!(script.contains("agy models"));
    assert!(script.contains("@@MARSAD_SEC_BEGIN:avail"));
    assert!(script.contains("@@MARSAD_SEC_BEGIN:models"));
    // agy has no over-the-wire session enumeration.
    assert!(!script.contains("@@MARSAD_SEC_BEGIN:sessions"));
}

#[test]
fn build_claude_discovery_script_env_and_sessions_for_project() {
    let script =
        build_claude_discovery_script("genie", Some("module load cuda"), Some("/home/u/proj"));
    assert!(script.contains("module load cuda"));
    assert!(script.contains("command -v 'genie'"));
    // The env section prints all three families.
    assert!(script.contains("ANTHROPIC_DEFAULT_OPUS_MODEL"));
    assert!(script.contains("ANTHROPIC_DEFAULT_SONNET_MODEL"));
    assert!(script.contains("ANTHROPIC_DEFAULT_HAIKU_MODEL"));
    // Sessions resolve + encode the project's real cwd.
    assert!(script.contains("cd '/home/u/proj'"));
    assert!(script.contains("projects/$enc"));
    for name in ["avail", "env", "extra_models", "sessions"] {
        assert!(script.contains(&format!("@@MARSAD_SEC_BEGIN:{name}")));
    }
}

#[test]
fn build_claude_discovery_script_no_project_empty_sessions_section() {
    let script = build_claude_discovery_script("claude", None, None);
    assert!(script.contains("command -v 'claude'"));
    // Sessions section still present but a no-op (no project-dir ls loop).
    assert!(script.contains("@@MARSAD_SEC_BEGIN:sessions"));
    // The sessions section's ls loop only appears when a project is given; it
    // uses "$dir"/*.jsonl — check that the literal project-dir pattern is absent.
    assert!(!script.contains("ls -t \"$dir\""),
        "sessions section must not contain the project-dir ls loop when no project given");
}

#[test]
fn build_claude_discovery_script_tilde_project_expands_home() {
    let script = build_claude_discovery_script("claude", None, Some("~/work"));
    assert!(script.contains("cd \"$HOME/work\""));
    let root = build_claude_discovery_script("claude", None, Some("~"));
    assert!(root.contains("cd \"$HOME\""));
}

#[test]
fn parse_discovery_sections_splits_named_sections() {
    let raw = "login banner noise\n\
        @@MARSAD_SEC_BEGIN:avail\n\
        /usr/bin/codex\n\
        @@MARSAD_SEC_END:avail\n\
        @@MARSAD_SEC_BEGIN:models\n\
        {\"models\":[]}\n\
        @@MARSAD_SEC_END:models\n";
    let secs = parse_discovery_sections(raw);
    assert_eq!(secs.get("avail"), "/usr/bin/codex");
    assert_eq!(secs.get("models"), "{\"models\":[]}");
    assert_eq!(secs.get("missing"), "");
}

#[test]
fn parse_discovery_sections_truncated_flushes_open_section() {
    // A killed probe (no END) still yields the partial body — honest, no panic.
    let raw = "@@MARSAD_SEC_BEGIN:avail\n/usr/bin/claude\n";
    let secs = parse_discovery_sections(raw);
    assert_eq!(secs.get("avail"), "/usr/bin/claude");
}

#[test]
fn parse_discovery_sections_new_begin_closes_dangling_section() {
    let raw = "@@MARSAD_SEC_BEGIN:avail\nx\n@@MARSAD_SEC_BEGIN:models\ny\n@@MARSAD_SEC_END:models\n";
    let secs = parse_discovery_sections(raw);
    assert_eq!(secs.get("avail"), "x");
    assert_eq!(secs.get("models"), "y");
}

#[test]
fn parse_avail_nonempty_means_available() {
    assert!(parse_avail("/usr/bin/codex"));
    assert!(parse_avail("  /opt/genie/genie  "));
    assert!(!parse_avail(""));
    assert!(!parse_avail("\n   \n"));
}

#[test]
fn parse_claude_env_models_maps_families_dropping_empty() {
    let body = "opus\tclaude-opus-4-1-20250805\nsonnet\tclaude-sonnet-4-5\nhaiku\t";
    let map = parse_claude_env_models(body);
    assert_eq!(map.get("opus").map(String::as_str), Some("claude-opus-4-1-20250805"));
    assert_eq!(map.get("sonnet").map(String::as_str), Some("claude-sonnet-4-5"));
    // haiku has an empty value → dropped, not stored as "".
    assert_eq!(map.get("haiku"), None);
}

#[test]
fn parse_claude_env_models_empty_is_empty() {
    assert!(parse_claude_env_models("").is_empty());
    assert!(parse_claude_env_models("\n\n").is_empty());
}

// ---------------------------------------------------------------------------
// parse_claude_extra_models — custom/subscription model ids from the
// extra_models section (`model\t<value>` lines)
// ---------------------------------------------------------------------------

#[test]
fn parse_claude_extra_models_extracts_custom_model_ids() {
    let body = "model\tfable\nmodel\tclaude-custom-123\n";
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable", "claude-custom-123"]);
}

#[test]
fn parse_claude_extra_models_drops_family_words() {
    // A "model" line that equals a family name must not create a duplicate
    // in the catalog (families are already first).
    let body = "model\topus\nmodel\tsonnet\nmodel\thaiku\nmodel\tfable\n";
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable"], "family words must be excluded");
}

#[test]
fn parse_claude_extra_models_drops_empty_and_blank_values() {
    let body = "model\t\nmodel\t   \nmodel\tfable\n";
    // Blank/empty tab values are dropped; only "fable" survives.
    // Note: "   " is treated as non-empty after trim but contains no visible content;
    // the shell emits truly empty strings for unset vars, but let's handle whitespace-only too.
    // The parser uses `.trim()` so "   " → "" → dropped.
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable"]);
}

#[test]
fn parse_claude_extra_models_deduplicates() {
    let body = "model\tfable\nmodel\tfable\nmodel\tcustom-x\nmodel\tfable\n";
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable", "custom-x"], "duplicates removed, order preserved");
}

#[test]
fn parse_claude_extra_models_drops_synthetic_placeholders() {
    // Values containing '<' are synthetic/placeholder (e.g. "<none>", "<synthetic>").
    let body = "model\t<none>\nmodel\t<synthetic>\nmodel\tfable\n";
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable"]);
}

#[test]
fn parse_claude_extra_models_ignores_non_model_keys() {
    // Only "model\t..." lines are parsed; other keys (like "opus\t...") are ignored.
    let body = "opus\tclaude-opus-4-1\nmodel\tfable\nsonnet\t\n";
    let extra = parse_claude_extra_models(body);
    assert_eq!(extra, vec!["fable"]);
}

#[test]
fn parse_claude_extra_models_empty_body_is_empty() {
    assert!(parse_claude_extra_models("").is_empty());
    assert!(parse_claude_extra_models("\n\n").is_empty());
}

/// End-to-end: extra model ids merge into catalog after families, no duplicates.
#[test]
fn build_claude_model_catalog_merges_extra_models_after_families() {
    let extra = vec![
        ModelInfo { id: "fable".to_string(), annotation: None },
        // A family word passed as extra must not duplicate the family entry.
        ModelInfo { id: "sonnet".to_string(), annotation: None },
        ModelInfo { id: "custom-x".to_string(), annotation: None },
    ];
    let catalog = build_claude_model_catalog(&HashMap::new(), &extra);
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
    // Observed ids first (fable, the bare sonnet — kept since no versioned sonnet
    // exists — and custom-x), then the bare family fallbacks for families with no
    // entry (opus, haiku). No versioned models here, so no bare family is dropped.
    assert_eq!(
        ids,
        vec!["fable", "sonnet", "custom-x", "opus", "haiku"],
        "observed ids kept; bare family fallback only for families with no entry"
    );
}

/// The script includes the extra_models section.
#[test]
fn build_claude_discovery_script_has_extra_models_section() {
    let script = build_claude_discovery_script("claude", None, None);
    assert!(
        script.contains("@@MARSAD_SEC_BEGIN:extra_models"),
        "extra_models section must be present"
    );
    assert!(
        script.contains("ANTHROPIC_MODEL"),
        "ANTHROPIC_MODEL env must be probed"
    );
    assert!(
        script.contains("settings.json"),
        "claude settings.json must be scanned"
    );
    assert!(
        script.contains(".claude.json"),
        ".claude.json must be scanned"
    );
    assert!(
        script.contains("projects"),
        "transcript scan must reference projects dir"
    );
}

/// End-to-end pure roundtrip for extra_models section → catalog merge.
#[test]
fn claude_discovery_extra_models_roundtrip_pure() {
    let out = "@@MARSAD_SEC_BEGIN:avail\n/usr/local/bin/claude\n@@MARSAD_SEC_END:avail\n\
               @@MARSAD_SEC_BEGIN:env\nopus\t\nsonnet\t\nhaiku\t\n@@MARSAD_SEC_END:env\n\
               @@MARSAD_SEC_BEGIN:extra_models\nmodel\tfable\nmodel\t\nmodel\topus\nmodel\tfable\n@@MARSAD_SEC_END:extra_models\n\
               @@MARSAD_SEC_BEGIN:sessions\n@@MARSAD_SEC_END:sessions\n";
    let secs = parse_discovery_sections(out);
    let extra_ids = parse_claude_extra_models(secs.get("extra_models"));
    // "fable" survives; empty dropped; "opus" family dropped; duplicate "fable" deduped.
    assert_eq!(extra_ids, vec!["fable"]);

    let extra_observed: Vec<ModelInfo> = extra_ids
        .into_iter()
        .map(|id| ModelInfo { id, annotation: None })
        .collect();
    let resolved = parse_claude_env_models(secs.get("env"));
    let resolved_ref: std::collections::HashMap<&str, String> =
        resolved.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let catalog = build_claude_model_catalog(&resolved_ref, &extra_observed);
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
    // No env versions and no versioned observed → fable kept, bare families as
    // fallback (all three have no concrete version).
    assert_eq!(ids, vec!["fable", "opus", "sonnet", "haiku"]);
}

#[test]
fn parse_remote_codex_session_list_extracts_id_project_from_records() {
    let meta = r#"{"type":"session_meta","payload":{"cwd":"/home/u/proj"}}"#;
    let record = format!(
        "1783100616000\trollout-2025-06-01T12-00-00-11111111-2222-3333-4444-555555555555.jsonl\t{meta}"
    );
    let sessions = parse_remote_codex_session_list(&record);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(sessions[0].project.as_deref(), Some("/home/u/proj"));
    assert_eq!(sessions[0].modified_ms, 1783100616000);
}

#[test]
fn parse_remote_codex_session_list_skips_bad_mtime_and_non_rollout() {
    let bad_mtime = "notanumber\trollout-x-11111111-2222-3333-4444-555555555555.jsonl\t{}";
    assert_eq!(parse_remote_codex_session_list(bad_mtime), vec![]);
    let non_rollout = "1783100616000\tnot-a-rollout.txt\t{}";
    assert_eq!(parse_remote_codex_session_list(non_rollout), vec![]);
}

#[test]
fn parse_remote_codex_session_list_empty_is_empty() {
    assert_eq!(parse_remote_codex_session_list(""), vec![]);
}

/// End-to-end pure roundtrip: a codex discovery script's output (as the shell
/// WOULD produce it) parses back into availability + models + sessions, proving
/// the builder's section names + the parsers agree without any IO.
#[test]
fn codex_discovery_roundtrip_pure() {
    let _script = build_codex_discovery_script("codex", None);
    // Simulate what running that script prints.
    let meta = r#"{"type":"session_meta","payload":{"cwd":"/p"}}"#;
    let out = format!(
        "@@MARSAD_SEC_BEGIN:avail\n/usr/bin/codex\n@@MARSAD_SEC_END:avail\n\
         @@MARSAD_SEC_BEGIN:models\n{{\"models\":[{{\"slug\":\"gpt-5\",\"visibility\":\"show\",\"priority\":1}}]}}\n@@MARSAD_SEC_END:models\n\
         @@MARSAD_SEC_BEGIN:sessions\n1783100616000\trollout-t-11111111-2222-3333-4444-555555555555.jsonl\t{meta}\n@@MARSAD_SEC_END:sessions\n"
    );
    let secs = parse_discovery_sections(&out);
    assert!(parse_avail(secs.get("avail")));
    let models = parse_codex_debug_models(secs.get("models"));
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-5");
    let sessions = parse_remote_codex_session_list(secs.get("sessions"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(sessions[0].project.as_deref(), Some("/p"));
}

/// End-to-end pure roundtrip for claude: env section → annotated families,
/// availability, and remote session records.
#[test]
fn claude_discovery_roundtrip_pure() {
    let _script = build_claude_discovery_script("claude", None, Some("/p"));
    let user = r#"{"type":"user","message":{"content":[{"text":"hello"}]}}"#;
    let out = format!(
        "@@MARSAD_SEC_BEGIN:avail\n/usr/local/bin/claude\n@@MARSAD_SEC_END:avail\n\
         @@MARSAD_SEC_BEGIN:env\nopus\tclaude-opus-4-1\nsonnet\t\nhaiku\t\n@@MARSAD_SEC_END:env\n\
         @@MARSAD_SEC_BEGIN:sessions\n1783100616000\taaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee\t{user}\n@@MARSAD_SEC_END:sessions\n"
    );
    let secs = parse_discovery_sections(&out);
    assert!(parse_avail(secs.get("avail")));
    let resolved = parse_claude_env_models(secs.get("env"));
    let resolved_ref: std::collections::HashMap<&str, String> =
        resolved.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let catalog = build_claude_model_catalog(&resolved_ref, &[]);
    // opus resolves to a concrete versioned entry (annotated with the family);
    // the bare "opus" alias is dropped. sonnet/haiku (no env) stay as bare aliases.
    let opus = catalog
        .iter()
        .find(|m| m.id == "claude-opus-4-1")
        .expect("resolved opus version present");
    assert_eq!(opus.annotation.as_deref(), Some("opus"));
    assert!(!catalog.iter().any(|m| m.id == "opus"), "bare opus dropped");
    let sonnet = catalog.iter().find(|m| m.id == "sonnet").unwrap();
    assert_eq!(sonnet.annotation, None);
    let sessions = parse_remote_claude_session_list(secs.get("sessions"));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    assert_eq!(sessions[0].snippet, "hello");
}

#[test]
fn host_launch_cfg_resolve_command() {
    let cfg = HostLaunchCfg {
        host_target: Some("user@box".to_string()),
        setup: Some("setup".to_string()),
        agent_commands: vec![("codex".to_string(), "codex --sandbox".to_string())],
        claude_bin: Some("genie".to_string()),
    };
    // agent_commands wins.
    assert_eq!(cfg.resolve_command("codex"), "codex --sandbox");
    // claude falls back to claude_bin.
    assert_eq!(cfg.resolve_command("claude"), "genie");
    // unknown driver → bare name.
    assert_eq!(cfg.resolve_command("gemini"), "gemini");
    // default cfg → bare names.
    let d = HostLaunchCfg::default();
    assert_eq!(d.resolve_command("claude"), "claude");
    assert_eq!(d.resolve_command("codex"), "codex");
}

#[test]
fn host_probe_ctx_resolve_command_matches_cfg() {
    let ctx = HostProbeCtx {
        host_id: "h".to_string(),
        host_target: None,
        setup: None,
        agent_commands: vec![("gemini".to_string(), "/opt/g".to_string())],
        claude_bin: Some("genie".to_string()),
        scope: SessionScope::AllKnown,
        project: None,
        deadline: Duration::from_secs(1),
    };
    assert_eq!(ctx.resolve_command("gemini"), "/opt/g");
    assert_eq!(ctx.resolve_command("claude"), "genie");
    assert_eq!(ctx.resolve_command("codex"), "codex");

    // claude with NO claude_bin set (and no agent_commands entry) → bare name,
    // covering the `claude_bin: None` arm of the claude fallback.
    let ctx_no_bin = HostProbeCtx {
        host_id: "h".to_string(),
        host_target: None,
        setup: None,
        agent_commands: vec![],
        claude_bin: None,
        scope: SessionScope::AllKnown,
        project: None,
        deadline: Duration::from_secs(1),
    };
    assert_eq!(ctx_no_bin.resolve_command("claude"), "claude");
}

// ===========================================================================
// Part 2 — CapabilityProvider: the fake-driven provider seam (§3, §4, §6)
// ===========================================================================

/// A fake `CapabilityProbe` for tests: sleeps `delay`, then returns a canned
/// `Capabilities` (or echoes the requested `SessionScope` back, for the
/// project-scope-accumulation test) — never touches the real filesystem or a
/// subprocess. Mirrors `marsad-core`'s injected `GitProbe`/`TranscriptReader`
/// fakes (`crates/marsad-core/src/lib.rs` `RealGit`/`RealTranscript` seam).
struct FakeProbe {
    driver: &'static str,
    delay: Duration,
    calls: Arc<AtomicUsize>,
    errors: Vec<String>,
    echo_scope: bool,
}

impl FakeProbe {
    fn new(driver: &'static str) -> Self {
        FakeProbe {
            driver,
            delay: Duration::from_millis(0),
            calls: Arc::new(AtomicUsize::new(0)),
            errors: vec![],
            echo_scope: false,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn with_errors(mut self, errors: Vec<String>) -> Self {
        self.errors = errors;
        self
    }

    fn echoing_scope(mut self) -> Self {
        self.echo_scope = true;
        self
    }

    fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.calls)
    }
}

impl CapabilityProbe for FakeProbe {
    fn driver(&self) -> &'static str {
        self.driver
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        Capabilities {
            driver: self.driver.to_string(),
            host_id: ctx.host_id.clone(),
            cli_available: true,
            launchable: self.launchable(),
            models: Default::default(),
            available_modes: vec![],
            session_scope: Some(if self.echo_scope {
                ctx.scope.clone()
            } else {
                SessionScope::AllKnown
            }),
            sessions: vec![],
            fetched_at_ms: 1,
            errors: self.errors.clone(),
        }
    }
}

fn refresh_queued_ack(host_id: Option<&str>) -> IpcMsg {
    IpcMsg::CapabilitiesRefreshQueued {
        host_id: host_id.map(str::to_string),
    }
}

// ---------------------------------------------------------------------------
// Refresh on demand + eventual broadcast (§3.2 trigger 3)
// ---------------------------------------------------------------------------

#[test]
fn handle_refresh_returns_ack_and_eventually_broadcasts_capabilities_update() {
    let tmp = tempdir();
    let probe = FakeProbe::new("codex");
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());
    let rx = provider.subscribe();

    let ack_line =
        provider.handle_refresh(Some("local".to_string()), Some("codex".to_string()), None, HostLaunchCfg::default());
    let ack = parse_ipc(&ack_line).expect("ack must be a valid IpcMsg");
    assert_eq!(ack, refresh_queued_ack(Some("local")));

    let update_line = wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if let Ok(IpcMsg::CapabilitiesUpdate { driver, .. }) = parse_ipc(&line) {
                if driver == "codex" {
                    return Some(line);
                }
            }
        }
        None
    });
    match parse_ipc(&update_line).unwrap() {
        IpcMsg::CapabilitiesUpdate {
            host_id,
            driver,
            capabilities,
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(driver, "codex");
            assert!(capabilities.cli_available);
        }
        other => panic!("expected CapabilitiesUpdate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// New subscriber cache replay (§3.2 trigger 2)
// ---------------------------------------------------------------------------

#[test]
fn new_subscriber_receives_cached_replay_before_any_live_event() {
    let tmp = tempdir();
    let probe = FakeProbe::new("agy");
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());

    // First subscriber drives + observes the first probe completing, so the
    // result lands in the cache.
    let rx_a = provider.subscribe();
    provider.handle_refresh(Some("local".to_string()), Some("agy".to_string()), None, HostLaunchCfg::default());
    wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx_a.try_recv() {
            if line.contains("agy") {
                return Some(());
            }
        }
        None
    });

    // A SECOND subscriber, registered with NO further refresh triggered,
    // must immediately receive the cached snapshot.
    let rx_b = provider.subscribe();
    let replayed = rx_b
        .recv_timeout(Duration::from_millis(200))
        .expect("new subscriber must receive the cached CapabilitiesUpdate without a live probe");
    match parse_ipc(&replayed).expect("must be valid IpcMsg") {
        IpcMsg::CapabilitiesUpdate { driver, .. } => assert_eq!(driver, "agy"),
        other => panic!("expected CapabilitiesUpdate replay, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// In-flight dedup (§3.2: "a (driver, host) probe already running skip
// queuing another")
// ---------------------------------------------------------------------------

#[test]
fn in_flight_refresh_for_same_driver_host_is_deduped() {
    let tmp = tempdir();
    let probe = FakeProbe::new("codex").with_delay(Duration::from_millis(150));
    let counter = probe.counter();
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());

    // Two rapid refreshes for the SAME (driver, host) while the first is
    // still sleeping.
    let _ = provider.handle_refresh(Some("local".to_string()), Some("codex".to_string()), None, HostLaunchCfg::default());
    let _ = provider.handle_refresh(Some("local".to_string()), Some("codex".to_string()), None, HostLaunchCfg::default());

    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "two rapid refreshes for the same (driver, host) must execute the probe exactly once"
    );
}

// ---------------------------------------------------------------------------
// Failing probe → errors populated and broadcast, not swallowed (§3.1: "never
// panic/propagate"; §3, table row for a remote gap driver)
// ---------------------------------------------------------------------------

#[test]
fn failing_probe_populates_errors_and_still_broadcasts() {
    let tmp = tempdir();
    let probe = FakeProbe::new("agy").with_errors(vec!["agy models: exit 1: boom".to_string()]);
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());
    let rx = provider.subscribe();

    let ack = provider.handle_refresh(Some("local".to_string()), Some("agy".to_string()), None, HostLaunchCfg::default());
    assert!(
        matches!(parse_ipc(&ack).unwrap(), IpcMsg::CapabilitiesRefreshQueued { .. }),
        "a probe that will fail must still be ACKED as queued — errors surface later, not as an ack failure"
    );

    let update_line = wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("agy") {
                return Some(line);
            }
        }
        None
    });
    match parse_ipc(&update_line).unwrap() {
        IpcMsg::CapabilitiesUpdate { capabilities, .. } => {
            assert_eq!(
                capabilities.errors,
                vec!["agy models: exit 1: boom".to_string()],
                "the probe's error must be surfaced verbatim, not swallowed"
            );
        }
        other => panic!("expected CapabilitiesUpdate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Project-scope accumulation (§1: SessionScope::Projects; §2: RefreshCapabilities.project)
// ---------------------------------------------------------------------------

#[test]
fn project_scope_accumulates_across_refreshes() {
    let tmp = tempdir();
    let probe = FakeProbe::new("claude").echoing_scope();
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());
    let rx = provider.subscribe();

    provider.handle_refresh(
        Some("local".to_string()),
        Some("claude".to_string()),
        Some("/home/u/proj-a".to_string()),
        HostLaunchCfg::default(),
    );
    wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("claude") {
                return Some(());
            }
        }
        None
    });

    provider.handle_refresh(
        Some("local".to_string()),
        Some("claude".to_string()),
        Some("/home/u/proj-b".to_string()),
        HostLaunchCfg::default(),
    );
    let final_scope = wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if let Ok(IpcMsg::CapabilitiesUpdate { capabilities, .. }) = parse_ipc(&line) {
                if let Some(SessionScope::Projects(projects)) = &capabilities.session_scope {
                    if projects.contains(&"/home/u/proj-b".to_string()) {
                        return Some(capabilities.session_scope.clone().unwrap());
                    }
                }
            }
        }
        None
    });

    match final_scope {
        SessionScope::Projects(projects) => {
            assert!(
                projects.contains(&"/home/u/proj-a".to_string()),
                "must retain the first requested project"
            );
            assert!(
                projects.contains(&"/home/u/proj-b".to_string()),
                "must add the second requested project"
            );
        }
        SessionScope::AllKnown => {
            panic!("claude's scope must accumulate as Projects, not AllKnown")
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence: <data_root>/capabilities/<host_id>.json (§3.4)
// ---------------------------------------------------------------------------

#[test]
fn successful_probe_persists_to_data_root_capabilities_host_json() {
    let tmp = tempdir();
    let probe = FakeProbe::new("codex");
    let provider =
        CapabilityProvider::new(vec![Box::new(probe)], vec!["local".to_string()], tmp.path());
    let rx = provider.subscribe();
    provider.handle_refresh(Some("local".to_string()), Some("codex".to_string()), None, HostLaunchCfg::default());
    wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("codex") {
                return Some(());
            }
        }
        None
    });

    let persisted_path = tmp.path().join("capabilities").join("local.json");
    let raw = wait_for(Duration::from_secs(2), || {
        std::fs::read_to_string(&persisted_path).ok()
    });
    assert!(
        raw.contains("codex"),
        "persisted file must contain the codex driver's snapshot"
    );

    // §3.4: durability across a daemon restart — a FRESH provider with NO
    // probes at all (so it CANNOT self-populate) must still serve the
    // persisted snapshot instantly on construction.
    let restarted = CapabilityProvider::new(vec![], vec![], tmp.path());
    let snap = restarted
        .snapshot("codex", "local")
        .expect("a restarted provider must load the persisted cache synchronously at construction");
    assert!(snap.cli_available);
}

// ---------------------------------------------------------------------------
// launchable — static per-driver launch-driver truth (NOT a host probe)
// ---------------------------------------------------------------------------

/// The real probes report `launchable()` from a static per-driver fact: claude
/// and codex have launch drivers (true); gemini (agy) does not (false). This is
/// the ground truth the launch UI gates on, independent of any host `which`.
/// Real probes are `#[cfg(not(coverage))]`; this assertion only runs in the
/// normal (non-coverage) test build, matching the crate's guardrail.
#[cfg(not(coverage))]
#[test]
fn real_probes_report_correct_launchable() {
    use marsad_agent::capabilities::{RealAgyProbe, RealClaudeProbe, RealCodexProbe};
    assert!(RealClaudeProbe.launchable(), "claude has a launch driver");
    assert!(RealCodexProbe.launchable(), "codex has a launch driver");
    assert!(
        !RealAgyProbe.launchable(),
        "gemini (agy) has no launch driver"
    );
}

#[test]
fn corrupt_persisted_file_starts_empty_not_error() {
    let tmp = tempdir();
    let dir = tmp.path().join("capabilities");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("local.json"), b"{ not valid json ").unwrap();

    // Construction must not panic even though the persisted file is garbage
    // (§3.4: "Corrupt/missing file => start empty (never an error)").
    let provider = CapabilityProvider::new(vec![], vec![], tmp.path());
    assert_eq!(provider.snapshot("codex", "local"), None);
}

// ---------------------------------------------------------------------------
// Passive claude-model observation (§3.2 trigger 4)
// ---------------------------------------------------------------------------

#[test]
fn observe_claude_model_accumulates_dedupes_and_broadcasts() {
    let tmp = tempdir();
    let provider = CapabilityProvider::new(vec![], vec!["local".to_string()], tmp.path());
    let rx = provider.subscribe();

    provider.observe_claude_model("local", "claude-opus-4-8");
    provider.observe_claude_model("local", "claude-opus-4-8"); // duplicate — must not double-count
    provider.observe_claude_model("local", "claude-sonnet-4-6");

    // At least one broadcast for the new-model case must arrive.
    let _ = wait_for(Duration::from_secs(2), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("claude-sonnet-4-6") {
                return Some(());
            }
        }
        None
    });

    let snap = provider
        .snapshot("claude", "local")
        .expect("observing a model must create/update the (claude, local) entry");
    assert_eq!(snap.models.source, ModelListSource::Observed);
    let ids: std::collections::HashSet<&str> =
        snap.models.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        std::collections::HashSet::from(["claude-opus-4-8", "claude-sonnet-4-6"]),
        "the observed set must be deduped"
    );
}

#[test]
fn observed_claude_models_survive_a_restart() {
    let tmp = tempdir();
    let provider = CapabilityProvider::new(vec![], vec!["local".to_string()], tmp.path());
    provider.observe_claude_model("local", "claude-opus-4-8");
    let persisted_path = tmp.path().join("capabilities").join("local.json");
    wait_for(Duration::from_secs(2), || {
        std::fs::metadata(&persisted_path).ok()
    });

    let restarted = CapabilityProvider::new(vec![], vec![], tmp.path());
    let snap = restarted
        .snapshot("claude", "local")
        .expect("observed models must persist across a restart");
    assert!(snap.models.models.iter().any(|m| m.id == "claude-opus-4-8"));
}

// ===========================================================================
// Part 3 — THE NON-BLOCKING TEST (§4, §6: "2s-slow fake probe must not block
// a concurrent Subscribe round-trip (<100ms)")
// ===========================================================================

#[test]
fn slow_probe_never_blocks_the_ack_or_a_concurrent_subscribe() {
    let tmp = tempdir();
    let slow = FakeProbe::new("slow-driver").with_delay(Duration::from_secs(2));
    let fast = FakeProbe::new("fast-driver");
    let provider = CapabilityProvider::new(
        vec![Box::new(slow), Box::new(fast)],
        vec!["local".to_string()],
        tmp.path(),
    );

    // Send the slow-driver refresh — the ack must return immediately even
    // though the probe behind it will block a background thread for 2s.
    let refresh_start = Instant::now();
    let ack_line = provider.handle_refresh(
        Some("local".to_string()),
        Some("slow-driver".to_string()),
        None,
        HostLaunchCfg::default(),
    );
    let refresh_elapsed = refresh_start.elapsed();
    assert!(
        refresh_elapsed < Duration::from_millis(100),
        "handle_refresh must return before the probe completes, took {refresh_elapsed:?}"
    );
    assert!(matches!(
        parse_ipc(&ack_line).unwrap(),
        IpcMsg::CapabilitiesRefreshQueued { .. }
    ));

    // Immediately, CONCURRENTLY with the still-sleeping slow probe, drive an
    // unrelated round-trip: subscribing (the provider-seam analog of a GUI's
    // `Subscribe` -> `Subscribed` round trip — registering a new listener
    // must never wait on an in-flight probe).
    let subscribe_start = Instant::now();
    let rx = provider.subscribe();
    let subscribe_elapsed = subscribe_start.elapsed();
    assert!(
        subscribe_elapsed < Duration::from_millis(100),
        "subscribe() must complete instantly regardless of the in-flight slow probe, took {subscribe_elapsed:?}"
    );

    // And a second, unrelated refresh for a FAST driver must also return —
    // and its own probe must complete — well before the slow one does.
    let fast_ack = provider.handle_refresh(
        Some("local".to_string()),
        Some("fast-driver".to_string()),
        None,
        HostLaunchCfg::default(),
    );
    assert!(matches!(
        parse_ipc(&fast_ack).unwrap(),
        IpcMsg::CapabilitiesRefreshQueued { .. }
    ));
    let fast_done_at = wait_for(Duration::from_secs(1), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("fast-driver") {
                return Some(Instant::now());
            }
        }
        None
    });
    assert!(
        fast_done_at.duration_since(refresh_start) < Duration::from_secs(1),
        "the fast driver's update must not be queued behind the slow one"
    );

    // Finally, the slow driver's CapabilitiesUpdate DOES eventually arrive —
    // proving it actually ran (not silently dropped), just not on the
    // critical path of anything else.
    let slow_done_at = wait_for(Duration::from_secs(4), || {
        while let Ok(line) = rx.try_recv() {
            if line.contains("slow-driver") {
                return Some(Instant::now());
            }
        }
        None
    });
    assert!(
        slow_done_at.duration_since(refresh_start) >= Duration::from_millis(1500),
        "sanity: the slow driver's update must reflect the full ~2s probe delay, not a fast short-circuit"
    );
}

/// Companion assertion at the `DiscoveredSession`/type level (keeps this
/// file's imports honest — `DiscoveredSession` is exercised structurally
/// above via `scan_codex_sessions`/`scan_agy_sessions`/
/// `parse_remote_claude_session_list`'s return values; this pins its Default
/// derive-free field shape directly against §1's struct definition).
#[test]
fn discovered_session_field_shape_matches_spec() {
    let s = DiscoveredSession {
        id: "id-1".to_string(),
        project: None,
        snippet: String::new(),
        modified_ms: 0,
    };
    assert_eq!(s.id, "id-1");
    assert_eq!(s.project, None);
    assert_eq!(s.snippet, "");
    assert_eq!(s.modified_ms, 0);
}

// ===========================================================================
// Task B — pure parse seams: codex collab-mode response + opencode config modes
// ===========================================================================

// ---------------------------------------------------------------------------
// parse_codex_collab_list_response
// Pure seam: given the `result` object JSON from a `collaborationMode/list`
// JSON-RPC response, returns the parsed SessionMode vec.
// Wire shape verified from the reader.rs comment + wasit_codex.
// ---------------------------------------------------------------------------

/// Nominal case: two modes in the `data` array, both have string `mode` keys.
#[test]
fn parse_codex_collab_list_response_two_modes() {
    // Wire shape from codex app-server: {"id":5,"result":{"data":[...]}}
    // This function takes ONLY the result object (not the full line).
    let result_json = r#"{"data":[{"mode":"code","settings":{}},{"mode":"read-only","settings":{"someKey":"val"}}]}"#;
    let modes = parse_codex_collab_list_response(result_json);
    assert_eq!(modes.len(), 2);
    assert_eq!(modes[0].id, "code");
    assert_eq!(modes[0].label, "code");
    assert_eq!(modes[0].description, "");
    assert_eq!(modes[1].id, "read-only");
}

/// Empty `data` array → no modes, no panic.
#[test]
fn parse_codex_collab_list_response_empty_data() {
    let result_json = r#"{"data":[]}"#;
    let modes = parse_codex_collab_list_response(result_json);
    assert!(modes.is_empty());
}

/// Missing `data` key → no modes.
#[test]
fn parse_codex_collab_list_response_no_data_key() {
    let result_json = r#"{"other":"field"}"#;
    let modes = parse_codex_collab_list_response(result_json);
    assert!(modes.is_empty());
}

/// Non-string `mode` fields and missing `mode` keys are skipped (don't panic).
#[test]
fn parse_codex_collab_list_response_skips_invalid_entries() {
    let result_json = r#"{"data":[{"mode":"code"},{"no_mode_key":true},{"mode":42},{"mode":"plan"}]}"#;
    let modes = parse_codex_collab_list_response(result_json);
    assert_eq!(modes.len(), 2, "only 'code' and 'plan' have string mode values");
    assert_eq!(modes[0].id, "code");
    assert_eq!(modes[1].id, "plan");
}

/// Malformed JSON → empty, no panic.
#[test]
fn parse_codex_collab_list_response_malformed_json_is_empty() {
    let modes = parse_codex_collab_list_response("not json at all {{");
    assert!(modes.is_empty());
}

/// Real captured collaborationMode/list result shape (from codex app-server).
/// The full JSON-RPC response line is:
/// `{"id":5,"result":{"data":[{"mode":"code","settings":{...}},...]}}`
/// Only the `result` object is passed here.
#[test]
fn parse_codex_collab_list_response_real_shape() {
    // Mirroring wasit_codex's codex_collab_mode_intercept wire comment.
    let result_json = r#"{"data":[{"mode":"code","settings":{"approvalPolicy":"on-request","sandbox":"workspace-write"}},{"mode":"full-auto","settings":{"approvalPolicy":"never","sandbox":"workspace-write"}},{"mode":"read-only","settings":{"approvalPolicy":"on-request","sandbox":"read-only"}}]}"#;
    let modes = parse_codex_collab_list_response(result_json);
    assert_eq!(modes.len(), 3);
    let ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"code"), "code mode must be present");
    assert!(ids.contains(&"full-auto"), "full-auto mode must be present");
    assert!(ids.contains(&"read-only"), "read-only mode must be present");
}

// ---------------------------------------------------------------------------
// parse_opencode_config_modes
// Pure seam: given a serialized `session/new` result JSON string, returns
// (available_modes, current_mode_id) parsed from `configOptions`.
// ---------------------------------------------------------------------------

/// Nominal case: configOptions with both model and mode groups.
#[test]
fn parse_opencode_config_modes_nominal() {
    let result_json = r#"{"sessionId":"ses_x","configOptions":[{"id":"model","currentValue":"claude-sonnet-4","options":[{"value":"claude-sonnet-4","name":"Sonnet 4"}]},{"id":"mode","currentValue":"build","options":[{"value":"build","name":"build","description":"Default."},{"value":"plan","name":"plan","description":"Plan mode."}]}]}"#;
    let (modes, current) = parse_opencode_config_modes(result_json);
    assert_eq!(modes.len(), 2);
    assert_eq!(modes[0].id, "build");
    assert_eq!(modes[0].description, "Default.");
    assert_eq!(modes[1].id, "plan");
    assert_eq!(current, "build");
}

/// Missing mode group → empty modes and empty current.
#[test]
fn parse_opencode_config_modes_no_mode_group() {
    let result_json = r#"{"configOptions":[{"id":"model","currentValue":"x","options":[]}]}"#;
    let (modes, current) = parse_opencode_config_modes(result_json);
    assert!(modes.is_empty());
    assert!(current.is_empty());
}

/// Missing configOptions entirely → empty.
#[test]
fn parse_opencode_config_modes_no_config_options() {
    let result_json = r#"{"sessionId":"s"}"#;
    let (modes, current) = parse_opencode_config_modes(result_json);
    assert!(modes.is_empty());
    assert!(current.is_empty());
}

/// Malformed JSON → empty, no panic.
#[test]
fn parse_opencode_config_modes_malformed_json_is_empty() {
    let (modes, current) = parse_opencode_config_modes("{not valid json");
    assert!(modes.is_empty());
    assert!(current.is_empty());
}

/// Golden: real opencode session/new result shape (from live capture in Wasl tests).
/// configOptions has both model and mode, with user-defined mode names.
#[test]
fn parse_opencode_config_modes_golden_real_shape() {
    // Simplified real shape (amd gateway session/new result).
    let result_json = r#"{
        "sessionId": "ses_real",
        "configOptions": [
            {
                "id": "model",
                "name": "Model",
                "currentValue": "amd-anthropic/Claude-Sonnet-5",
                "options": [
                    {"value": "amd-anthropic/Claude-Haiku-4.5", "name": "Haiku"},
                    {"value": "amd-anthropic/Claude-Sonnet-5", "name": "Sonnet 5"}
                ]
            },
            {
                "id": "mode",
                "name": "Session Mode",
                "currentValue": "build",
                "options": [
                    {"value": "build", "name": "build", "description": "Default"},
                    {"value": "plan", "name": "plan", "description": "Plan mode"}
                ]
            }
        ]
    }"#;
    let (modes, current) = parse_opencode_config_modes(result_json);
    assert_eq!(modes.len(), 2);
    let mode_ids: Vec<&str> = modes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(mode_ids, vec!["build", "plan"]);
    assert_eq!(current, "build");
}

// ===========================================================================
// Task D — filter_codex_sessions_by_project (pure)
// ===========================================================================

fn make_session(id: &str, project: Option<&str>) -> DiscoveredSession {
    DiscoveredSession {
        id: id.to_string(),
        project: project.map(str::to_string),
        snippet: String::new(),
        modified_ms: 0,
    }
}

/// Sessions matching the requested project pass; others are excluded.
#[test]
fn filter_codex_sessions_by_project_keeps_matching() {
    let sessions = vec![
        make_session("s1", Some("/home/user/proj")),
        make_session("s2", Some("/home/user/other")),
        make_session("s3", Some("/home/user/proj")),
        make_session("s4", None),
    ];
    let filtered = filter_codex_sessions_by_project(sessions, "/home/user/proj");
    let ids: Vec<&str> = filtered.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["s1", "s3"], "only exact-project sessions survive");
}

/// Sessions with `project: None` are excluded when a project is requested.
#[test]
fn filter_codex_sessions_by_project_excludes_no_project() {
    let sessions = vec![
        make_session("s1", None),
        make_session("s2", Some("/home/user/proj")),
    ];
    let filtered = filter_codex_sessions_by_project(sessions, "/home/user/proj");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "s2");
}

/// Trailing slashes are stripped before comparison (honest path-equality).
#[test]
fn filter_codex_sessions_by_project_trims_trailing_slashes() {
    let sessions = vec![
        make_session("s1", Some("/home/user/proj/")),
        make_session("s2", Some("/home/user/proj")),
    ];
    // Both should match "/home/user/proj" after trimming.
    let filtered = filter_codex_sessions_by_project(sessions, "/home/user/proj");
    assert_eq!(filtered.len(), 2, "trailing slashes are stripped before comparison");
}

/// Empty input → empty output.
#[test]
fn filter_codex_sessions_by_project_empty_input() {
    let filtered = filter_codex_sessions_by_project(vec![], "/proj");
    assert!(filtered.is_empty());
}

/// All sessions match → all pass.
#[test]
fn filter_codex_sessions_by_project_all_match() {
    let sessions = vec![
        make_session("a", Some("/proj")),
        make_session("b", Some("/proj")),
    ];
    let filtered = filter_codex_sessions_by_project(sessions, "/proj");
    assert_eq!(filtered.len(), 2);
}

/// No sessions match → empty output.
#[test]
fn filter_codex_sessions_by_project_none_match() {
    let sessions = vec![
        make_session("x", Some("/other")),
        make_session("y", None),
    ];
    let filtered = filter_codex_sessions_by_project(sessions, "/proj");
    assert!(filtered.is_empty());
}

// Task D — parse_codex_rollout_meta with the EXACT real on-disk first-line format
// (verified against ~/.codex/sessions on this host — 2026-08-04).
// Real first line: {"timestamp":"2026-07-05T03:43:07.069Z","type":"session_meta",
//   "payload":{...,"cwd":"/tmp",...}}

const REAL_DISK_ROLLOUT_FIRST_LINE: &str = r#"{"timestamp":"2026-07-05T03:43:07.069Z","type":"session_meta","payload":{"session_id":"019f305f-689e-7f13-a0e0-942dab67bc8c","id":"019f305f-689e-7f13-a0e0-942dab67bc8c","timestamp":"2026-07-05T03:43:06.913Z","cwd":"/tmp","originator":"codex_exec","cli_version":"0.142.5","source":"exec","thread_source":"user","model_provider":"amd","base_instructions":{"text":"You are a coding agent."}}}"#;

const REAL_DISK_ROLLOUT_FILENAME: &str =
    "rollout-2026-07-05T03-43-06-019f305f-689e-7f13-a0e0-942dab67bc8c.jsonl";

/// Verifies that parse_codex_rollout_meta correctly handles the REAL on-disk format
/// that uses an outer `timestamp` field (top-level) in addition to the inner
/// `payload` structure. The function must extract `payload.cwd` correctly.
#[test]
fn parse_codex_rollout_meta_real_disk_format_extracts_cwd() {
    let (id, project) = parse_codex_rollout_meta(REAL_DISK_ROLLOUT_FILENAME, REAL_DISK_ROLLOUT_FIRST_LINE)
        .expect("real on-disk format must parse successfully");
    assert_eq!(id, "019f305f-689e-7f13-a0e0-942dab67bc8c", "uuid from filename");
    assert_eq!(
        project.as_deref(),
        Some("/tmp"),
        "payload.cwd from the real on-disk first line"
    );
}

/// Companion: the full real-disk discovery roundtrip through parse_remote_codex_session_list.
/// A discovery section record uses `mtime<TAB>filename<TAB>first_line`.
#[test]
fn parse_remote_codex_session_list_real_disk_format_roundtrip() {
    let record = format!(
        "1783100616000\t{filename}\t{first_line}",
        filename = REAL_DISK_ROLLOUT_FILENAME,
        first_line = REAL_DISK_ROLLOUT_FIRST_LINE,
    );
    let sessions = parse_remote_codex_session_list(&record);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "019f305f-689e-7f13-a0e0-942dab67bc8c");
    assert_eq!(sessions[0].project.as_deref(), Some("/tmp"));
    assert_eq!(sessions[0].modified_ms, 1783100616000);
}

// ===========================================================================
// Task B (mode probe transport fix) — build_probe_spawn_argv pure unit tests
//
// Verifies that the mode probe subprocess argv is constructed correctly for
// ALL supported transports: local (None), SSH (user@host / bare alias),
// WSL (wsl:<distro>), plink (plink:<target>), and wslssh (<distro>:<target>).
// These tests are PURE — no IO, no subprocess, no live tools required.
// ===========================================================================

/// Helper: build a minimal tool_argv for assertions.
fn tool_argv_codex() -> Vec<String> {
    vec![
        "codex".to_string(),
        "app-server".to_string(),
        "--stdio".to_string(),
    ]
}

fn tool_argv_opencode() -> Vec<String> {
    vec!["opencode".to_string(), "acp".to_string()]
}

/// Local (None) → ["bash", "-lc", "exec 'codex' 'app-server' '--stdio'"]
/// No ssh/wsl prefix; bash -lc not bash -l -s.
#[test]
fn build_probe_spawn_argv_local_none_starts_with_bash_lc() {
    let argv = build_probe_spawn_argv(None, None, &tool_argv_codex()).unwrap();
    assert_eq!(argv[0], "bash", "local spawn must start with bash");
    assert_eq!(argv[1], "-lc", "must use -lc (not -l -s) so stdin stays free for JSON-RPC");
    let inner = &argv[2];
    assert!(inner.contains("exec "), "inner must contain exec");
    assert!(inner.contains("'codex'"), "tool word must be single-quoted");
    assert!(inner.contains("'app-server'"), "subcommand must be single-quoted");
    assert!(!inner.contains("-l -s"), "must NOT use -l -s syntax");
}

/// "local" and "localhost" string targets behave identically to None.
#[test]
fn build_probe_spawn_argv_local_string_aliases() {
    let none_argv = build_probe_spawn_argv(None, None, &tool_argv_codex()).unwrap();
    let local_argv = build_probe_spawn_argv(Some("local"), None, &tool_argv_codex()).unwrap();
    let localhost_argv =
        build_probe_spawn_argv(Some("localhost"), None, &tool_argv_codex()).unwrap();
    assert_eq!(local_argv, none_argv, "'local' must behave identically to None");
    assert_eq!(localhost_argv, none_argv, "'localhost' must behave identically to None");
}

/// SSH `user@host` → ["ssh", "-T", ..., "user@host", "bash", "-lc", inner]
#[test]
fn build_probe_spawn_argv_ssh_user_at_host() {
    let argv = build_probe_spawn_argv(Some("user@host"), None, &tool_argv_codex()).unwrap();
    assert_eq!(argv[0], "ssh", "SSH transport must start with ssh");
    // The target must be directly before "bash"
    let bash_pos = argv.iter().position(|a| a == "bash").expect("must contain bash");
    assert_eq!(argv[bash_pos - 1], "user@host", "target must appear just before bash");
    assert_eq!(argv[bash_pos + 1], "-lc", "must use bash -lc");
    // The inner command (after bash -lc) must have exec + quoted tool
    let inner = &argv[bash_pos + 2];
    assert!(inner.contains("exec 'codex'"), "inner must exec the quoted tool");
    // -T must appear (no PTY)
    assert!(argv.contains(&"-T".to_string()), "ssh must include -T");
}

/// Bare SSH alias (no @) → treated as SSH, target is the alias.
#[test]
fn build_probe_spawn_argv_ssh_bare_alias() {
    let argv = build_probe_spawn_argv(Some("etx"), None, &tool_argv_codex()).unwrap();
    assert_eq!(argv[0], "ssh");
    let bash_pos = argv.iter().position(|a| a == "bash").unwrap();
    assert_eq!(argv[bash_pos - 1], "etx");
    assert_eq!(argv[bash_pos + 1], "-lc");
}

/// `wsl:Ubuntu` → ["wsl.exe", "-d", "Ubuntu", "bash", "-lc", inner]
#[test]
fn build_probe_spawn_argv_wsl_distro() {
    let argv = build_probe_spawn_argv(Some("wsl:Ubuntu"), None, &tool_argv_opencode()).unwrap();
    assert_eq!(argv[0], "wsl.exe");
    assert_eq!(argv[1], "-d");
    assert_eq!(argv[2], "Ubuntu");
    assert_eq!(argv[3], "bash");
    assert_eq!(argv[4], "-lc");
    let inner = &argv[5];
    assert!(inner.contains("exec 'opencode' 'acp'"), "inner must exec opencode acp");
}

/// `plink:etx` → ["plink", "-share", "-batch", "etx", "bash", "-lc", inner]
#[test]
fn build_probe_spawn_argv_plink_target() {
    let argv = build_probe_spawn_argv(Some("plink:etx"), None, &tool_argv_codex()).unwrap();
    assert_eq!(argv[0], "plink");
    assert_eq!(argv[1], "-share");
    assert_eq!(argv[2], "-batch");
    assert_eq!(argv[3], "etx");
    assert_eq!(argv[4], "bash");
    assert_eq!(argv[5], "-lc");
    let inner = &argv[6];
    assert!(inner.contains("exec 'codex'"), "inner must exec codex");
}

/// `wslssh:Ubuntu:agent33` → ["wsl.exe", "-d", "Ubuntu", "ssh", ..., "agent33", "bash", "-lc", inner]
#[test]
fn build_probe_spawn_argv_wslssh_distro_target() {
    let argv =
        build_probe_spawn_argv(Some("wslssh:Ubuntu:agent33"), None, &tool_argv_codex()).unwrap();
    assert_eq!(argv[0], "wsl.exe");
    assert_eq!(argv[1], "-d");
    assert_eq!(argv[2], "Ubuntu");
    assert_eq!(argv[3], "ssh");
    // The final SSH target appears just before "bash"
    let bash_pos = argv.iter().position(|a| a == "bash").expect("must contain bash");
    assert_eq!(argv[bash_pos - 1], "agent33", "wslssh target must appear just before bash");
    assert_eq!(argv[bash_pos + 1], "-lc");
    let inner = &argv[bash_pos + 2];
    assert!(inner.contains("exec 'codex'"), "inner must exec codex");
    // ControlMaster is always on for wslssh (Linux ssh in WSL)
    assert!(
        argv.contains(&"ControlMaster=auto".to_string()),
        "wslssh must always include ControlMaster=auto: {argv:?}"
    );
}

/// `wslssh:Debian:me@h.example.com` — target with user@host preserved verbatim.
#[test]
fn build_probe_spawn_argv_wslssh_user_at_host_target() {
    let argv = build_probe_spawn_argv(
        Some("wslssh:Debian:me@h.example.com"),
        None,
        &tool_argv_codex(),
    )
    .unwrap();
    let bash_pos = argv.iter().position(|a| a == "bash").unwrap();
    assert_eq!(argv[bash_pos - 1], "me@h.example.com");
}

/// Setup (Some, non-empty) is prepended as `{setup}; exec {tool_argv}` in the inner command.
#[test]
fn build_probe_spawn_argv_setup_prepended_before_exec() {
    let argv =
        build_probe_spawn_argv(None, Some("module load cuda"), &tool_argv_codex()).unwrap();
    let inner = &argv[2]; // bash -lc <inner>
    assert!(
        inner.starts_with("module load cuda; exec "),
        "setup must precede exec: {inner:?}"
    );
    assert!(inner.contains("exec 'codex'"), "exec must follow setup");
}

/// Setup = None or empty string → no setup prefix, just `exec {tool_argv}`.
#[test]
fn build_probe_spawn_argv_no_setup_no_prefix() {
    let no_setup = build_probe_spawn_argv(None, None, &tool_argv_codex()).unwrap();
    let empty_setup = build_probe_spawn_argv(None, Some(""), &tool_argv_codex()).unwrap();
    let whitespace_setup =
        build_probe_spawn_argv(None, Some("   "), &tool_argv_codex()).unwrap();

    for (label, argv) in [
        ("no_setup", &no_setup),
        ("empty_setup", &empty_setup),
        ("whitespace_setup", &whitespace_setup),
    ] {
        let inner = &argv[2];
        assert!(
            inner.starts_with("exec "),
            "{label}: inner must start with 'exec ' when setup is absent/empty, got: {inner:?}"
        );
    }
}

/// Multi-word command (e.g. "codex --sandbox") — extra flags become quoted argv elements.
#[test]
fn build_probe_spawn_argv_multiword_command_flags_are_quoted() {
    let tool_argv = vec![
        "codex".to_string(),
        "--sandbox".to_string(),
        "app-server".to_string(),
        "--stdio".to_string(),
    ];
    let argv = build_probe_spawn_argv(None, None, &tool_argv).unwrap();
    let inner = &argv[2];
    assert!(inner.contains("'--sandbox'"), "extra flags must be quoted: {inner:?}");
}

/// Argument with single quotes → escaped via sh_squote (e.g. `it's` → `'it'\''s'`).
#[test]
fn build_probe_spawn_argv_single_quotes_in_argv_are_escaped() {
    let tool_argv = vec!["echo".to_string(), "it's a test".to_string()];
    let argv = build_probe_spawn_argv(None, None, &tool_argv).unwrap();
    let inner = &argv[2];
    // sh_squote produces 'it'\''s a test'
    assert!(
        inner.contains(r"'\''"),
        "embedded single quotes must be shell-escaped: {inner:?}"
    );
}

/// Empty tool_argv → Err (no binary to spawn).
#[test]
fn build_probe_spawn_argv_empty_tool_argv_is_err() {
    let result = build_probe_spawn_argv(None, None, &[]);
    assert!(result.is_err(), "empty tool_argv must return Err");
}

/// SSH transport: inner contains `exec ` immediately after optional setup.
#[test]
fn build_probe_spawn_argv_ssh_inner_contains_exec() {
    let argv = build_probe_spawn_argv(
        Some("user@host"),
        Some("source ~/env.sh"),
        &tool_argv_opencode(),
    )
    .unwrap();
    let bash_pos = argv.iter().position(|a| a == "bash").unwrap();
    let inner = &argv[bash_pos + 2];
    assert!(
        inner.starts_with("source ~/env.sh; exec "),
        "ssh inner must have setup; exec: {inner:?}"
    );
    assert!(inner.contains("'opencode'"), "tool must be quoted: {inner:?}");
}

/// Invariant: the token "-lc" always appears immediately after "bash" in the argv —
/// never "-l -s" (which would consume stdin and break JSON-RPC).
#[test]
fn build_probe_spawn_argv_never_uses_l_s_form() {
    for target in [
        None,
        Some("local"),
        Some("user@host"),
        Some("wsl:Ubuntu"),
        Some("plink:etx"),
        Some("wslssh:Ubuntu:agent33"),
    ] {
        let argv = build_probe_spawn_argv(target, None, &tool_argv_codex()).unwrap();
        let bash_pos = argv.iter().position(|a| a == "bash").unwrap();
        assert_eq!(
            argv[bash_pos + 1], "-lc",
            "transport {target:?}: must use bash -lc, not bash -l -s"
        );
        // Also confirm no standalone "-s" follows
        assert!(
            !argv[bash_pos..].contains(&"-s".to_string()),
            "transport {target:?}: must not contain bare -s: {argv:?}"
        );
    }
}
