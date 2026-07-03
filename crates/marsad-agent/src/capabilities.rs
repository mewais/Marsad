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

// ===========================================================================
// Part 3 — the CapabilityProbe seam (§3.1)
// ===========================================================================

/// One driver's discovery logic. Implementations are BLOCKING and are always
/// invoked on a dedicated spawned thread (§4) — never the serve loop, never
/// under any agent mutex.
pub trait CapabilityProbe: Send + Sync + 'static {
    fn driver(&self) -> &'static str;
    /// Full probe for one host: availability + models + sessions for the given
    /// scope. Must respect `ctx.deadline` internally; errors go into
    /// `Capabilities::errors`, never panic/propagate.
    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities;
}

pub struct HostProbeCtx {
    pub host_id: String,
    /// None = local; Some = SSH/WSL target (exec via run_remote_script_bounded).
    pub host_target: Option<String>,
    /// Session scope to scan (claude: accumulated requested projects).
    pub scope: SessionScope,
    /// Hard wall-clock budget (default 6s — matches the existing bounded-probe
    /// convention, agent/src/lib.rs:1440-1490).
    pub deadline: Duration,
}

// ===========================================================================
// Part 4 — CapabilityProvider (§3.2/§3.3/§3.4, §4)
// ===========================================================================

const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_secs(6);

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
            provider.handle_refresh(Some(host), None, None);
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
            self.queue_probe(host.clone(), d, project.clone());
        }

        encode_ipc(&IpcMsg::CapabilitiesRefreshQueued { host_id })
    }

    /// Queue one (driver, host) probe. Accumulates `project` into the host's
    /// `SessionScope::Projects` set (claude) REGARDLESS of in-flight dedup (an
    /// AllKnown driver's probe simply ignores the accumulated scope). A probe
    /// already running for this (driver, host), or a driver with no registered
    /// probe, is skipped — the imminent broadcast (if any) serves the requester.
    fn queue_probe(&self, host_id: String, driver: String, project: Option<String>) {
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
            let ctx = HostProbeCtx {
                host_id: host_id.clone(),
                host_target: Some(host_id.clone()).filter(|h| h != "local"),
                scope,
                deadline: DEFAULT_PROBE_DEADLINE,
            };
            let caps = inner.probes[probe_idx].probe(&ctx);

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
                models: ModelCatalog::default(),
                session_scope: None,
                sessions: vec![],
                fetched_at_ms: 0,
                errors: vec![],
            });
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

#[cfg(not(coverage))]
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(not(coverage))]
fn cli_available(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(coverage))]
fn run_local(bin: &str, args: &[&str], deadline: Duration) -> Result<String, String> {
    // A simple bounded wait: spawn, then poll for exit within the deadline
    // rather than blocking forever (mirrors the `run_remote_script_bounded`
    // convention, agent/src/lib.rs:1440-1490, applied to a LOCAL child).
    let mut child = std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{bin}: spawn failed: {e}"))?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_string(&mut stdout);
                }
                if status.success() {
                    return Ok(stdout);
                }
                return Err(format!("{bin}: exit {status}"));
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{bin}: timed out"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("{bin}: wait failed: {e}")),
        }
    }
}

/// Real `codex debug models` + `~/.codex/sessions` probe.
#[cfg(not(coverage))]
pub struct RealCodexProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealCodexProbe {
    fn driver(&self) -> &'static str {
        "codex"
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let mut errors = Vec::new();
        let available = cli_available("codex");
        let models = if available {
            match run_local("codex", &["debug", "models"], ctx.deadline) {
                Ok(out) => parse_codex_debug_models(&out),
                Err(e) => {
                    errors.push(format!("codex debug models: {e}"));
                    vec![]
                }
            }
        } else {
            vec![]
        };
        let sessions = home_dir()
            .map(|h| scan_codex_sessions(&h.join(".codex").join("sessions"), 200))
            .unwrap_or_default();
        Capabilities {
            driver: "codex".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available: available,
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

/// Real `agy models` + `~/.gemini/antigravity-cli/brain` probe.
#[cfg(not(coverage))]
pub struct RealAgyProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealAgyProbe {
    fn driver(&self) -> &'static str {
        "gemini"
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let mut errors = Vec::new();
        let available = cli_available("agy");
        let models = if available {
            match run_local("agy", &["models"], ctx.deadline) {
                Ok(out) => parse_agy_models(&out),
                Err(e) => {
                    errors.push(format!("agy models: {e}"));
                    vec![]
                }
            }
        } else {
            vec![]
        };
        let root = home_dir().map(|h| h.join(".gemini").join("antigravity-cli"));
        let sessions = root
            .as_ref()
            .map(|root| {
                let last_conversations =
                    std::fs::read_to_string(root.join("cache").join("last_conversations.json"))
                        .unwrap_or_else(|_| "{}".to_string());
                scan_agy_sessions(&root.join("brain"), &last_conversations)
            })
            .unwrap_or_default();
        Capabilities {
            driver: "gemini".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available: available,
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: false,
            },
            session_scope: Some(SessionScope::AllKnown),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// claude has no list command (§1.1) — this probe only establishes
/// `cli_available` + tracks the requested project scope; the model catalog
/// comes entirely from `observe_claude_model` (passive `system:init`
/// observation, §3.2 trigger 4). Local `~/.claude/projects` session
/// enumeration reuses `marsad_core::discovery::{list_sessions,
/// first_user_prompt}` per §1.2 — NOT wired here (would add a new
/// marsad-agent -> marsad-core crate dependency, out of U1's edit scope);
/// an explicit gap is surfaced rather than a fabricated list, per §1.2's
/// disposition for known-but-unimplemented probe legs. Remote (SSH/WSL)
/// hosts are the same explicit-gap disposition (§1.2).
#[cfg(not(coverage))]
pub struct RealClaudeProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealClaudeProbe {
    fn driver(&self) -> &'static str {
        "claude"
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        Capabilities {
            driver: "claude".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available: cli_available("claude"),
            models: ModelCatalog::default(),
            session_scope: Some(ctx.scope.clone()),
            sessions: vec![],
            fetched_at_ms: now_ms(),
            errors: vec!["claude session enumeration not yet wired (M7b follow-up)".to_string()],
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
