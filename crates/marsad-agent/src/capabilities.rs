//! U1 dynamic capability discovery — the agent-side capability provider.
//!
//! See `docs/specs/dynamic-capabilities.md` §1.1/§1.2 (per-driver model/session
//! discovery mechanisms), §3 (the `CapabilityProbe` seam, refresh triggers,
//! staleness policy, persistence), and §4 (the non-blocking rule).
//!
//! Layout:
//! - Pure parse functions (`parse_agy_models`, `parse_codex_debug_models`,
//!   `parse_codex_rollout_meta`, `agy_snippet_from_transcript`,
//!   `parse_remote_claude_session_list`) — no IO, fully unit-tested.
//! - Temp-dir-parameterized scanners (`scan_codex_sessions`,
//!   `scan_agy_sessions`) — real filesystem IO but rooted at a caller-supplied
//!   `&Path`, so tests can point them at a temp tree.
//! - The `CapabilityProbe` trait + `HostProbeCtx` (§3.1) and `CapabilityProvider`
//!   (the provider seam: refresh-on-demand, cache replay, in-flight dedup,
//!   TTLs, persistence, spawned-thread probes with hard deadlines).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use marsad_proto::{
    encode_ipc, Capabilities, DiscoveredSession, IpcMsg, ModelCatalog, ModelInfo, ModelListSource,
    SessionScope,
};

// ===========================================================================
// Part 1 — pure parse functions (no IO)
// ===========================================================================

/// Parse `agy models` stdout: one model name per line, verbatim (spaces,
/// parens, mixed vendors all preserved). Blank/whitespace-only lines and
/// trailing whitespace are tolerated; empty input yields an empty list.
pub fn parse_agy_models(raw: &str) -> Vec<ModelInfo> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| ModelInfo {
            id: line.to_string(),
            annotation: None,
        })
        .collect()
}

/// claude has no `list-models` command (dynamic-capabilities.md §1.1), but the
/// CLI DOES accept the model FAMILY words `opus`/`sonnet`/`haiku` directly (and
/// the user's `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` env resolves each
/// family to a concrete versioned id at launch). This pure builder offers those
/// three families so the picker isn't stuck on a single observed model.
///
/// `resolved` maps a family name → its resolved versioned id (from the env),
/// used only as a verbatim `annotation` (never a synthesized sublabel — it's
/// the driver's own configured value). `observed` are models already seen via
/// `observe_claude_model`; they're MERGED in (appended, de-duplicated by id) so
/// nothing observed is lost. Family entries come first (stable order:
/// opus, sonnet, haiku), then any observed model not already listed.
pub fn build_claude_model_catalog(
    resolved: &HashMap<&str, String>,
    observed: &[ModelInfo],
) -> Vec<ModelInfo> {
    const FAMILIES: [&str; 3] = ["opus", "sonnet", "haiku"];
    let mut out: Vec<ModelInfo> = Vec::new();

    // 1. Concrete versioned models resolved from the host env (family → version),
    //    annotated with the family word (picker reads "claude-opus-4-8 (opus)").
    for family in FAMILIES {
        if let Some(v) = resolved.get(family).map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if !out.iter().any(|m| m.id == v) {
                out.push(ModelInfo {
                    id: v.to_string(),
                    annotation: Some(family.to_string()),
                });
            }
        }
    }

    // 2. Every observed/discovered model (transcript scan, config, env), deduped.
    for m in observed {
        if !out.iter().any(|existing| existing.id == m.id) {
            out.push(m.clone());
        }
    }

    // 3. Drop the bare family alias (opus/sonnet/haiku) whenever a CONCRETE
    //    versioned model of that family is present — the picker must not show both
    //    "opus" AND "claude-opus-4-8". A versioned id contains its family word.
    //    Precompute which families are versioned to avoid borrowing `out` twice.
    let versioned: Vec<&str> = FAMILIES
        .iter()
        .copied()
        .filter(|f| out.iter().any(|m| m.id.as_str() != *f && m.id.contains(f)))
        .collect();
    out.retain(|m| !(FAMILIES.contains(&m.id.as_str()) && versioned.contains(&m.id.as_str())));

    // 4. Bare family fallback: if a family has NEITHER a version NOR a bare entry
    //    yet (nothing observed/resolved for it), offer the bare alias so it stays
    //    pickable (claude accepts "opus"/"sonnet"/"haiku" directly).
    for family in FAMILIES {
        let present = out.iter().any(|m| m.id.contains(family));
        if !present {
            out.push(ModelInfo {
                id: family.to_string(),
                annotation: None,
            });
        }
    }

    out
}

