//! Pure capability parse functions (no IO) — model catalogs, rollout/transcript metadata, remote session lists.
//!
//! Extracted from `capabilities.rs` verbatim; `use super::*` inherits the
//! parent module's imports and cross-section helpers.

use super::*;

// Task B pure seams use wasit types directly.
// `wasit_codex` and `wasit_opencode` are workspace deps of marsad-agent.

// ===========================================================================
// Part 1 — pure parse functions (no IO)
// ===========================================================================

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
/// "hide"`) are excluded. The numeric `priority` is codex's ORDERING key, not a
/// display label — it is used to sort the list ascending (lower priority first;
/// models with no priority sort last, in their original relative order) and is
/// NEVER surfaced as a picker annotation. codex models carry no meaningful
/// per-model display word (unlike claude's family), so `annotation` is always
/// `None`. Malformed JSON or an absent/empty `models` array degrades to an empty
/// list — never a panic (the caller surfaces the real subprocess failure into
/// `Capabilities.errors` instead).
pub fn parse_codex_debug_models(raw: &str) -> Vec<ModelInfo> {
    let mut entries: Vec<CodexModelEntry> = serde_json::from_str::<CodexModelsFile>(raw)
        .unwrap_or_default()
        .models
        .into_iter()
        .filter(|m| m.visibility != "hide")
        .collect();
    // Stable sort on priority ascending keeps un-prioritized (None → i64::MAX)
    // models last while preserving their input order among equals.
    entries.sort_by_key(|m| m.priority.unwrap_or(i64::MAX));
    entries
        .into_iter()
        .map(|m| ModelInfo {
            id: m.slug,
            annotation: None,
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
pub(crate) fn extract_codex_user_snippet(body: &str) -> String {
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

// ===========================================================================
// Task B — pure parse seams for connect-time mode discovery
// (unit-testable wrappers over the wasit-codex and wasit-opencode functions;
//  the real subprocess handshake lives in real_probes.rs under cfg(not(coverage)))
// ===========================================================================

/// Parse a `collaborationMode/list` JSON-RPC RESPONSE body (the `result`
/// object) into a vec of `SessionMode`s.
///
/// Wire shape: `{"data":[{"mode":"<id>","settings":{...}}, ...]}`
/// This is the pure seam that `RealCodexProbe` uses after capturing the
/// response from a bounded `codex app-server` handshake.
/// Delegates to `wasit_codex::codex_modes_from_collab_list` — no
/// reimplementation.
///
/// `raw_result_json` is the serialized `result` object from the JSON-RPC
/// response line `{"id":5,"result":{…}}`. Returns empty on any parse
/// failure — never a panic.
pub fn parse_codex_collab_list_response(raw_result_json: &str) -> Vec<marsad_proto::SessionMode> {
    let val: serde_json::Value = match serde_json::from_str(raw_result_json) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    wasit_codex::codex_modes_from_collab_list(&val)
}

/// Parse the `configOptions` container from an opencode `session/new` or
/// `session/load` result (a full serialized JSON value) into
/// `(available_modes, current_mode_id)`.
///
/// This is the pure seam that `RealOpencodeProbe` uses after capturing the
/// init response from a bounded `opencode acp` handshake. Delegates to
/// `wasit_opencode::parse_config_modes` — no reimplementation.
///
/// `raw_result_json` is the serialized result VALUE (NOT a full JSON-RPC
/// line — just the result object). Returns `(vec![], String::new())` on
/// any parse failure — never a panic.
pub fn parse_opencode_config_modes(
    raw_result_json: &str,
) -> (Vec<marsad_proto::SessionMode>, String) {
    let val: serde_json::Value = match serde_json::from_str(raw_result_json) {
        Ok(v) => v,
        Err(_) => return (vec![], String::new()),
    };
    wasit_opencode::parse_config_modes(&val)
}

// ===========================================================================
// Task D — pure filter for project-scoped codex session discovery
// ===========================================================================

/// Filter a slice of `DiscoveredSession`s to only those whose `project`
/// matches `requested_project` (honest path-equality; both sides trim
/// trailing slashes). Sessions with `project: None` are excluded when a
/// project is requested — they lack cwd information and cannot be
/// attributed to a specific project.
///
/// Pure; no IO. Used by `RealCodexProbe` to scope the session list to the
/// requested project, mirroring how `RealClaudeProbe` stamps sessions with
/// `ctx.project`.
pub fn filter_codex_sessions_by_project(
    sessions: Vec<DiscoveredSession>,
    requested_project: &str,
) -> Vec<DiscoveredSession> {
    let want = requested_project.trim_end_matches('/');
    sessions
        .into_iter()
        .filter(|s| {
            s.project
                .as_deref()
                .map(|p| p.trim_end_matches('/') == want)
                .unwrap_or(false)
        })
        .collect()
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

