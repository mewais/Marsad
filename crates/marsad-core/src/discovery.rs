//! Session discovery — enumerate existing Claude Code sessions on disk.
//!
//! Claude stores sessions at `~/.claude/projects/<ENCODED_CWD>/<UUID>.jsonl`.
//! `ENCODED_CWD` is the absolute project path with every character that is NOT
//! in `[A-Za-z0-9]` replaced by `-`.
//!
//! Each `<UUID>.jsonl` file contains NDJSON lines; the UUID (filename stem) is
//! the Claude session id usable with `--resume`.  The first line whose `"type"`
//! field equals `"user"` carries the session's first prompt in
//! `message.content[].text`.

use std::path::Path;

use marsad_events::TranscriptEntry;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// LiteLLM pricing DB (embedded at compile time)
// ---------------------------------------------------------------------------

/// Raw per-token costs for one model entry in the LiteLLM pricing JSON.
#[derive(Deserialize)]
struct LiteLlmEntry {
    input_cost_per_token: f64,
    output_cost_per_token: f64,
    #[serde(default)]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    cache_creation_input_token_cost: Option<f64>,
}

/// Resolved $/million-token rates after applying the claude default conventions
/// for missing cache fields (cache_read = 0.1×input, cache_write = 1.25×input).
#[derive(Clone, Copy)]
struct LiteLlmRate {
    /// Input tokens $/Mtok.
    input: f64,
    /// Output tokens $/Mtok.
    output: f64,
    /// Cache read tokens $/Mtok.
    cache_read: f64,
    /// Cache write (creation) tokens $/Mtok.
    cache_write: f64,
}

impl LiteLlmRate {
    fn from_entry(e: &LiteLlmEntry) -> Self {
        let input = e.input_cost_per_token * 1_000_000.0;
        let output = e.output_cost_per_token * 1_000_000.0;
        let cache_read = e
            .cache_read_input_token_cost
            .map(|v| v * 1_000_000.0)
            .unwrap_or(input * 0.1);
        let cache_write = e
            .cache_creation_input_token_cost
            .map(|v| v * 1_000_000.0)
            .unwrap_or(input * 1.25);
        LiteLlmRate {
            input,
            output,
            cache_read,
            cache_write,
        }
    }
}

/// The compiled-in LiteLLM claude/anthropic pricing subset.
static LITELLM_JSON: &str = include_str!("litellm_pricing.json");

/// Parsed pricing map, initialised once and reused.
static LITELLM_MAP: std::sync::OnceLock<std::collections::HashMap<String, LiteLlmRate>> =
    std::sync::OnceLock::new();

fn litellm_map() -> &'static std::collections::HashMap<String, LiteLlmRate> {
    LITELLM_MAP.get_or_init(|| {
        let raw: std::collections::HashMap<String, LiteLlmEntry> =
            serde_json::from_str(LITELLM_JSON).unwrap_or_default();
        raw.into_iter()
            .map(|(k, v)| (k, LiteLlmRate::from_entry(&v)))
            .collect()
    })
}

/// Strip provider prefix (e.g. `"anthropic/"`, `"azure_ai/"`) from a model key
/// so bare names like `"claude-sonnet-4-6"` are tried too.
fn strip_provider_prefix(key: &str) -> &str {
    // Keys look like: "anthropic/claude-sonnet-4-6", "azure_ai/claude-opus-4-8",
    // "openrouter/anthropic/claude-opus-4.5", "claude-opus-4-8" (already bare).
    if let Some(pos) = key.rfind('/') {
        &key[pos + 1..]
    } else {
        key
    }
}

/// Normalise a model id as seen in Claude Code transcripts into a bare
/// LiteLLM-style key:
///   1. Strip a trailing `[…]` suffix (e.g. `[1m]`, `[128k]`).
///   2. Strip any provider prefix like `"anthropic/"`.
///   3. Lowercase.
fn normalize_model_id(model: &str) -> String {
    // Strip trailing [...] suffix (e.g. "[1m]", "[128k]")
    let s = if let Some(bracket) = model.find('[') {
        &model[..bracket]
    } else {
        model
    };
    let s = s.trim_end_matches('-').trim();
    // Strip provider prefix
    let s = strip_provider_prefix(s);
    s.to_lowercase()
}

/// Look up $/million-token rates for `model` from the LiteLLM pricing DB.
///
/// Matching order (first hit wins):
/// (a) Exact key (case-sensitive as stored).
/// (b) Normalized key — strip `[1m]`/`[…]` suffix, strip provider prefix, lowercase.
/// (c) Normalized key with dots replaced by dashes (`claude-sonnet-4.6` → `claude-sonnet-4-6`).
/// (d) Family-major-minor prefix fallback: pick the lexicographically greatest
///     key whose bare name starts with `claude-<family>-<major>-<minor>`.
///     This handles date-suffixed variants (e.g. `claude-opus-4-8-20260604`).
///     NOTE: this only matches keys with the SAME family/major/minor triple, so
///     `claude-opus-4-0` (absent from LiteLLM) won't accidentally inherit opus-4-8
///     pricing — that model falls through to the hand-written table.
///
/// Returns `None` if no entry is found anywhere (caller falls back to the
/// hand-written table).
pub fn litellm_rate(model: &str) -> Option<(f64, f64, f64, f64)> {
    let map = litellm_map();

    // (a) exact key
    if let Some(r) = map.get(model) {
        return Some((r.input, r.output, r.cache_read, r.cache_write));
    }

    // (b) normalised (strip suffix + prefix + lowercase)
    let norm = normalize_model_id(model);
    if let Some(r) = map.get(norm.as_str()) {
        return Some((r.input, r.output, r.cache_read, r.cache_write));
    }

    // (c) dots → dashes
    let dashed = norm.replace('.', "-");
    if let Some(r) = map.get(dashed.as_str()) {
        return Some((r.input, r.output, r.cache_read, r.cache_write));
    }

    // (d) family-major-minor prefix: e.g. "claude-opus-4-8", "claude-sonnet-4-6".
    // Split on "-" and require at least 4 parts (claude / family / major / minor).
    // This intentionally does NOT fall back to a family+major prefix, because that
    // could return wrong pricing for older versions absent from LiteLLM (e.g.
    // claude-opus-4-0 would otherwise inherit opus-4-8 rates instead of the
    // correct $15/$75 hand-written value).
    let parts: Vec<&str> = dashed.split('-').collect();
    if parts.len() >= 4 && parts[0] == "claude" {
        let prefix4 = format!("{}-{}-{}-{}", parts[0], parts[1], parts[2], parts[3]);
        let best4 = map
            .keys()
            .filter(|k| {
                let bare = strip_provider_prefix(k).to_lowercase();
                bare.starts_with(&prefix4)
            })
            .max_by(|a, b| a.cmp(b));
        if let Some(key) = best4 {
            let r = &map[key];
            return Some((r.input, r.output, r.cache_read, r.cache_write));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A summary of one discoverable Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The Claude session id (UUID filename stem). Pass to `--resume`.
    pub id: String,
    /// The first user prompt, trimmed and truncated to ~100 chars.
    pub snippet: String,
    /// File mtime as Unix epoch **milliseconds**.
    pub modified: u64,
}

// ---------------------------------------------------------------------------
// encode_project_dir
// ---------------------------------------------------------------------------

/// Encode an absolute project directory path into the Claude projects-root
/// subdirectory name.
///
/// Every character that is **not** ASCII alphanumeric (`[A-Za-z0-9]`) is
/// replaced by `-`.
///
/// ```
/// use marsad_core::discovery::encode_project_dir;
/// assert_eq!(encode_project_dir("/work/mewais/Marsad"), "-work-mewais-Marsad");
/// assert_eq!(encode_project_dir("/tmp/marsad-cont-3lx1an_6/project"),
///            "-tmp-marsad-cont-3lx1an-6-project");
/// ```
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ---------------------------------------------------------------------------
// first_user_prompt
// ---------------------------------------------------------------------------

/// Minimal serde shapes for parsing a JSONL line just enough to find the first
/// user message content.
#[derive(Deserialize)]
struct JsonlLine {
    #[serde(rename = "type")]
    line_type: Option<String>,
    message: Option<JsonlMessage>,
    // Present on `system/compact_boundary` lines.
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default, rename = "compact_metadata")]
    compact_metadata: Option<JsonlCompactMetadata>,
    /// Per-entry ISO-8601 UTC time claude records on every JSONL line
    /// (e.g. "2026-06-01T20:03:03.967Z"). Parsed to epoch-ms for the transcript
    /// timestamp so a resumed session's history shows real day separators/times.
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct JsonlCompactMetadata {
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    cumulative_dropped_tokens: u64,
}

#[derive(Deserialize)]
struct JsonlMessage {
    content: Option<JsonlContentField>,
}

/// Claude records `message.content` as EITHER a bare string (simple user/assistant
/// messages) OR an array of typed content blocks (rich messages). Typing it only
/// as the array form made a string-content line FAIL to deserialize into
/// `JsonlLine`, so it was silently skipped — dropping those prompts/replies from
/// the transcript entirely (the root cause of "response shows but my prompt
/// doesn't" for sessions whose prompt was stored as a plain string).
#[derive(Deserialize)]
#[serde(untagged)]
enum JsonlContentField {
    Text(String),
    Blocks(Vec<JsonlContent>),
}

impl JsonlContentField {
    /// Normalize to blocks so both shapes flow through the same iteration: a bare
    /// string becomes a single `text` block.
    fn into_blocks(self) -> Vec<JsonlContent> {
        match self {
            JsonlContentField::Text(s) => vec![JsonlContent {
                content_type: Some("text".to_string()),
                text: Some(s),
                name: None,
                input: None,
            }],
            JsonlContentField::Blocks(b) => b,
        }
    }
}

#[derive(Deserialize)]
struct JsonlContent {
    #[serde(rename = "type")]
    content_type: Option<String>,
    text: Option<String>,
    // Present on assistant `tool_use` blocks.
    name: Option<String>,
    input: Option<serde_json::Value>,
}

/// Parse NDJSON from a `.jsonl` file and return the first `"user"` message's
/// first text-block content, trimmed and truncated to ~100 chars.
///
/// Returns `None` if no qualifying line is found.
pub fn first_user_prompt(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<JsonlLine>(trimmed) else {
            continue;
        };
        if parsed.line_type.as_deref() != Some("user") {
            continue;
        }
        let Some(message) = parsed.message else {
            continue;
        };
        let Some(contents) = message.content else {
            continue;
        };
        for block in contents.into_blocks() {
            if block.content_type.as_deref() == Some("text") {
                if let Some(text) = block.text {
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    return Some(truncate_snippet(text));
                }
            }
        }
    }
    None
}