/// Append each `observed` model not already present (by id) onto `catalog`'s
/// model list, preserving the existing order (family entries stay first). Pure;
/// mutates the passed `ModelCatalog` in place. Used to re-fold the provider's
/// observed-model superset back into a freshly-probed claude catalog.
pub fn merge_observed_into_catalog(catalog: &mut ModelCatalog, observed: &[ModelInfo]) {
    for m in observed {
        if !catalog.models.iter().any(|existing| existing.id == m.id) {
            catalog.models.push(m.clone());
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct CodexModelsFile {
    #[serde(default)]
    models: Vec<CodexModelEntry>,
}

#[derive(Debug, Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    priority: Option<i64>,
}

/// Parse `codex debug models` JSON stdout. Hidden models (`visibility ==
/// "hide"`) are excluded; the remaining models' numeric `priority`, stringified
/// verbatim, lands in `annotation`. Malformed JSON or an absent/empty `models`
/// array degrades to an empty list — never a panic (the caller surfaces the
/// real subprocess failure into `Capabilities.errors` instead).
pub fn parse_codex_debug_models(raw: &str) -> Vec<ModelInfo> {
    serde_json::from_str::<CodexModelsFile>(raw)
        .unwrap_or_default()
        .models
        .into_iter()
        .filter(|m| m.visibility != "hide")
        .map(|m| ModelInfo {
            id: m.slug,
            annotation: m.priority.map(|p| p.to_string()),
        })
        .collect()
}

/// Parse a codex rollout filename + its first NDJSON line into `(id, project)`.
///
/// `filename` must match `rollout-<timestamp>-<uuid>.jsonl` (codex-research.md
/// §d) — the id is the trailing 36-character UUID. A non-matching filename
/// yields `None` (not a rollout file at all). A well-formed filename with a
/// malformed/non-`session_meta` first line still yields the id (from the
/// filename alone) with `project: None` — "skip, don't fail" (§6).
pub fn parse_codex_rollout_meta(
    filename: &str,
    first_line: &str,
) -> Option<(String, Option<String>)> {
    let stem = filename
        .strip_prefix("rollout-")
        .and_then(|s| s.strip_suffix(".jsonl"))?;
    let id = stem[stem.len().saturating_sub(36)..].to_string();

    let project = serde_json::from_str::<serde_json::Value>(first_line)
        .unwrap_or(serde_json::Value::Null)
        .get("payload")
        .and_then(|p| p.get("cwd"))
        .and_then(|c| c.as_str())
        .map(str::to_string);

    Some((id, project))
}

/// Extract the first `event_msg`/`response_item` user message text from a
/// codex rollout file body (codex-research.md §d). Falls through to an empty
/// string when no `response_item` line exists.
fn extract_codex_user_snippet(body: &str) -> String {
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        if v.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        return v
            .get("payload")
            .and_then(|p| p.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|content| content.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
    }
    String::new()
}

/// Strip the `<USER_REQUEST>…</USER_REQUEST>` wrapper (and sibling injected
/// blocks) from an agy `transcript_full.jsonl` body, returning the first
/// `USER_INPUT` step's real prompt text. `None` when the input is empty or no
/// `USER_INPUT` step exists.
pub fn agy_snippet_from_transcript(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        if v.get("type").and_then(|t| t.as_str()) != Some("USER_INPUT") {
            continue;
        }
        let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let start = content
            .find("<USER_REQUEST>")
            .map(|i| i + "<USER_REQUEST>".len())
            .unwrap_or(0);
        let end = content.find("</USER_REQUEST>").unwrap_or(content.len());
        return content.get(start..end).map(|s| s.trim().to_string());
    }
    None
}

/// Parse the moved-from-bridge remote claude session list: one
/// `<mtime_ms>\t<uuid>\t<first-user-line-json>` record per line, as produced
/// by the agent's `fetch_remote_session_list` remote script. `project` is
/// always `None` (the record carries no project column — the caller already
/// knows the requested project). Malformed mtime or an empty id skips the
/// record; a missing third field yields an empty snippet. Order is preserved
/// (the remote `ls -t` already emits newest-first).
pub fn parse_remote_claude_session_list(raw: &str) -> Vec<DiscoveredSession> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.splitn(3, '\t');
        let mtime_str = parts.next().unwrap_or("");
        let id = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let modified_ms: u64 = match mtime_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(DiscoveredSession {
            id: id.to_string(),
            project: None,
            snippet: extract_user_text(rest),
            modified_ms,
        });
    }
    out
}

/// Extract the first text block of a raw `{"type":"user",...}` stream-json
/// line. Anything that doesn't parse as a `user` message (or lacks a text
/// block) yields an empty snippet (never a parse error surfaced to the caller).
fn extract_user_text(raw_line: &str) -> String {
    if raw_line.trim().is_empty() {
        return String::new();
    }
    serde_json::from_str::<serde_json::Value>(raw_line)
        .unwrap_or(serde_json::Value::Null)
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.get(0))
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

// ===========================================================================
// Part 1b — discovery SHELL SCRIPT builders + section parser (all PURE, no IO)
//
// Each driver builds ONE discovery script that runs the host's `setup` first
// (so `command -v`, `codex debug models`, the claude env, etc. see the SAME
// environment a launched session would), then emits sections delimited by
// unique sentinel markers. `run_local`/`run_remote_script_bounded` executes the
// script; `parse_discovery_sections` splits the output back into named sections
// with NO IO. Every builder + the splitter is unit-tested (the crate's 100% bar
// stays on this pure layer; the real exec paths are `cfg(not(coverage))`).
// ===========================================================================

/// Sentinel that opens a named discovery section. The name is a fixed ASCII
/// keyword (`avail`/`models`/`sessions`/`env`) chosen by the builders — never
/// user input — so it can never collide with shell metacharacters.
const SECTION_BEGIN: &str = "@@MARSAD_SEC_BEGIN:";
/// Sentinel that closes a named discovery section.
const SECTION_END: &str = "@@MARSAD_SEC_END:";

/// POSIX single-quote a string for safe embedding in a `bash -l -s` script.
/// (Kept in-crate rather than reused from `remote.rs`, whose `remote_shell_quote`
/// is `pub(crate)` but lives behind `cfg(not(coverage))`-adjacent code; this one
/// is always compiled + unit-tested so the pure builders keep the crate's 100%
/// bar.)
fn sh_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Emit the shell that prints a section: a BEGIN marker line, the body of
/// `cmd` (run in a subshell so a failure doesn't abort the outer script), then
/// an END marker line. `name` is a fixed keyword; `cmd` is verbatim shell.
fn section(name: &str, cmd: &str) -> String {
    format!(
        "echo '{SECTION_BEGIN}{name}'\n{cmd}\necho '{SECTION_END}{name}'\n",
        SECTION_BEGIN = SECTION_BEGIN,
        SECTION_END = SECTION_END,
        name = name,
        cmd = cmd,
    )
}

/// Prepend the host's `setup` (if any) to a discovery script body. The setup runs
/// in the SAME shell as the sections so its env/PATH mutations are visible to
/// `command -v` and the models subcommands. A failing setup is tolerated
/// (`|| true`) so discovery still reports availability honestly rather than
/// aborting with an empty result.
fn with_setup(setup: Option<&str>, body: &str) -> String {
    match setup.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("{{ {s}; }} 2>/dev/null || true\n{body}", s = s, body = body),
        None => body.to_string(),
    }
}

/// The first whitespace-separated token of a resolved command line — the WORD
/// that `command -v` checks for availability (e.g. `codex --sandbox` → `codex`,
/// `/opt/genie/genie` → `/opt/genie/genie`). Empty/whitespace-only input yields
/// an empty string (the caller treats an empty word as "unavailable").
pub fn command_word(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("")
}

