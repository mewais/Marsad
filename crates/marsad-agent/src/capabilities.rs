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


mod parse;
pub use parse::*;
mod scripts;
pub use scripts::*;
mod scan;
pub use scan::*;
mod real_probes;
pub use real_probes::*;

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
    // Dedup key is (driver, host_id, project): a project-scoped session probe
    // must NOT collide with the connect-time (project=None) capability probe —
    // otherwise picking a path while the ~slow capability probe is still running
    // gets silently dropped and the requester never receives its sessions.
    in_flight: Mutex<HashSet<(String, String, Option<String>)>>,
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
        // Dedup token includes the project so a path-scoped session probe runs
        // independently of the connect-time capability probe (project=None).
        let flight_key = (driver.clone(), host_id.clone(), project.clone());

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
                in_flight.insert(flight_key.clone()).then_some(idx)
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

            inner.in_flight.lock().unwrap().remove(&flight_key);
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
                available_modes: vec![],
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