/// Truncate `s` to at most 100 Unicode scalar values, appending `"…"` when
/// truncation occurs.
fn truncate_snippet(s: String) -> String {
    let mut chars = s.char_indices();
    // Walk to the 100th character boundary.
    if let Some((byte_idx, _)) = chars.nth(100) {
        // There are more than 100 chars — truncate and append ellipsis.
        let mut result = s[..byte_idx].to_string();
        result.push('…');
        result
    } else {
        // 100 chars or fewer — return as-is.
        s
    }
}

// ---------------------------------------------------------------------------
// transcript parsing
// ---------------------------------------------------------------------------

/// Parse a full session `.jsonl` into ordered transcript entries.
///
/// - `"user"` lines → `TranscriptEntry::User` for each non-empty text block
///   (tool_result and other non-text blocks are machine noise — skipped).
/// - `"assistant"` lines → `TranscriptEntry::Assistant` for each text block and
///   `TranscriptEntry::Tool` for each `tool_use` block.
/// - Every other line type, blank line, or unparseable line is skipped.
/// Parse an ISO-8601 UTC timestamp ("2026-06-01T20:03:03.967Z", millis
/// optional) to epoch milliseconds. `None` on any malformed input. No date
/// crate: the date part uses Howard Hinnant's `days_from_civil`.
fn iso_to_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if s.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Optional ".fff" fractional seconds → milliseconds (pad/truncate to 3).
    let millis: i64 = if s.len() > 20 && b[19] == b'.' {
        let frac: String = s[20..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(3)
            .collect();
        if frac.is_empty() {
            0
        } else {
            let mut m: i64 = frac.parse().ok()?;
            for _ in 0..(3 - frac.len()) {
                m *= 10;
            }
            m
        }
    } else {
        0
    };
    // days_from_civil (Hinnant): days since 1970-01-01.
    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hour * 3600 + min * 60 + sec) * 1000 + millis)
}