/// Build the claude discovery script: after `setup`, check availability of the
/// resolved command's first word, then print the three `ANTHROPIC_DEFAULT_*`
/// env values (the host's real versioned models — claude has no "list models"
/// command, so the env is the only truthful per-host source). Three additional
/// sections surface custom/subscription models: `ANTHROPIC_MODEL` env override,
/// `"model"` fields in the claude config JSON files, and `"model":"..."` values
/// observed in recent transcript JSONL files. Sessions are listed per requested
/// `project` (claude stores transcripts under `~/.claude/projects/<encoded-cwd>/`),
/// newest-first, one `mtime_ms<TAB>uuid<TAB><first user line json>` record each —
/// the same shape `parse_remote_claude_session_list` already parses.
pub fn build_claude_discovery_script(
    command: &str,
    setup: Option<&str>,
    project: Option<&str>,
) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    // Print each family's resolved env value on its own labelled line so the
    // parser can map family → versioned id.
    let env = section(
        "env",
        "printf 'opus\\t%s\\n' \"${ANTHROPIC_DEFAULT_OPUS_MODEL:-}\"\n\
         printf 'sonnet\\t%s\\n' \"${ANTHROPIC_DEFAULT_SONNET_MODEL:-}\"\n\
         printf 'haiku\\t%s\\n' \"${ANTHROPIC_DEFAULT_HAIKU_MODEL:-}\"",
    );
    // Surface any custom/subscription model from three host sources.
    //
    // Source 1: ANTHROPIC_MODEL env — the primary single-model override used by
    // subscription and custom setups (e.g. "fable" pinned this way).
    // ANTHROPIC_SMALL_FAST_MODEL is a secondary fast-path override.
    let extra_models = section(
        "extra_models",
        "base=${CLAUDE_CONFIG_DIR:-$HOME/.claude}\n\
         printf 'model\\t%s\\n' \"${ANTHROPIC_MODEL:-}\"\n\
         printf 'model\\t%s\\n' \"${ANTHROPIC_SMALL_FAST_MODEL:-}\"\n\
         \n\
         # Source 2: \"model\" fields in claude config JSON files (no jq dependency).\n\
         grep -ho '\"model\"[[:space:]]*:[[:space:]]*\"[^\"]*\"' \
           \"$base/settings.json\" \"$HOME/.claude.json\" 2>/dev/null \
           | sed 's/.*\"\\([^\"]*\\)\"$/\\1/' \
           | while IFS= read -r m; do printf 'model\\t%s\\n' \"$m\"; done\n\
         \n\
         # Source 3: models observed in recent transcripts — most likely to surface\n\
         # a subscription model like \"fable\" that is only stored in session history.\n\
         # Bounded so it can't dominate the probe deadline on a slow remote FS:\n\
         # only the newest few files, each read for just its FIRST model match\n\
         # (grep -m1, so a huge transcript isn't scanned end-to-end).\n\
         ls -t \"$base\"/projects/*/*.jsonl 2>/dev/null | head -6 \
           | while IFS= read -r f; do \
               grep -m1 -h -o '\"model\":\"[^\"]*\"' \"$f\" 2>/dev/null; \
             done \
           | sed 's/\"model\":\"\\([^\"]*\\)\"/\\1/' \
           | sort -u | head -20 \
           | while IFS= read -r m; do printf 'model\\t%s\\n' \"$m\"; done",
    );
    // Sessions: only when a concrete project is in scope (claude's on-disk dir is
    // the project's real cwd, `/`→non-alnum encoded). Resolve the cwd on the host
    // and encode it there, matching `encode_project_dir` exactly.
    let sessions = match project {
        Some(p) if !p.trim().is_empty() => {
            let cd = cd_for(p);
            let body = format!(
                "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\n\
                 cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0\n\
                 enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g')\n\
                 dir=\"$base/projects/$enc\"\n\
                 for f in $(ls -t \"$dir\"/*.jsonl 2>/dev/null); do \
                   u=$(basename \"$f\" .jsonl); \
                   m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
                   l=$(grep -m1 '\"type\":\"user\"' \"$f\" 2>/dev/null | head -c 4000); \
                   printf '%s000\\t%s\\t%s\\n' \"$m\" \"$u\" \"$l\"; \
                 done",
                cd = cd
            );
            section("sessions", &body)
        }
        _ => section("sessions", "true"),
    };
    with_setup(setup, &format!("{avail}{env}{extra_models}{sessions}"))
}

/// Build the codex discovery script: after `setup`, availability of the resolved
/// command word, then `<command> debug models` (JSON parsed by
/// `parse_codex_debug_models`), then the newest codex rollout sessions under
/// `~/.codex/sessions` printed as `mtime_ms<TAB>filename<TAB><first NDJSON line>`
/// records (parsed by `parse_remote_codex_session_list`).
pub fn build_codex_discovery_script(command: &str, setup: Option<&str>) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    // Run the models subcommand against the RESOLVED command (verbatim, so flags
    // like `--sandbox` are preserved) only if the word resolves.
    let models = section(
        "models",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} debug models 2>/dev/null || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    let sessions = section(
        "sessions",
        "root=\"$HOME/.codex/sessions\"\n\
         for f in $(ls -t \"$root\"/*/*/*/rollout-*.jsonl 2>/dev/null | head -200); do \
           b=$(basename \"$f\"); \
           m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
           l=$(head -1 \"$f\" 2>/dev/null | head -c 4000); \
           printf '%s000\\t%s\\t%s\\n' \"$m\" \"$b\" \"$l\"; \
         done",
    );
    with_setup(setup, &format!("{avail}{models}{sessions}"))
}

/// Build the agy/gemini discovery script: after `setup`, availability of the
/// resolved command word, then `<command> models` (parsed by `parse_agy_models`).
/// Sessions are NOT enumerated over the connection (the agy brain-dir layout is
/// scanned locally only); an empty sessions section keeps the output shape
/// uniform.
pub fn build_agy_discovery_script(command: &str, setup: Option<&str>) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    let models = section(
        "models",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} models 2>/dev/null || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    with_setup(setup, &format!("{avail}{models}"))
}

/// A parsed discovery output: the named sections extracted between the
/// BEGIN/END sentinels. Unknown/duplicate section names keep the LAST body
/// (probes only emit each name once). Text outside any section (setup noise,
/// login-shell banners) is discarded.
#[derive(Debug, Default, PartialEq)]
pub struct DiscoverySections {
    pub sections: HashMap<String, String>,
}

impl DiscoverySections {
    /// Body of `name`, or `""` when absent.
    pub fn get(&self, name: &str) -> &str {
        self.sections.get(name).map(String::as_str).unwrap_or("")
    }
}

/// Split raw discovery stdout into named sections by the BEGIN/END sentinels.
/// A BEGIN with no matching END (truncated output / a killed probe) yields
/// whatever body accumulated up to EOF — honest partial data, never a panic.
/// Lines outside any open section are ignored.
pub fn parse_discovery_sections(raw: &str) -> DiscoverySections {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix(SECTION_BEGIN) {
            // A new BEGIN closes any dangling open section first.
            if let Some((n, body)) = current.take() {
                sections.insert(n, body.join("\n"));
            }
            current = Some((name.trim().to_string(), Vec::new()));
        } else if let Some(name) = line.strip_prefix(SECTION_END) {
            if let Some((n, body)) = current.take() {
                // Only close if the END names the open section; a mismatched END
                // still closes (defensive) — the name recorded is the OPEN one.
                let _ = name;
                sections.insert(n, body.join("\n"));
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line.to_string());
        }
    }
    // A truncated output: flush the still-open section.
    if let Some((n, body)) = current.take() {
        sections.insert(n, body.join("\n"));
    }
    DiscoverySections { sections }
}

/// Interpret an `avail` section body: availability is "the resolved command word
/// printed a non-empty path" (`command -v` prints the resolved path on success,
/// nothing on failure). Trims; any non-empty line means available.
pub fn parse_avail(body: &str) -> bool {
    body.lines().any(|l| !l.trim().is_empty())
}

/// Parse the claude `env` section (`family<TAB>value` lines) into the
/// family→resolved-id map `build_claude_model_catalog` consumes. Empty values
/// (env unset) are dropped so a family with no host env carries no annotation.
pub fn parse_claude_env_models(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in body.lines() {
        let mut parts = line.splitn(2, '\t');
        let family = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if !family.is_empty() && !value.is_empty() {
            out.insert(family.to_string(), value.to_string());
        }
    }
    out
}

/// Parse the claude `extra_models` section (`model<TAB>value` lines) into a
/// deduplicated list of extra model ids. Empty values, family words
/// (opus/sonnet/haiku), and synthetic placeholders (empty or containing `<`)
/// are dropped. The order is preserved; dedup removes later duplicates.
pub fn parse_claude_extra_models(body: &str) -> Vec<String> {
    const FAMILIES: &[&str] = &["opus", "sonnet", "haiku"];
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        let mut parts = line.splitn(2, '\t');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if key != "model" {
            continue;
        }
        // Drop empties, synthetic placeholders, and known family words.
        if value.is_empty() || value.contains('<') || FAMILIES.contains(&value) {
            continue;
        }
        if seen.insert(value.to_string()) {
            out.push(value.to_string());
        }
    }
    out
}

/// Parse the codex `sessions` section: one
/// `mtime_ms<TAB>filename<TAB><first NDJSON line>` record per line (newest-first,
/// as emitted). Reuses `parse_codex_rollout_meta` (filename → id+project) and
/// `extract_codex_user_snippet` for the snippet. A malformed mtime or a non-
/// rollout filename skips the record. Order is preserved.
pub fn parse_remote_codex_session_list(raw: &str) -> Vec<DiscoveredSession> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.splitn(3, '\t');
        let mtime_str = parts.next().unwrap_or("");
        let filename = parts.next().unwrap_or("");
        let first_line = parts.next().unwrap_or("");
        let modified_ms: u64 = match mtime_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some((id, project)) = parse_codex_rollout_meta(filename, first_line) {
            out.push(DiscoveredSession {
                id,
                project,
                snippet: extract_codex_user_snippet(first_line),
                modified_ms,
            });
        }
    }
    out
}

/// `cd` target for a discovery script's project path, expanding a leading `~`
/// (mirrors `remote.rs::cd_target`; kept in-crate + tested so the pure builders
/// stay in the coverage layer).
fn cd_for(project: &str) -> String {
    if project == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = project.strip_prefix("~/") {
        format!("\"$HOME/{rest}\"")
    } else {
        sh_squote(project)
    }
}

// ===========================================================================
// Part 2 — temp-dir-parameterized scanners (real filesystem IO, rooted at a
// caller-supplied &Path so unit tests can point them at a temp tree)
// ===========================================================================

/// Recursively collect every file under `dir`, paired with its mtime. Callers
/// filter by name pattern afterward (`parse_codex_rollout_meta` already
/// rejects anything that isn't `rollout-*.jsonl`). An absent/unreadable `dir`
/// silently yields no files.
fn collect_jsonl_files(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
            continue;
        }
        out.extend(
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|mtime| (path, mtime)),
        );
    }
}

fn system_time_to_ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Scan `<root>/<Y>/<M>/<D>/rollout-*.jsonl` newest-first, capped at `cap`
/// files (codex-research.md §d). An absent/empty root yields an empty list —
/// never an error.
pub fn scan_codex_sessions(root: &Path, cap: usize) -> Vec<DiscoveredSession> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    files
        .into_iter()
        .take(cap)
        .filter_map(|(path, mtime)| {
            path.file_name()
                .and_then(|f| f.to_str())
                .map(str::to_string)
                .and_then(|filename| {
                    let body = std::fs::read_to_string(&path).unwrap_or_default();
                    let first_line = body.lines().next().unwrap_or("");
                    parse_codex_rollout_meta(&filename, first_line).map(|(id, project)| {
                        DiscoveredSession {
                            id,
                            project,
                            snippet: extract_codex_user_snippet(&body),
                            modified_ms: system_time_to_ms(mtime),
                        }
                    })
                })
        })
        .collect()
}

/// Enumerate `<brain_root>/<uuid>/` conversation directories (antigravity-
/// research.md §3.2/§5). Prefers `.system_generated/logs/transcript_full.jsonl`
/// over the sibling `transcript.jsonl` (double-escaping trap). `project` comes
/// from a reverse lookup of `last_conversations_json` (`{"<workspace>":
/// "<conversation-uuid>"}`) — a miss yields `None`, never a fabricated project.
/// An absent/empty root yields an empty list.
pub fn scan_agy_sessions(
    brain_root: &Path,
    last_conversations_json: &str,
) -> Vec<DiscoveredSession> {
    let reverse: HashMap<String, String> =
        serde_json::from_str::<HashMap<String, String>>(last_conversations_json)
            .unwrap_or_default()
            .into_iter()
            .map(|(workspace, conv_id)| (conv_id, workspace))
            .collect();

    let entries = match std::fs::read_dir(brain_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let out: Vec<DiscoveredSession> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let path = entry.path();
            let id = path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or_default()
                .to_string();
            let logs_dir = path.join(".system_generated").join("logs");
            let full = logs_dir.join("transcript_full.jsonl");
            let plain = logs_dir.join("transcript.jsonl");
            let content = std::fs::read_to_string(&full)
                .ok()
                .or(std::fs::read_to_string(&plain).ok())
                .unwrap_or_default();
            let snippet = agy_snippet_from_transcript(&content).unwrap_or_default();
            let project = reverse.get(&id).cloned();
            let modified_ms = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(system_time_to_ms)
                .unwrap_or(0);
            DiscoveredSession {
                id,
                project,
                snippet,
                modified_ms,
            }
        })
        .collect();
    out
}