pub fn parse_transcript(jsonl: &str) -> Vec<TranscriptEntry> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<JsonlLine>(trimmed) else {
            continue;
        };
        let role = parsed.line_type.as_deref();
        // Per-entry time from the JSONL (used for day separators / per-turn times).
        let at = parsed.timestamp.as_deref().and_then(iso_to_ms);

        // A `system/compact_boundary` line inserts a Compaction entry in sequence
        // so the UI shows it at the chronological point where compaction occurred.
        if role == Some("system")
            && parsed.subtype.as_deref() == Some("compact_boundary")
        {
            if let Some(cm) = parsed.compact_metadata {
                out.push(TranscriptEntry::Compaction {
                    trigger: cm.trigger,
                    dropped_tokens: cm.cumulative_dropped_tokens,
                    at,
                });
            }
            continue;
        }

        let Some(message) = parsed.message else {
            continue;
        };
        let Some(contents) = message.content else {
            continue;
        };
        let contents = contents.into_blocks();
        match role {
            Some("user") => {
                for block in contents {
                    if block.content_type.as_deref() == Some("text") {
                        if let Some(text) = block.text {
                            let text = text.trim();
                            if !text.is_empty() {
                                out.push(TranscriptEntry::User {
                                    text: text.to_string(),
                                    at,
                                });
                            }
                        }
                    }
                }
            }
            Some("assistant") => {
                for block in contents {
                    match block.content_type.as_deref() {
                        Some("text") => {
                            if let Some(text) = block.text {
                                let text = text.trim();
                                if !text.is_empty() {
                                    out.push(TranscriptEntry::Assistant {
                                        text: text.to_string(),
                                        at,
                                    });
                                }
                            }
                        }
                        Some("tool_use") => {
                            out.push(TranscriptEntry::Tool {
                                name: block.name.unwrap_or_default(),
                                input: block.input.map(|v| v.to_string()).unwrap_or_default(),
                                at,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// per-model usage aggregation from JSONL backfill
// ---------------------------------------------------------------------------

/// One row of per-model accumulated usage, computed from a session's JSONL.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

/// Pricing table: (input $/Mtok, output $/Mtok, cache_read $/Mtok, cache_write $/Mtok).
///
/// Tries the LiteLLM embedded pricing DB first (see `litellm_rate`); falls back
/// to the verified hand-written table for models absent from LiteLLM or for
/// correctness overrides. The hand-written rates for Opus 4.8 and Sonnet 4.6 are
/// VERIFIED to the cent against claude's own `result.modelUsage.costUSD`.
fn pricing_for(model: &str) -> (f64, f64, f64, f64) {
    // LiteLLM lookup — returns Some on hit, None on miss.
    if let Some(rates) = litellm_rate(model) {
        return rates;
    }

    // Hand-written fallback: covers unknown/future models not yet in LiteLLM.
    // All known specific versions (opus-4-8, sonnet-4-6, haiku-4-5, …) are already
    // handled by litellm_rate above. This table only fires for:
    //   • brand-new versions whose LiteLLM entry hasn't landed yet, OR
    //   • very old versions (e.g. claude-opus-4-0, claude-haiku-3-5) that LiteLLM
    //     uses a different naming scheme for.
    // Rates are (input, output, cache_read, cache_write) $ per million tokens.
    // cache_read = 0.1×input, cache_write = 1.25×input throughout.
    //
    // NOTE: sub-version branches for known LiteLLM entries (e.g. opus-4-8, sonnet)
    // have been removed here — the LiteLLM lookup handles them. Only coarse family
    // fallbacks remain so that any future model that misses LiteLLM still gets a
    // reasonable approximation rather than $0.
    let m = model.to_lowercase();
    if m.contains("opus") {
        // All known Opus 4.x versions with lower pricing ($5/$25) are in LiteLLM.
        // Old Opus (4.0/4.1/3.x) isn't in LiteLLM under the "claude-opus-4-0"
        // naming, so this branch handles those at $15/$75.
        (15.0, 75.0, 1.50, 18.75)
    } else if m.contains("sonnet") {
        // All known Sonnet versions are in LiteLLM; this is a safety net.
        (3.0, 15.0, 0.30, 3.75)
    } else if m.contains("haiku") {
        // Haiku 3.5 uses a different key in LiteLLM (anthropic.claude-3-5-haiku-*)
        // so "claude-haiku-3-5" lands here. Use $0.80/$4.00 (Haiku 3.5 rate).
        (0.80, 4.0, 0.08, 1.0)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    }
}

/// Compute cost in USD from token counts and a pricing tuple ($/Mtok rates).
///
/// Public so `marsad-core` can call it from `handle_usage_update` without
/// duplicating the pricing table.
///
/// Pricing is per model VERSION (see `pricing_for`): e.g. Opus 4.8 = $5/$25,
/// older Opus = $15/$75, Sonnet = $3/$15, Haiku 4.5 = $1/$5 (cache_read =
/// 0.1×input, cache_write = 1.25×input). Unknown model → $0 (not an error).
pub fn model_cost_usd(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
) -> f64 {
    let (ri, ro, rcr, rcc) = pricing_for(model);
    let c_in = input as f64 * ri;
    let c_out = output as f64 * ro;
    let c_cr = cache_read as f64 * rcr;
    let c_cc = cache_creation as f64 * rcc;
    (c_in + c_out + c_cr + c_cc) / 1_000_000.0
}

/// Minimal serde shape for the assistant entries in a JSONL file.
#[derive(serde::Deserialize)]
struct AssistantJsonlLine {
    #[serde(rename = "type")]
    line_type: Option<String>,
    message: Option<AssistantJsonlMessage>,
}

#[derive(serde::Deserialize)]
struct AssistantJsonlMessage {
    model: Option<String>,
    usage: Option<AssistantJsonlUsage>,
}

#[derive(serde::Deserialize)]
struct AssistantJsonlUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// Parse a full session JSONL and aggregate token usage per model.
///
/// Walks all `"assistant"` lines (which includes subagent turns since they are
/// recorded with `isSidechain: true` but the same `type: "assistant"` discriminant).
/// Returns a map from model name → `ModelUsageSummary`.
///
/// Filters out:
/// - the `"<synthetic>"` pseudo-model (claude's token-counting placeholder),
/// - rows where all token counts are zero (no real usage).
pub fn aggregate_model_usage(jsonl: &str) -> std::collections::HashMap<String, ModelUsageSummary> {
    let mut out: std::collections::HashMap<String, ModelUsageSummary> =
        std::collections::HashMap::new();
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<AssistantJsonlLine>(trimmed) else {
            continue;
        };
        if parsed.line_type.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = parsed.message else {
            continue;
        };
        let Some(model) = message.model else {
            continue;
        };
        // Skip the synthetic pseudo-model (claude's internal placeholder).
        if model == "<synthetic>" {
            continue;
        }
        let Some(usage) = message.usage else {
            continue;
        };
        // Skip zero-usage rows (no real tokens consumed).
        let total_tokens = usage
            .input_tokens
            .saturating_add(usage.output_tokens)
            .saturating_add(usage.cache_read_input_tokens)
            .saturating_add(usage.cache_creation_input_tokens);
        if total_tokens == 0 {
            continue;
        }
        let cost = model_cost_usd(
            &model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        );
        let entry = out.entry(model).or_insert(ModelUsageSummary {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            cost_usd: 0.0,
        });
        entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(usage.output_tokens);
        entry.cache_read_input_tokens = entry
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        entry.cache_creation_input_tokens = entry
            .cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
        entry.cost_usd += cost;
    }
    out
}

/// Read the transcript for one Claude session.
///
/// Reads `<projects_root>/<encode_project_dir(cwd)>/<session_id>.jsonl` and
/// returns its parsed [`TranscriptEntry`] list. Returns an empty `Vec` (not an
/// error) when the file does not exist yet (e.g. claude has not written it).
pub fn session_transcript(
    projects_root: &Path,
    cwd: &str,
    session_id: &str,
) -> std::io::Result<Vec<TranscriptEntry>> {
    let path = projects_root
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(&path)?;
    Ok(parse_transcript(&contents))
}

/// Aggregate per-model token usage from one Claude session's JSONL file.
/// Returns empty map when the file does not exist or cannot be read.
/// Used to seed usage on attach for LOCAL sessions (remote sessions use the
/// agent-side python aggregation script which covers all *.jsonl in the project).
pub fn session_usage(
    projects_root: &Path,
    cwd: &str,
    session_id: &str,
) -> std::collections::HashMap<String, ModelUsageSummary> {
    let path = projects_root
        .join(encode_project_dir(cwd))
        .join(format!("{session_id}.jsonl"));
    match std::fs::read_to_string(&path) {
        Ok(contents) => aggregate_model_usage(&contents),
        Err(_) => std::collections::HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// list_sessions
// ---------------------------------------------------------------------------

/// Enumerate Claude Code sessions for `cwd` under `projects_root`.
///
/// Reads `<projects_root>/<encode_project_dir(cwd)>/*.jsonl`, builds a
/// [`SessionSummary`] for each file, and returns the list sorted **newest
/// first** (by mtime).
///
/// If the encoded directory does not exist, returns an empty `Vec` (not an
/// error).
pub fn list_sessions(projects_root: &Path, cwd: &str) -> std::io::Result<Vec<SessionSummary>> {
    let encoded = encode_project_dir(cwd);
    let dir = projects_root.join(&encoded);

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries: Vec<SessionSummary> = Vec::new();

    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();

        // Skip everything that is not a `.jsonl` file (or has a non-UTF-8 stem).
        // Using filter_map logic in-line keeps the control flow branchless for llvm-cov.
        let id = match path
            .extension()
            .and_then(|e| e.to_str())
            .filter(|&e| e == "jsonl")
            .and_then(|_| path.file_stem())
            .and_then(|s| s.to_str())
        {
            Some(s) => s.to_string(),
            None => continue,
        };

        // Mtime as epoch milliseconds.
        let metadata = std::fs::metadata(&path)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Read the file and extract the first user prompt.
        let contents = std::fs::read_to_string(&path)?;
        let snippet = first_user_prompt(&contents).unwrap_or_default();

        summaries.push(SessionSummary {
            id,
            snippet,
            modified,
        });
    }

    // Sort newest first.
    summaries.sort_by_key(|s| std::cmp::Reverse(s.modified));

    Ok(summaries)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::SystemTime;

    /// Regression: Claude records simple messages with `content` as a bare STRING
    /// (not an array of blocks). Such a user line previously failed to deserialize
    /// and was silently dropped — losing the user's prompt from the transcript
    /// (a real user hit this: the assistant reply showed, but the string-content
    /// prompt above it did not). Both the string form and the array form must
    /// yield entries.
    #[test]
    fn parse_transcript_handles_string_content_messages() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":"hello world"},"timestamp":"2026-01-01T00:00:00.000Z"}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi back"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":"plain string reply"}}"#,
        );
        let out = parse_transcript(jsonl);
        assert_eq!(
            out.len(),
            3,
            "string-content user + array-content assistant + string-content assistant all parse"
        );
        assert!(
            matches!(&out[0], TranscriptEntry::User { text, .. } if text == "hello world"),
            "string-content user message must yield a User entry, got {:?}",
            out[0]
        );
        assert!(matches!(&out[1], TranscriptEntry::Assistant { text, .. } if text == "hi back"));
        assert!(
            matches!(&out[2], TranscriptEntry::Assistant { text, .. } if text == "plain string reply"),
            "string-content assistant message must yield an Assistant entry, got {:?}",
            out[2]
        );
    }

    // -------------------------------------------------------------------------
    // encode_project_dir
    // -------------------------------------------------------------------------

    #[test]
    fn encode_plain_alphanumeric_unchanged() {
        assert_eq!(encode_project_dir("abc123"), "abc123");
    }

    #[test]
    fn encode_leading_slash() {
        assert_eq!(
            encode_project_dir("/work/mewais/Marsad"),
            "-work-mewais-Marsad"
        );
    }

    #[test]
    fn encode_underscores_become_dashes() {
        assert_eq!(
            encode_project_dir("/tmp/marsad-cont-3lx1an_6/project"),
            "-tmp-marsad-cont-3lx1an-6-project"
        );
    }

    #[test]
    fn encode_dots_become_dashes() {
        assert_eq!(
            encode_project_dir("/home/user/my.project"),
            "-home-user-my-project"
        );
    }

    #[test]
    fn encode_spaces_become_dashes() {
        assert_eq!(
            encode_project_dir("/home/user/my project"),
            "-home-user-my-project"
        );
    }

    #[test]
    fn encode_mixed_case_preserved() {
        assert_eq!(encode_project_dir("/Work/MyProject"), "-Work-MyProject");
    }

    #[test]
    fn encode_empty_string() {
        assert_eq!(encode_project_dir(""), "");
    }

    // -------------------------------------------------------------------------
    // first_user_prompt
    // -------------------------------------------------------------------------

    #[test]
    fn first_user_prompt_none_on_empty_input() {
        assert_eq!(first_user_prompt(""), None);
    }

    #[test]
    fn first_user_prompt_none_when_no_user_line() {
        let jsonl = r#"{"type":"queue-operation","data":{}}"#;
        assert_eq!(first_user_prompt(jsonl), None);
    }

    #[test]
    fn first_user_prompt_returns_text_from_user_line() {
        let jsonl = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":[{"text":"Remember this exact token: MARSAD-CONT-77. Reply only: OK","type":"text"}]}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(
            result,
            Some("Remember this exact token: MARSAD-CONT-77. Reply only: OK".to_string())
        );
    }

    #[test]
    fn first_user_prompt_skips_non_user_lines_first() {
        let jsonl = concat!(
            r#"{"type":"queue-operation","data":{}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"text":"hi","type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"text":"first prompt","type":"text"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("first prompt".to_string()));
    }

    #[test]
    fn first_user_prompt_returns_first_of_multiple_user_lines() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"text":"first","type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"text":"second","type":"text"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("first".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_content_has_no_text_block() {
        // content present but no "type":"text" block
        let jsonl =
            r#"{"type":"user","message":{"role":"user","content":[{"type":"image","url":"x"}]}}"#;
        assert_eq!(first_user_prompt(jsonl), None);
    }

    #[test]
    fn first_user_prompt_skips_non_text_content_then_finds_text() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"image","url":"x"},{"type":"text","text":"hello"}]}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn first_user_prompt_truncates_long_text() {
        // 101 'a' characters → truncated to 100 + '…'
        let long_text = "a".repeat(101);
        let jsonl = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{long_text}"}}]}}}}"#
        );
        let result = first_user_prompt(&jsonl).unwrap();
        assert!(result.ends_with('…'), "should end with ellipsis: {result}");
        // The snippet should be 100 'a's + '…'
        assert_eq!(result, format!("{}…", "a".repeat(100)));
    }

    #[test]
    fn first_user_prompt_exactly_100_chars_no_truncation() {
        let text = "b".repeat(100);
        let jsonl = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        );
        let result = first_user_prompt(&jsonl).unwrap();
        assert_eq!(result, text);
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn first_user_prompt_skips_empty_text_block() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"   "}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"real prompt"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("real prompt".to_string()));
    }

    #[test]
    fn first_user_prompt_skips_invalid_json_lines() {
        let jsonl = concat!(
            "not json\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"valid"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("valid".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_message_is_null() {
        let jsonl = r#"{"type":"user","message":null}"#;
        // The serde type uses Option<JsonlMessage>, so null deserializes to None.
        // The function should gracefully return None and fall through.
        let result = first_user_prompt(jsonl);
        assert_eq!(result, None);
    }

    #[test]
    fn first_user_prompt_skips_blank_lines() {
        // A JSONL file may contain blank lines between records.
        let jsonl = concat!(
            "   \n",
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"after blanks"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("after blanks".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_message_content_is_null() {
        // message present but content field is null
        let jsonl = r#"{"type":"user","message":{"role":"user","content":null}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(result, None);
    }

    #[test]
    fn first_user_prompt_text_block_with_no_text_field() {
        // content block has type "text" but no text field, followed by a valid line
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"second"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("second".to_string()));
    }

    // -------------------------------------------------------------------------
    // list_sessions
    // -------------------------------------------------------------------------

    fn write_jsonl(dir: &std::path::Path, filename: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn set_mtime(path: &std::path::Path, offset_secs: i64) {
        // Set mtime relative to now using filetime crate — but we don't have that dep.
        // Instead we use std: touch the file with a fake timestamp via std::fs::File
        // metadata manipulation — not available in std. We'll just write and immediately
        // sleep is not an option. Use std::fs::set_modified... which is nightly only.
        //
        // Work-around: write the file at slightly different real times is unreliable.
        // Instead we manipulate the mtime via the `utime` syscall through a raw libc call.
        // Since we're in test code only, we use std::process::Command touch -t.
        use std::process::Command;
        // Compute a timestamp offset from now.
        let base = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let ts = base + offset_secs;
        // Format as [[CC]YY]MMDDhhmm[.SS]
        let dt = ts;
        // Use `touch -d @<epoch>` (GNU coreutils) for simplicity.
        Command::new("touch")
            .arg("-d")
            .arg(format!("@{dt}"))
            .arg(path)
            .status()
            .expect("touch failed");
    }

    #[test]
    fn list_sessions_empty_when_dir_missing() {
        let tmp = tempdir();
        // Use a projects root that has no matching subdir.
        let result = list_sessions(tmp.path(), "/nonexistent/project").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_sessions_empty_when_encoded_dir_missing() {
        let tmp = tempdir();
        // Create a different subdir to make sure the root exists.
        std::fs::create_dir_all(tmp.path().join("some-other-dir")).unwrap();
        let result = list_sessions(tmp.path(), "/my/project").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_sessions_single_file_with_prompt() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/my/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello world"}]}}"#;
        write_jsonl(&proj_dir, "abc-uuid-123.jsonl", jsonl);

        let result = list_sessions(tmp.path(), "/my/project").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "abc-uuid-123");
        assert_eq!(result[0].snippet, "hello world");
        assert!(result[0].modified > 0);
    }

    #[test]
    fn list_sessions_file_with_no_user_prompt_has_empty_snippet() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/proj");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl = r#"{"type":"queue-operation","data":{}}"#;
        write_jsonl(&proj_dir, "no-prompt.jsonl", jsonl);

        let result = list_sessions(tmp.path(), "/proj").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].snippet, "");
    }

    #[test]
    fn list_sessions_skips_non_jsonl_files() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/proj2");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        write_jsonl(&proj_dir, "session.jsonl", r#"{"type":"queue-operation"}"#);
        // Write a non-.jsonl file.
        let txt_path = proj_dir.join("readme.txt");
        std::fs::write(&txt_path, "not a session").unwrap();

        let result = list_sessions(tmp.path(), "/proj2").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "session");
    }

    #[test]
    fn list_sessions_sorted_newest_first() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/myproject");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl_older = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"older prompt"}]}}"#;
        let jsonl_newer = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"newer prompt"}]}}"#;

        let older_path = write_jsonl(&proj_dir, "older-session.jsonl", jsonl_older);
        let newer_path = write_jsonl(&proj_dir, "newer-session.jsonl", jsonl_newer);

        // Set older file mtime to 60 seconds in the past.
        set_mtime(&older_path, -60);
        // Set newer file mtime to now (default after write is fine, but make explicit).
        set_mtime(&newer_path, 0);

        let result = list_sessions(tmp.path(), "/myproject").unwrap();
        assert_eq!(result.len(), 2);
        // Newest first.
        assert_eq!(result[0].snippet, "newer prompt", "newest must be first");
        assert_eq!(result[1].snippet, "older prompt");
        // Newest-first ordering: [0].modified must be >= [1].modified
        let newer_mod = result[0].modified;
        let older_mod = result[1].modified;
        assert!(newer_mod >= older_mod);
    }

    #[test]
    fn list_sessions_multiple_files_with_prompts() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/multi");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let sessions = [
            ("session-a.jsonl", "task alpha", -120i64),
            ("session-b.jsonl", "task beta", -60),
            ("session-c.jsonl", "task gamma", 0),
        ];

        for (filename, prompt, offset) in &sessions {
            let jsonl = format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{prompt}"}}]}}}}"#
            );
            let path = write_jsonl(&proj_dir, filename, &jsonl);
            set_mtime(&path, *offset);
        }

        let result = list_sessions(tmp.path(), "/multi").unwrap();
        assert_eq!(result.len(), 3);
        // Newest first: gamma (0), beta (-60), alpha (-120)
        assert_eq!(result[0].snippet, "task gamma");
        assert_eq!(result[1].snippet, "task beta");
        assert_eq!(result[2].snippet, "task alpha");
    }

    #[test]
    fn list_sessions_io_error_on_file_instead_of_dir() {
        // If the encoded path exists as a FILE (not a dir), read_dir returns an error.
        let tmp = tempdir();
        let encoded = encode_project_dir("/io/error/project");
        // Write a regular file at the encoded path.
        let fake_dir_path = tmp.path().join(&encoded);
        std::fs::write(&fake_dir_path, "not a directory").unwrap();

        // list_sessions should propagate the I/O error.
        let result = list_sessions(tmp.path(), "/io/error/project");
        assert!(
            result.is_err(),
            "expected I/O error when encoded path is a file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn list_sessions_io_error_on_unreadable_jsonl_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir();
        let encoded = encode_project_dir("/perm/test/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        // Write a valid .jsonl file, then remove read permission.
        let path = write_jsonl(
            &proj_dir,
            "secret.jsonl",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"secret"}]}}"#,
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Privileged users (e.g. root in a CI/dev container) bypass DAC permission
        // bits, so a 0o000 file is still readable and this failure mode does not
        // occur under root. Assert whichever outcome actually matches the current
        // process's privilege instead of early-returning on the privileged branch
        // — an early return is itself a line that only the *other* environment
        // (root) ever executes, which made this test permanently short one line
        // of coverage under the non-root environment the CI gate actually runs
        // in. Folding both outcomes into one always-executed assertion keeps
        // `cargo llvm-cov -p marsad-core --fail-under-lines 100
        // --fail-under-regions 100` genuinely at 100% in both environments,
        // rather than requiring a documented exemption for this file.
        let privileged = std::fs::read(&path).is_ok();

        let result = list_sessions(tmp.path(), "/perm/test/project");
        // Restore permissions so the TempDir cleanup can remove the file.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        // A static (non-formatted) panic message, unlike one with format
        // arguments, does not introduce a lazily-evaluated-on-failure region
        // — keeping every part of this line executed regardless of which
        // branch the equality check takes.
        assert_eq!(
            result.is_err(),
            !privileged,
            "list_sessions error state must match privilege"
        );
    }

    #[test]
    #[cfg(unix)]
    fn list_sessions_io_error_on_dangling_symlink_jsonl() {
        // A dangling symlink ending in .jsonl will appear in the directory listing
        // but `std::fs::metadata()` (which follows symlinks) will fail with ENOENT.
        use std::os::unix::fs::symlink;
        let tmp = tempdir();
        let encoded = encode_project_dir("/symlink/test/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        // Create a dangling symlink: points to a non-existent target.
        let link_path = proj_dir.join("dangling.jsonl");
        symlink("/nonexistent/target/path", &link_path).unwrap();

        let result = list_sessions(tmp.path(), "/symlink/test/project");
        assert!(
            result.is_err(),
            "expected I/O error for dangling symlink .jsonl"
        );
    }

    // -------------------------------------------------------------------------
    // parse_transcript
    // -------------------------------------------------------------------------

    #[test]
    fn transcript_user_text_becomes_user_entry() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi there"}]}}"#;
        assert_eq!(
            parse_transcript(jsonl),
            vec![TranscriptEntry::User {
                text: "hi there".to_string()
            , at: None }]
        );
    }

    #[test]
    fn transcript_assistant_text_and_tool_use() {
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"on it"},{"type":"tool_use","id":"t1","name":"Write","input":{"path":"a.txt"}}]}}"#;
        let t = parse_transcript(jsonl);
        assert_eq!(
            t,
            vec![
                TranscriptEntry::Assistant {
                    text: "on it".to_string()
                , at: None },
                // serde_json renders the single-key object deterministically.
                TranscriptEntry::Tool {
                    name: "Write".to_string(),
                    input: r#"{"path":"a.txt"}"#.to_string(),
                    at: None,
                },
            ]
        );
    }

    #[test]
    fn transcript_tool_use_defaults_when_name_input_absent() {
        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use"}]}}"#;
        assert_eq!(
            parse_transcript(jsonl),
            vec![TranscriptEntry::Tool {
                name: String::new(),
                input: String::new(),
                at: None
            }]
        );
    }

    #[test]
    fn transcript_skips_user_tool_result_and_empty_and_missing_text() {
        let jsonl = concat!(
            // tool_result (non-text user block) → skipped
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}"#,
            "\n",
            // whitespace-only user text → skipped
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"   "}]}}"#,
            "\n",
            // user text block with no text field → skipped
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text"}]}}"#,
        );
        assert_eq!(parse_transcript(jsonl), vec![]);
    }

    #[test]
    fn transcript_skips_assistant_empty_text_missing_text_and_other_blocks() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"  "}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"image","url":"x"}]}}"#,
        );
        assert_eq!(parse_transcript(jsonl), vec![]);
    }

    #[test]
    fn transcript_skips_blank_invalid_other_roles_and_missing_fields() {
        let jsonl = concat!(
            "   \n",                               // blank
            "not json\n",                          // unparseable
            r#"{"type":"summary","summary":"x"}"#, // other role, no message
            "\n",
            r#"{"type":"user"}"#, // no message
            "\n",
            r#"{"type":"user","message":{"role":"user","content":null}}"#, // null content
            "\n",
            r#"{"type":"file-history-snapshot","message":{"content":[{"type":"text","text":"ignored"}]}}"#, // unknown role
        );
        assert_eq!(parse_transcript(jsonl), vec![]);
    }

    #[test]
    fn transcript_full_conversation_in_order() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"q1"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a1"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"q2"}]}}"#,
        );
        assert_eq!(
            parse_transcript(jsonl),
            vec![
                TranscriptEntry::User { text: "q1".into() , at: None },
                TranscriptEntry::Assistant { text: "a1".into() , at: None },
                TranscriptEntry::User { text: "q2".into() , at: None },
            ]
        );
    }

    #[test]
    fn transcript_compact_boundary_inserted_in_sequence() {
        // A compact_boundary line between two turns must produce a Compaction
        // entry at the correct position — not appended at the end.
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"before"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"compact_boundary","session_id":"s1","compact_metadata":{"trigger":"manual","pre_tokens":1000,"post_tokens":200,"cumulative_dropped_tokens":800,"duration_ms":500}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"after"}]}}"#,
        );
        let t = parse_transcript(jsonl);
        assert_eq!(
            t,
            vec![
                TranscriptEntry::User { text: "before".into() , at: None },
                TranscriptEntry::Compaction {
                    trigger: "manual".into(),
                    dropped_tokens: 800,
                    at: None,
                },
                TranscriptEntry::Assistant { text: "after".into() , at: None },
            ],
            "Compaction must appear between user and assistant, not at the end"
        );
    }

    #[test]
    fn transcript_compact_boundary_without_metadata_skipped() {
        // A compact_boundary line with no compact_metadata is skipped gracefully.
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"compact_boundary","session_id":"s1"}"#,
        );
        let t = parse_transcript(jsonl);
        assert_eq!(t, vec![TranscriptEntry::User { text: "hi".into() , at: None }]);
    }

    #[test]
    fn iso_to_ms_parses_utc_timestamps() {
        // Epoch anchors (verifiable): 1970-01-01 = 0; 2000-01-01 = 946684800000ms.
        assert_eq!(iso_to_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_ms("2000-01-01T00:00:00Z"), Some(946_684_800_000));
        // Milliseconds captured; later same-day time is strictly greater.
        let a = iso_to_ms("2026-06-01T20:03:03.967Z").unwrap();
        assert_eq!(a % 1000, 967, "fractional seconds → millis");
        let b = iso_to_ms("2026-06-01T20:03:04Z").unwrap();
        assert!(b > a, "later time must be greater");
        // Space separator + no millis both accepted.
        assert!(iso_to_ms("2026-06-01 20:03:03Z").is_some());
        // Malformed → None (never panics).
        assert_eq!(iso_to_ms("not-a-date"), None);
        assert_eq!(iso_to_ms(""), None);
        assert_eq!(iso_to_ms("2026-13-99T00:00:00Z"), None);
    }

    #[test]
    fn parse_transcript_reads_entry_timestamp() {
        let jsonl = r#"{"type":"user","timestamp":"2026-06-01T20:03:03.967Z","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        let t = parse_transcript(jsonl);
        assert_eq!(t.len(), 1);
        match &t[0] {
            TranscriptEntry::User { text, at } => {
                assert_eq!(text, "hi");
                assert_eq!(*at, iso_to_ms("2026-06-01T20:03:03.967Z"));
                assert!(at.is_some(), "JSONL timestamp must be parsed into `at`");
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // session_transcript
    // -------------------------------------------------------------------------

    #[test]
    fn session_transcript_reads_and_parses() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/my/proj");
        let dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"world"}]}}"#,
        );
        write_jsonl(&dir, "uuid-1.jsonl", jsonl);

        let t = session_transcript(tmp.path(), "/my/proj", "uuid-1").unwrap();
        assert_eq!(
            t,
            vec![
                TranscriptEntry::User {
                    text: "hello".into()
                , at: None },
                TranscriptEntry::Assistant {
                    text: "world".into()
                , at: None },
            ]
        );
    }

    #[test]
    fn session_transcript_empty_when_file_missing() {
        let tmp = tempdir();
        let t = session_transcript(tmp.path(), "/my/proj", "nope").unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn session_transcript_io_error_when_path_is_dir() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/my/proj");
        let dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&dir).unwrap();
        // Create a DIRECTORY where the .jsonl file is expected → read fails.
        std::fs::create_dir_all(dir.join("uuid-2.jsonl")).unwrap();
        let result = session_transcript(tmp.path(), "/my/proj", "uuid-2");
        assert!(result.is_err(), "reading a directory as a file must error");
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("marsad-discovery-test-{n}"));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    // -------------------------------------------------------------------------
    // model_cost_usd — pricing table unit tests
    // -------------------------------------------------------------------------

    #[test]
    fn model_cost_usd_opus_input() {
        // 1_000_000 input tokens on Opus 4.8 = $5.00
        let cost = model_cost_usd("claude-opus-4-8", 1_000_000, 0, 0, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 5_000_000);
    }

    #[test]
    fn model_cost_usd_opus_output() {
        // 1_000_000 output tokens on Opus 4.8 = $25.00
        let cost = model_cost_usd("claude-opus-4-8", 0, 1_000_000, 0, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 25_000_000);
    }

    #[test]
    fn model_cost_usd_opus_old_version_higher_rate() {
        // Older Opus (4.0/4.1) stays at $15/$75.
        let cost = model_cost_usd("claude-opus-4-0", 1_000_000, 0, 0, 0);
        assert_eq!((cost * 1_000_000.0).round() as i64, 15_000_000);
    }

    #[test]
    fn model_cost_usd_haiku_35_lower_rate() {
        // Haiku 3.5 = $0.80 input.
        let cost = model_cost_usd("claude-haiku-3-5", 1_000_000, 0, 0, 0);
        assert_eq!((cost * 1_000_000.0).round() as i64, 800_000);
    }

    #[test]
    fn model_cost_usd_sonnet_input() {
        // 1_000_000 input tokens on sonnet = $3.00
        let cost = model_cost_usd("claude-sonnet-4-6", 1_000_000, 0, 0, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 3_000_000);
    }

    #[test]
    fn model_cost_usd_sonnet_output() {
        // 1_000_000 output on sonnet = $15.00
        let cost = model_cost_usd("claude-sonnet-4-6", 0, 1_000_000, 0, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 15_000_000);
    }

    #[test]
    fn model_cost_usd_haiku() {
        // 1_000_000 input + 1_000_000 output on Haiku 4.5 = $1.00 + $5.00 = $6.00
        let cost = model_cost_usd("claude-haiku-4-5", 1_000_000, 1_000_000, 0, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 6_000_000);
    }

    #[test]
    fn model_cost_usd_opus_cache_read() {
        // 1_000_000 cache_read tokens on Opus 4.8 = $0.50
        let cost = model_cost_usd("claude-opus-4-8", 0, 0, 1_000_000, 0);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 500_000);
    }

    #[test]
    fn model_cost_usd_sonnet_cache_creation() {
        // 1_000_000 cache_creation tokens on sonnet = $3.75
        let cost = model_cost_usd("claude-sonnet-4-6", 0, 0, 0, 1_000_000);
        let micros = (cost * 1_000_000.0).round() as i64;
        assert_eq!(micros, 3_750_000);
    }

    #[test]
    fn model_cost_usd_unknown_model_returns_zero() {
        assert_eq!(
            model_cost_usd(
                "unknown-model-xyz",
                1_000_000,
                1_000_000,
                1_000_000,
                1_000_000
            ),
            0.0
        );
    }

    // -------------------------------------------------------------------------
    // aggregate_model_usage — synthetic + zero filtering
    // -------------------------------------------------------------------------

    #[test]
    fn aggregate_model_usage_skips_synthetic_model() {
        let jsonl = r#"{"type":"assistant","message":{"model":"<synthetic>","usage":{"input_tokens":999,"output_tokens":999,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let result = aggregate_model_usage(jsonl);
        assert!(result.is_empty(), "<synthetic> must be skipped");
    }

    #[test]
    fn aggregate_model_usage_skips_zero_usage_rows() {
        let jsonl = r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let result = aggregate_model_usage(jsonl);
        assert!(result.is_empty(), "zero-usage row must be skipped");
    }

    #[test]
    fn aggregate_model_usage_keeps_real_rows() {
        let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let result = aggregate_model_usage(jsonl);
        assert_eq!(result.len(), 1);
        assert_eq!(result["claude-sonnet-4-6"].input_tokens, 100);
    }

    /// A row with only cache_read tokens (zero input/output) must NOT be skipped.
    #[test]
    fn aggregate_model_usage_keeps_cache_only_rows() {
        let jsonl = r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":500,"cache_creation_input_tokens":0}}}"#;
        let result = aggregate_model_usage(jsonl);
        assert_eq!(result.len(), 1, "cache_read-only row must be kept");
        assert_eq!(result["claude-opus-4-8"].cache_read_input_tokens, 500);
    }

    /// A row with only cache_creation tokens must NOT be skipped.
    #[test]
    fn aggregate_model_usage_keeps_cache_creation_only_rows() {
        let jsonl = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":200}}}"#;
        let result = aggregate_model_usage(jsonl);
        assert_eq!(result.len(), 1, "cache_creation-only row must be kept");
        assert_eq!(result["claude-haiku-4-5"].cache_creation_input_tokens, 200);
    }

    /// Exercise every skip branch (empty line, bad JSON, non-assistant, missing
    /// message/model/usage) AND the same-model accumulation path in one pass.
    #[test]
    fn aggregate_model_usage_skips_all_malformed_and_accumulates() {
        let jsonl = concat!(
            "\n",                // empty line → skip
            "   \n",             // whitespace-only → skip
            "not json at all\n", // parse error → skip
            r#"{"type":"user","message":{"model":"claude-opus-4-8","usage":{"input_tokens":5}}}"#,
            "\n", // non-assistant → skip
            r#"{"type":"assistant"}"#,
            "\n", // no message → skip
            r#"{"type":"assistant","message":{"usage":{"input_tokens":5}}}"#,
            "\n", // no model → skip
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8"}}"#,
            "\n", // no usage → skip
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":20,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            "\n", // same model → accumulate
        );
        let result = aggregate_model_usage(jsonl);
        assert_eq!(
            result.len(),
            1,
            "only the two real opus rows count: {result:?}"
        );
        let opus = &result["claude-opus-4-8"];
        assert_eq!(opus.input_tokens, 30, "accumulated input");
        assert_eq!(opus.output_tokens, 3, "accumulated output");
    }

    // -------------------------------------------------------------------------
    // litellm_rate — branch coverage for every matching step
    // -------------------------------------------------------------------------

    /// (a) Exact key: "claude-opus-4-8" is a top-level key in LiteLLM.
    /// Ground truth: $5/$25/$0.50/$6.25 verified against claude's own costUSD.
    #[test]
    fn litellm_rate_exact_key_opus_4_8() {
        let (inp, out, cr, cc) = litellm_rate("claude-opus-4-8").expect("must hit exact key");
        assert!((inp - 5.0).abs() < 1e-9, "input $/Mtok: {inp}");
        assert!((out - 25.0).abs() < 1e-9, "output $/Mtok: {out}");
        assert!((cr - 0.5).abs() < 1e-9, "cache_read $/Mtok: {cr}");
        assert!((cc - 6.25).abs() < 1e-9, "cache_write $/Mtok: {cc}");
    }

    /// (a) Exact key: "claude-sonnet-4-6".
    /// Ground truth: $3/$15/$0.30/$3.75 verified against claude's own costUSD.
    #[test]
    fn litellm_rate_exact_key_sonnet_4_6() {
        let (inp, out, cr, cc) = litellm_rate("claude-sonnet-4-6").expect("must hit exact key");
        assert!((inp - 3.0).abs() < 1e-9, "input $/Mtok: {inp}");
        assert!((out - 15.0).abs() < 1e-9, "output $/Mtok: {out}");
        assert!((cr - 0.3).abs() < 1e-9, "cache_read $/Mtok: {cr}");
        assert!((cc - 3.75).abs() < 1e-9, "cache_write $/Mtok: {cc}");
    }

    /// (a) Exact key: "claude-haiku-4-5" is present as a top-level key.
    #[test]
    fn litellm_rate_exact_key_haiku_4_5() {
        let (inp, out, cr, cc) = litellm_rate("claude-haiku-4-5").expect("must hit exact key");
        assert!((inp - 1.0).abs() < 1e-9, "input $/Mtok: {inp}");
        assert!((out - 5.0).abs() < 1e-9, "output $/Mtok: {out}");
        assert!((cr - 0.1).abs() < 1e-9, "cache_read $/Mtok: {cr}");
        assert!((cc - 1.25).abs() < 1e-9, "cache_write $/Mtok: {cc}");
    }

    /// (b) Normalized match: "[1m]" suffix stripped → hits exact key.
    #[test]
    fn litellm_rate_normalized_strip_1m_suffix_sonnet() {
        // "claude-sonnet-4.6[1m]" → normalize → "claude-sonnet-4.6" → still not exact
        // → dashed → "claude-sonnet-4-6" → exact match.
        let (inp, out, _cr, _cc) =
            litellm_rate("claude-sonnet-4.6[1m]").expect("must resolve via [1m] strip + dot-dash");
        assert!(
            (inp - 3.0).abs() < 1e-9,
            "input $/Mtok after [1m] strip: {inp}"
        );
        assert!((out - 15.0).abs() < 1e-9, "output $/Mtok: {out}");
    }

    /// (c) Dots → dashes: "claude-sonnet-4.6" → "claude-sonnet-4-6" is in LiteLLM.
    #[test]
    fn litellm_rate_dotted_model_id_resolved() {
        let (inp, out, cr, cc) =
            litellm_rate("claude-sonnet-4.6").expect("must resolve via dot→dash");
        assert!((inp - 3.0).abs() < 1e-9, "input $/Mtok: {inp}");
        assert!((out - 15.0).abs() < 1e-9, "output $/Mtok: {out}");
        assert!((cr - 0.3).abs() < 1e-9, "cache_read $/Mtok: {cr}");
        assert!((cc - 3.75).abs() < 1e-9, "cache_write $/Mtok: {cc}");
    }

    /// (d) Family-major-minor prefix fallback: a date-suffixed key for the same
    /// family+major+minor should be found as the "best" key for that prefix.
    /// "claude-haiku-4-5-20251001" is in LiteLLM, so prefix4 = "claude-haiku-4-5" matches it.
    #[test]
    fn litellm_rate_prefix4_family_fallback_finds_date_suffixed_key() {
        // "claude-haiku-4-5-20251001" is an exact key in LiteLLM.
        // Separately, if we query "claude-haiku-4-5" it matches (a) directly.
        // Query the date-suffixed variant to exercise the prefix4 path for a model
        // that ISN'T an exact key but shares the prefix4 with an existing key.
        // "claude-haiku-4-5-alt" does not exist but its prefix4 = "claude-haiku-4-5"
        // will match "claude-haiku-4-5" and "claude-haiku-4-5-20251001".
        // We use a made-up suffix that forces the prefix path.
        // Actually the prefix4 extraction uses parts [0..3] = "claude-haiku-4-5"
        // for any "claude-haiku-4-5-*" input. Let's pass the date-suffixed key
        // directly — it is an exact match at step (a) first.
        // To truly exercise step (d), use something like "claude-haiku-4-5-rc1" which
        // won't be an exact key but has prefix4 "claude-haiku-4-5" matching the real key.
        let (inp, out, _cr, _cc) = litellm_rate("claude-haiku-4-5-rc1")
            .expect("prefix4 should match claude-haiku-4-5-* keys");
        // The "best" (lex max) key starting with "claude-haiku-4-5" is "claude-haiku-4-5-20251001"
        // or "claude-haiku-4-5" itself — both have $1/$5 for the standard region.
        assert!((inp - 1.0).abs() < 1e-9, "input $/Mtok via prefix4: {inp}");
        assert!((out - 5.0).abs() < 1e-9, "output $/Mtok via prefix4: {out}");
    }

    /// Unknown model → litellm_rate returns None; pricing_for falls back to hand table.
    #[test]
    fn litellm_rate_unknown_model_returns_none() {
        assert!(litellm_rate("claude-future-xyz").is_none());
    }

    /// Unknown model → model_cost_usd falls through to hand table → $0 for fully unknown.
    #[test]
    fn model_cost_usd_fully_unknown_model_falls_back_to_zero() {
        assert_eq!(
            model_cost_usd("totally-unknown-model", 1_000_000, 0, 0, 0),
            0.0
        );
    }

    /// Older Opus (claude-opus-4-0) not in LiteLLM → falls through to hand table → $15/$75.
    /// This validates the hand-table fallback path and ensures old models get correct pricing.
    #[test]
    fn litellm_rate_old_opus_4_0_returns_none_hand_table_used() {
        // claude-opus-4-0 is NOT in LiteLLM; litellm_rate must return None so the
        // hand table kicks in and returns the correct $15/$75 rates.
        assert!(
            litellm_rate("claude-opus-4-0").is_none(),
            "claude-opus-4-0 must NOT be found in LiteLLM (would return wrong $5 rate)"
        );
        let cost = model_cost_usd("claude-opus-4-0", 1_000_000, 0, 0, 0);
        assert!(
            (cost - 15.0).abs() < 1e-9,
            "old opus must use hand table $15: {cost}"
        );
    }

    /// Haiku 3.5 not in LiteLLM (LiteLLM uses a different naming scheme) →
    /// hand table returns $0.80/$4.00.
    #[test]
    fn litellm_rate_haiku_3_5_not_in_litellm_hand_table_used() {
        assert!(
            litellm_rate("claude-haiku-3-5").is_none(),
            "claude-haiku-3-5 must NOT be found in LiteLLM (different naming)"
        );
        let cost = model_cost_usd("claude-haiku-3-5", 1_000_000, 0, 0, 0);
        assert!((cost - 0.80).abs() < 1e-9, "haiku 3.5 hand table: {cost}");
    }

    /// A hypothetical future sonnet not yet in LiteLLM falls through to the hand
    /// table sonnet branch → $3/$15 approximate rate.
    #[test]
    fn hand_table_sonnet_branch_future_model_not_in_litellm() {
        // "claude-sonnet-future-1" is not in LiteLLM; prefix4 = "claude-sonnet-future-1"
        // matches nothing; hand table's sonnet branch fires → $3/$15.
        assert!(litellm_rate("claude-sonnet-future-1").is_none());
        let cost = model_cost_usd("claude-sonnet-future-1", 1_000_000, 0, 0, 0);
        assert!(
            (cost - 3.0).abs() < 1e-9,
            "sonnet hand-table fallback: {cost}"
        );
    }

    /// The embedded JSON parses correctly (OnceLock init path is exercised).
    #[test]
    fn litellm_embedded_json_parses_and_map_is_non_empty() {
        let map = litellm_map();
        assert!(!map.is_empty(), "litellm_map must not be empty after parse");
        // Confirm the map contains expected canonical keys.
        assert!(
            map.contains_key("claude-opus-4-8"),
            "must contain claude-opus-4-8"
        );
        assert!(
            map.contains_key("claude-sonnet-4-6"),
            "must contain claude-sonnet-4-6"
        );
        assert!(
            map.contains_key("claude-haiku-4-5"),
            "must contain claude-haiku-4-5"
        );
    }

    /// Cache fields absent from LiteLLM entry → defaults applied (0.1×input and 1.25×input).
    /// anthropic.claude-instant-v1 has no cache fields in LiteLLM.
    #[test]
    fn litellm_rate_missing_cache_fields_default_to_conventions() {
        // anthropic.claude-instant-v1: input=8e-07, output=2.4e-06, no cache fields.
        let (inp, _out, cr, cc) =
            litellm_rate("anthropic.claude-instant-v1").expect("must find exact key");
        // cache_read default = 0.1 × input = 0.1 × 0.8 = 0.08 $/Mtok
        assert!(
            (cr - inp * 0.1).abs() < 1e-9,
            "cache_read must default to 0.1×input: cr={cr} inp={inp}"
        );
        // cache_write default = 1.25 × input = 1.25 × 0.8 = 1.0 $/Mtok
        assert!(
            (cc - inp * 1.25).abs() < 1e-9,
            "cache_write must default to 1.25×input: cc={cc} inp={inp}"
        );
    }

    /// Provider-prefixed key lookup: "anthropic/claude-sonnet-4-6" is not an exact key
    /// in the map (our subset uses bare "anthropic." prefix keys), but normalization
    /// strips the provider prefix and the bare key is found.
    #[test]
    fn litellm_rate_anthropic_slash_prefix_stripped() {
        // The LiteLLM map uses "anthropic.claude-sonnet-4-6" as a key, not
        // "anthropic/claude-sonnet-4-6". The normalize step strips the "/" prefix
        // to get "claude-sonnet-4-6" which IS in the map at step (b).
        let result = litellm_rate("anthropic/claude-sonnet-4-6");
        // Must resolve via normalize (strip prefix to "claude-sonnet-4-6").
        assert!(
            result.is_some(),
            "anthropic/ prefix must be stripped to find the model"
        );
        let (inp, out, _cr, _cc) = result.unwrap();
        assert!((inp - 3.0).abs() < 1e-9, "input $/Mtok: {inp}");
        assert!((out - 15.0).abs() < 1e-9, "output $/Mtok: {out}");
    }
}