/// Encode a project's absolute path into claude's on-disk directory name:
/// every `/` becomes `-` (claude stores sessions at
/// `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`). Pure; no IO.
pub fn encode_claude_project_dir(project_abs: &str) -> String {
    project_abs.replace('/', "-")
}

/// Extract the first user-message text from a claude session `.jsonl` body.
/// claude's transcript lines are `{"type":"user","message":{"role":"user",
/// "content":...}}` where content is either a plain string or an array of
/// content blocks. Returns an empty string when nothing is cheaply
/// extractable (never a parse error).
fn claude_snippet_from_body(body: &str) -> String {
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let content = match v.get("message").and_then(|m| m.get("content")) {
            Some(c) => c,
            None => continue,
        };
        // Plain-string content.
        if let Some(s) = content.as_str() {
            return s.trim().to_string();
        }
        // Array-of-blocks content: first block with a `text` field.
        if let Some(arr) = content.as_array() {
            for block in arr {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    return t.trim().to_string();
                }
            }
        }
    }
    String::new()
}

/// List claude resumable sessions for one `project` under `projects_root`
/// (`~/.claude/projects`). Sessions live at
/// `<projects_root>/<encoded-cwd>/<uuid>.jsonl` — the encoded dir is the
/// project abs path with `/`→`-`. Returns each `*.jsonl` newest-first, capped
/// at `cap`, as `DiscoveredSession { id = filename stem (uuid), project,
/// snippet = first user message text, modified }`. An absent/empty project
/// directory yields an empty list — never an error.
pub fn scan_claude_sessions(
    projects_root: &Path,
    project: &str,
    cap: usize,
) -> Vec<DiscoveredSession> {
    let dir = projects_root.join(encode_claude_project_dir(project));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("jsonl")
        })
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, mtime))
        })
        .collect();
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    files
        .into_iter()
        .take(cap)
        .filter_map(|(path, mtime)| {
            let id = path.file_stem().and_then(|s| s.to_str())?.to_string();
            if id.is_empty() {
                return None;
            }
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            Some(DiscoveredSession {
                id,
                project: Some(project.to_string()),
                snippet: claude_snippet_from_body(&body),
                modified_ms: system_time_to_ms(mtime),
            })
        })
        .collect()
}

// ===========================================================================
// Part 3 — the CapabilityProbe seam (§3.1)
// ===========================================================================

/// One driver's discovery logic. Implementations are BLOCKING and are always
/// invoked on a dedicated spawned thread (§4) — never the serve loop, never
/// under any agent mutex.
pub trait CapabilityProbe: Send + Sync + 'static {
    fn driver(&self) -> &'static str;
    /// Whether Marsad has a real LAUNCH DRIVER for this agent — a static,
    /// per-driver fact, NOT a probe of the host. Defaults to `false`; drivers
    /// Marsad can actually spawn (claude/codex) override it to `true`. This is
    /// what the launch UI gates on (a launchable driver is offered on every
    /// host; bad host setup surfaces as a launch error, not a greyed button),
    /// distinct from `cli_available` (a best-effort local `which`).
    fn launchable(&self) -> bool {
        false
    }
    /// Full probe for one host: availability + models + sessions for the given
    /// scope. Must respect `ctx.deadline` internally; errors go into
    /// `Capabilities::errors`, never panic/propagate.
    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities;
}

/// A host's launch configuration, carried from core's registry into the probe so
/// discovery runs setup-first over the right connection with the exact per-driver
/// command a launch would use (deliverable #1). `Default` = the local host with no
/// setup and no overrides (bare `claude`/`codex`/`agy`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostLaunchCfg {
    /// The remote execution target (`user@host`, `wsl:<distro>`, …). `None` = the
    /// agent's LOCAL machine (discovery runs in-process; setup still honored).
    pub host_target: Option<String>,
    /// A shell pre-command run BEFORE every discovery probe on this host.
    pub setup: Option<String>,
    /// Per-driver command overrides (`(driver, command)`).
    pub agent_commands: Vec<(String, String)>,
    /// The `claude` executable override (fallback for the `claude` driver).
    pub claude_bin: Option<String>,
}

impl HostLaunchCfg {
    /// Resolve the concrete command line for `driver`: `agent_commands[driver]`
    /// first, then `claude_bin` for the claude driver, else the bare driver name.
    /// Mirrors `marsad_hosts::resolve_driver_bin` (kept in-crate to avoid a new
    /// dependency edge, and unit-tested here).
    pub fn resolve_command(&self, driver: &str) -> String {
        if let Some((_, cmd)) = self.agent_commands.iter().find(|(d, _)| d == driver) {
            return cmd.clone();
        }
        if driver == "claude" {
            if let Some(bin) = &self.claude_bin {
                return bin.clone();
            }
        }
        driver.to_string()
    }
}

pub struct HostProbeCtx {
    pub host_id: String,
    /// None = local; Some = SSH/WSL target (exec via run_remote_script_bounded).
    pub host_target: Option<String>,
    /// The host's pre-command, run FIRST in every discovery script (real env).
    pub setup: Option<String>,
    /// The host's per-driver command overrides (`(driver, command)`), used to
    /// resolve the exact command each probe discovers against.
    pub agent_commands: Vec<(String, String)>,
    /// The host's `claude` executable override (fallback for the claude driver).
    pub claude_bin: Option<String>,
    /// Session scope to scan (claude: accumulated requested projects).
    pub scope: SessionScope,
    /// The SPECIFIC project this refresh was requested for (claude session
    /// enumeration scopes to exactly one `<encoded-cwd>` directory). `None` for
    /// AllKnown drivers / a project-less refresh. Distinct from `scope`, which
    /// is the accumulated superset across every refresh for this (driver,host).
    pub project: Option<String>,
    /// Hard wall-clock budget (default 6s — matches the existing bounded-probe
    /// convention, agent/src/lib.rs:1440-1490).
    pub deadline: Duration,
}

impl HostProbeCtx {
    /// Resolve the concrete command line a probe discovers against for `driver`
    /// (see `HostLaunchCfg::resolve_command`).
    pub fn resolve_command(&self, driver: &str) -> String {
        if let Some((_, cmd)) = self.agent_commands.iter().find(|(d, _)| d == driver) {
            return cmd.clone();
        }
        if driver == "claude" {
            if let Some(bin) = &self.claude_bin {
                return bin.clone();
            }
        }
        driver.to_string()
    }
}

// ===========================================================================
// Part 4 — CapabilityProvider (§3.2/§3.3/§3.4, §4)
// ===========================================================================

// Deadline for a host with NO setup command: SSH connect (bounded separately by
// ConnectTimeout=10) + a trivial discovery script. Kept snappy so startup isn't
// sluggish when some hosts are down/slow (unreachable ones fail at ConnectTimeout
// regardless). `run_remote_script_bounded` returns None on timeout, dropping ALL
// sections (→ "Claude Code isn't available") even for a reachable host, so this
// must still comfortably cover a reachable host's fast path.
const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_secs(10);

// Longer deadline for a host WITH a setup command: `source ~/module.sh` on an HPC
// login node can take tens of seconds (measured ~58s on one) before the discovery
// script even starts, and it re-pays that on every fresh (non-multiplexed) SSH
// connection. This is a CEILING, not a fixed cost — fast hosts return early — so
// only genuinely-slow login hosts use the window. It costs no user-visible
// finickiness because the host shows "Connecting" until discovery lands (see
// Core::set_host_status), so a long-but-completing probe is honest, not a timeout.
// run_remote_script_bounded logs the real elapsed time so this is tuned from data.
const SETUP_PROBE_DEADLINE: Duration = Duration::from_secs(90);

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedHostFile {
    /// driver -> last Capabilities snapshot for this host.
    #[serde(default)]
    capabilities: HashMap<String, Capabilities>,
    /// The claude observed-models superset (§3.4): survives even if a later
    /// snapshot narrows.
    #[serde(default)]
    observed_claude_models: Vec<ModelInfo>,
}

struct Inner {
    probes: Vec<Box<dyn CapabilityProbe>>,
    data_root: PathBuf,
    cache: Mutex<HashMap<(String, String), Capabilities>>,
    scopes: Mutex<HashMap<(String, String), SessionScope>>,
    observed_models: Mutex<HashMap<String, Vec<ModelInfo>>>,
    subscribers: Mutex<Vec<mpsc::Sender<String>>>,
    in_flight: Mutex<HashSet<(String, String)>>,
}

/// The capability provider: owns the (driver, host) cache, dedupes in-flight
/// probes, persists to `<data_root>/capabilities/<host_id>.json`, and
/// broadcasts `CapabilitiesUpdate` lines to subscribers. Every public method
/// is non-blocking — probe IO always runs on a spawned thread (§4).
pub struct CapabilityProvider {
    inner: Arc<Inner>,
}

impl CapabilityProvider {
    /// Load any persisted `<data_root>/capabilities/<host>.json` files
    /// synchronously, then spawn a background full refresh (§3.2 trigger 1) —
    /// serving starts immediately; nothing waits on probes.
    pub fn new(
        probes: Vec<Box<dyn CapabilityProbe>>,
        known_hosts: Vec<String>,
        data_root: &Path,
    ) -> Self {
        let mut cache = HashMap::new();
        let mut observed_models = HashMap::new();

        let caps_dir = data_root.join("capabilities");
        if let Ok(entries) = std::fs::read_dir(&caps_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let host_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string);
                // Corrupt/missing file => start empty (never an error, §3.4).
                let file = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<PersistedHostFile>(&raw).ok());
                if let (Some(host_id), Some(file)) = (host_id, file) {
                    for (driver, caps) in file.capabilities {
                        cache.insert((driver, host_id.clone()), caps);
                    }
                    if !file.observed_claude_models.is_empty() {
                        observed_models.insert(host_id, file.observed_claude_models);
                    }
                }
            }
        }

        let inner = Arc::new(Inner {
            probes,
            data_root: data_root.to_path_buf(),
            cache: Mutex::new(cache),
            scopes: Mutex::new(HashMap::new()),
            observed_models: Mutex::new(observed_models),
            subscribers: Mutex::new(Vec::new()),
            in_flight: Mutex::new(HashSet::new()),
        });

        let provider = CapabilityProvider { inner };

        for host in known_hosts {
            // Startup refresh for known hosts. `known_hosts` here is only ever the
            // local host (serve.rs wires `["local"]`); remote hosts arrive with
            // their full launch config via explicit RefreshCapabilities. So the
            // local startup probe uses the default (local, no-setup) config.
            provider.handle_refresh(Some(host), None, None, HostLaunchCfg::default());
        }

        provider
    }

    /// Register a new subscriber; replays every cached `CapabilitiesUpdate`
    /// line to it before returning (§3.2 trigger 2), then it streams live
    /// broadcasts. Never touches IO beyond the in-memory cache lock.
    pub fn subscribe(&self) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        {
            let cache = self.inner.cache.lock().unwrap();
            for caps in cache.values() {
                let line = encode_ipc(&IpcMsg::CapabilitiesUpdate {
                    host_id: caps.host_id.clone(),
                    driver: caps.driver.clone(),
                    capabilities: caps.clone(),
                });
                let _ = tx.send(line);
            }
        }
        self.inner.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Returns the encoded `CapabilitiesRefreshQueued` ack immediately; queues
    /// matching probes on background threads (§3.2 trigger 3, §4). `None`
    /// driver means "every registered driver"; `None` host defaults to `"local"`.
    pub fn handle_refresh(
        &self,
        host_id: Option<String>,
        driver: Option<String>,
        project: Option<String>,
        cfg: HostLaunchCfg,
    ) -> String {
        let host = host_id.as_deref().unwrap_or("local").to_string();
        let drivers: Vec<String> = match &driver {
            Some(d) => vec![d.clone()],
            None => self
                .inner
                .probes
                .iter()
                .map(|p| p.driver().to_string())
                .collect(),
        };

        for d in drivers {
            self.queue_probe(host.clone(), d, project.clone(), cfg.clone());
        }

        encode_ipc(&IpcMsg::CapabilitiesRefreshQueued { host_id })
    }

    /// Queue one (driver, host) probe. Accumulates `project` into the host's
    /// `SessionScope::Projects` set (claude) REGARDLESS of in-flight dedup (an
    /// AllKnown driver's probe simply ignores the accumulated scope). A probe
    /// already running for this (driver, host), or a driver with no registered
    /// probe, is skipped — the imminent broadcast (if any) serves the requester.
    fn queue_probe(
        &self,
        host_id: String,
        driver: String,
        project: Option<String>,
        cfg: HostLaunchCfg,
    ) {
        let key = (driver.clone(), host_id.clone());

        let scope = {
            let mut scopes = self.inner.scopes.lock().unwrap();
            let entry = scopes.entry(key.clone()).or_insert(SessionScope::AllKnown);
            if let Some(p) = &project {
                match entry {
                    SessionScope::Projects(v) => {
                        v.push(p.clone());
                    }
                    SessionScope::AllKnown => {
                        *entry = SessionScope::Projects(vec![p.clone()]);
                    }
                }
            }
            entry.clone()
        };

        // A driver with no registered probe, or one already in flight, yields
        // no index to spawn — both collapse into the same "nothing to do" arm.
        let probe_idx = self
            .inner
            .probes
            .iter()
            .position(|p| p.driver() == driver)
            .and_then(|idx| {
                let mut in_flight = self.inner.in_flight.lock().unwrap();
                in_flight.insert(key.clone()).then_some(idx)
            });
        let probe_idx = match probe_idx {
            Some(i) => i,
            None => return,
        };

        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            // `host_target` comes from the launch config (the real shell target),
            // NOT from `host_id` (an opaque registry key). A `None` target means
            // the agent's local machine — discovery runs in-process (setup still
            // honored). Back-compat: if no target was threaded but the host_id is
            // itself a usable target (the old convention: host_id == target for a
            // non-"local" host), fall back to it so a caller that only sent a
            // host_id still probes the right machine.
            let host_target = cfg
                .host_target
                .clone()
                .or_else(|| Some(host_id.clone()).filter(|h| h != "local"));
            // Per-host deadline: a heavy `setup` (e.g. `source ~/module.sh` on an
            // HPC module system) legitimately takes many seconds, so those hosts
            // get the long window; everything else fails fast so startup isn't
            // sluggish. Unreachable hosts already fail in ~ConnectTimeout (10s)
            // regardless — the deadline only bounds a REACHABLE-but-slow script.
            let has_setup = cfg
                .setup
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let deadline = if has_setup {
                SETUP_PROBE_DEADLINE
            } else {
                DEFAULT_PROBE_DEADLINE
            };
            let ctx = HostProbeCtx {
                host_id: host_id.clone(),
                host_target,
                setup: cfg.setup.clone(),
                agent_commands: cfg.agent_commands.clone(),
                claude_bin: cfg.claude_bin.clone(),
                scope,
                project,
                deadline,
            };
            let mut caps = inner.probes[probe_idx].probe(&ctx);

            // §3.4: the claude probe builds a family catalog, but the passively
            // observed-model superset lives in the provider — merge it in so a
            // probe never DROPS a model the user has actually run (append,
            // dedup by id, family entries stay first).
            if driver == "claude" {
                let observed = inner
                    .observed_models
                    .lock()
                    .unwrap()
                    .get(&host_id)
                    .cloned()
                    .unwrap_or_default();
                merge_observed_into_catalog(&mut caps.models, &observed);
            }

            inner
                .cache
                .lock()
                .unwrap()
                .insert(key.clone(), caps.clone());
            persist_host(&inner, &host_id);
            broadcast_update(&inner, &caps);

            inner.in_flight.lock().unwrap().remove(&key);
        });
    }

    /// Lock-clone-return cache read (no IO), mirrors `Core::capabilities()`.
    pub fn snapshot(&self, driver: &str, host_id: &str) -> Option<Capabilities> {
        self.inner
            .cache
            .lock()
            .unwrap()
            .get(&(driver.to_string(), host_id.to_string()))
            .cloned()
    }

    /// Passive claude-model observation (§3.2 trigger 4): accumulates into the
    /// host's `Observed` model set, persists, and broadcasts.
    pub fn observe_claude_model(&self, host_id: &str, model: &str) {
        let key = ("claude".to_string(), host_id.to_string());

        let models_vec = {
            let mut observed = self.inner.observed_models.lock().unwrap();
            let entry = observed.entry(host_id.to_string()).or_default();
            if !entry.iter().any(|m| m.id == model) {
                entry.push(ModelInfo {
                    id: model.to_string(),
                    annotation: None,
                });
            }
            entry.clone()
        };

        let caps = {
            let mut cache = self.inner.cache.lock().unwrap();
            let mut caps = cache.get(&key).cloned().unwrap_or_else(|| Capabilities {
                driver: "claude".to_string(),
                host_id: host_id.to_string(),
                cli_available: false,
                // claude has a launch driver — static truth, independent of any
                // host probe (mirrors RealClaudeProbe::launchable()).
                launchable: true,
                models: ModelCatalog::default(),
                session_scope: None,
                sessions: vec![],
                fetched_at_ms: 0,
                errors: vec![],
            });
            // Observed-only set (contract: this is the passively-observed
            // superset, persisted across restarts). The family list
            // (opus/sonnet/haiku) is contributed by `RealClaudeProbe` and
            // re-merged with this observed set on every refresh (queue_probe),
            // which the launch modals trigger on open — so the picker always
            // shows families PLUS anything ever observed.
            caps.models = ModelCatalog {
                models: models_vec,
                source: ModelListSource::Observed,
                free_text: true,
            };
            cache.insert(key, caps.clone());
            caps
        };

        persist_host(&self.inner, host_id);
        broadcast_update(&self.inner, &caps);
    }
}

fn broadcast_update(inner: &Inner, caps: &Capabilities) {
    let line = encode_ipc(&IpcMsg::CapabilitiesUpdate {
        host_id: caps.host_id.clone(),
        driver: caps.driver.clone(),
        capabilities: caps.clone(),
    });
    let subs = inner.subscribers.lock().unwrap();
    for s in subs.iter() {
        let _ = s.send(line.clone());
    }
}

fn persist_host(inner: &Inner, host_id: &str) {
    let dir = inner.data_root.join("capabilities");
    let _ = std::fs::create_dir_all(&dir);
    let capabilities_map: HashMap<String, Capabilities> = inner
        .cache
        .lock()
        .unwrap()
        .iter()
        .filter(|((_, h), _)| h == host_id)
        .map(|((driver, _), caps)| (driver.clone(), caps.clone()))
        .collect();
    let observed_claude_models = inner
        .observed_models
        .lock()
        .unwrap()
        .get(host_id)
        .cloned()
        .unwrap_or_default();
    let file = PersistedHostFile {
        capabilities: capabilities_map,
        observed_claude_models,
    };
    let json = serde_json::to_string_pretty(&file).unwrap_or_default();
    let _ = std::fs::write(dir.join(format!("{host_id}.json")), json);
}

// ===========================================================================
// Part 5 — real, subprocess/$HOME-rooted probe implementations. Production
// only: never unit-tested (no live `codex`/`agy`/`claude` on the test
// machine, per the house guardrail — the `run` fixture-driven live suites
// prove these against the real CLIs). Compiled out under coverage so the
// crate's 100% bar stays on the pure/logic layer (§3.1: "Real subprocess/
// FS-rooted-at-$HOME paths follow the established RealGit/RealTranscript
// pattern").
// ===========================================================================

/// Run a discovery SCRIPT over the host connection, setup-first, and return its
/// raw stdout — or `Err` on an unreachable host / a timeout / a spawn failure.
///
/// - Remote host (`Some(target)`): `run_remote_script_bounded` pipes the script
///   to `bash -l -s` over ssh/wsl/plink (login shell so `$HOME`/PATH/setup env
///   resolve), bounded by `deadline`. `None` back from it = host unreachable /
///   the bounded probe timed out.
/// - Local host (`None`): the SAME script is fed to a LOCAL `bash -l -s` (so
///   `setup` runs identically), bounded by the same deadline. This is what makes
///   local discovery real (setup honored) rather than a bare `which`.
#[cfg(not(coverage))]
fn run_discovery(
    host_target: Option<&str>,
    script: &str,
    deadline: Duration,
) -> Result<String, String> {
    match host_target {
        Some(target) => crate::remote::run_remote_script_bounded(target, script, deadline)
            .ok_or_else(|| format!("host '{target}' unreachable or probe timed out")),
        None => run_local_script(script, deadline),
    }
}

/// Run a shell `script` on the LOCAL machine via `bash -l -s` (login shell,
/// script on stdin), bounded by `deadline`. Mirrors `run_remote_script_bounded`
/// applied to a local child so `setup` + section markers behave identically to
/// the remote path.
#[cfg(not(coverage))]
fn run_local_script(script: &str, deadline: Duration) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("bash spawn failed: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "bash: no stdin".to_string())?;
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| format!("bash: write failed: {e}"))?;
    } // stdin dropped → EOF so `bash -s` runs and exits.

    // Drain stdout on a dedicated thread so a LARGE output (e.g. `codex debug
    // models` is ~200 KB JSON) can never fill the OS pipe buffer and DEADLOCK
    // the child — the child would block on write, never exit, and the poll below
    // would falsely time out. Reading concurrently keeps the pipe flowing.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut so) = stdout {
            let _ = so.read_to_string(&mut out);
        }
        out
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return Ok(reader.join().unwrap_or_default());
            }
            Ok(None) => {
                if start.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err("local discovery: timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("bash: wait failed: {e}")),
        }
    }
}

/// Real codex probe: setup-first `command -v` + `<cmd> debug models` +
/// `~/.codex/sessions` listing, run over the host connection (remote) or locally
/// (setup honored). No `which`; no local-only optimism.
#[cfg(not(coverage))]
pub struct RealCodexProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealCodexProbe {
    fn driver(&self) -> &'static str {
        "codex"
    }

    fn launchable(&self) -> bool {
        true
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("codex");
        let script = build_codex_discovery_script(&command, ctx.setup.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models, sessions) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    (
                        parse_avail(secs.get("avail")),
                        parse_codex_debug_models(secs.get("models")),
                        parse_remote_codex_session_list(secs.get("sessions")),
                    )
                }
                Err(e) => {
                    errors.push(format!("codex discovery: {e}"));
                    (false, vec![], vec![])
                }
            };
        Capabilities {
            driver: "codex".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: true,
            },
            session_scope: Some(SessionScope::AllKnown),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Real agy/gemini probe: setup-first `command -v` + `<cmd> models`, run over
/// the host connection (remote) or locally (setup honored).
#[cfg(not(coverage))]
pub struct RealAgyProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealAgyProbe {
    fn driver(&self) -> &'static str {
        "gemini"
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("gemini");
        let script = build_agy_discovery_script(&command, ctx.setup.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    (
                        parse_avail(secs.get("avail")),
                        parse_agy_models(secs.get("models")),
                    )
                }
                Err(e) => {
                    errors.push(format!("gemini discovery: {e}"));
                    (false, vec![])
                }
            };
        Capabilities {
            driver: "gemini".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            // gemini (agy) has NO launch driver in Marsad — the launch UI greys
            // it with the "not yet launchable" note. Uses the trait default.
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: false,
            },
            session_scope: Some(SessionScope::AllKnown),
            sessions: vec![],
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Real claude probe: setup-first `command -v` (real availability, local AND
/// remote) + the host's `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` env
/// (claude has no "list models" command — the env is the only truthful per-host
/// source), offered as the pickable `opus/sonnet/haiku` families annotated with
/// the host's resolved versions where set (`free_text: true`). Sessions are
/// enumerated per requested project over the connection. The provider re-merges
/// the passively-observed model superset afterward (§3.4).
#[cfg(not(coverage))]
pub struct RealClaudeProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealClaudeProbe {
    fn driver(&self) -> &'static str {
        "claude"
    }

    fn launchable(&self) -> bool {
        true
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("claude");
        let script =
            build_claude_discovery_script(&command, ctx.setup.as_deref(), ctx.project.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models, sessions) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    // Family catalog annotated with the host's REAL resolved
                    // versions from the env section (families are only offered
                    // when claude actually resolved on the host).
                    let resolved = parse_claude_env_models(secs.get("env"));
                    // Extra models from env overrides, config files, and transcripts.
                    let extra_ids = parse_claude_extra_models(secs.get("extra_models"));
                    let extra_observed: Vec<ModelInfo> = extra_ids
                        .into_iter()
                        .map(|id| ModelInfo { id, annotation: None })
                        .collect();
                    let models = if parse_avail(secs.get("avail")) {
                        let resolved_ref: HashMap<&str, String> =
                            resolved.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                        build_claude_model_catalog(&resolved_ref, &extra_observed)
                    } else {
                        // claude didn't resolve on the host: no families offered
                        // (the passively-observed superset is still merged by the
                        // provider afterward, so nothing ever run is lost).
                        vec![]
                    };
                    (
                        parse_avail(secs.get("avail")),
                        models,
                        parse_remote_claude_session_list(secs.get("sessions")),
                    )
                }
                Err(e) => {
                    errors.push(format!("claude discovery: {e}"));
                    (false, vec![], vec![])
                }
            };
        Capabilities {
            driver: "claude".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Observed,
                free_text: true,
            },
            session_scope: Some(ctx.scope.clone()),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

#[cfg(not(coverage))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
