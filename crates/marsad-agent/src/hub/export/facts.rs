//! What an export knows about each host before it reads anything (design §6.5 step 4, §6.6): the
//! link as the fleet reports it, and for a connected host its clock against the hub's, its
//! build and platform, its environment, its agents, how many sessions it has.
//!
//! Everything is turned into `marsad_proto::export::HostFacts`, whose every field is a
//! `wasit-diag` value type: ids and words stay words, sizes and times stay numbers, what an agent
//! CLI wrote when it failed shrinks to a length and a fingerprint, and the few texts a host's own
//! shell or transport wrote (environment notes, `PATH`, the link's last error) are scrubbed and
//! bounded by `diag::{host_note, host_path, transport}`. The session list is reduced to counts and
//! the ids of the sessions to read.

use std::sync::Arc;
use std::time::Duration;

use marsad_proto::export::{DriverFacts, EnvFacts, FactFailure, HostFacts, ProbeFacts, SetupFacts};
use marsad_proto::fleet::{FleetHost, FleetLink};
use marsad_proto::hosts::HostKind;
use marsad_proto::HostId;
use wasit_diag::{Code, ErrorInfo, Ident, Ms, TimeMs};
use wasit_runtime::wire::{
    DiagSource, DriverInventory, EnvSource, HostEnvironment, HostInfo, InventoryReply, SessionId,
    SessionSnapshot,
};
use wasit_runtime::RuntimeClient;

use super::source::{client_code, Fail, HostClock, HostLogs, Logs};
use crate::diag;
use crate::util::now_ms;

/// The most `PATH` entries and environment notes the manifest shows.
const PATH_MAX: usize = 64;
const NOTES_MAX: usize = 8;

/// How the hub's link to a host stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkInfo {
    /// The hub's own computer, or a host whose link is up.
    Connected { word: String },
    /// Not connected: the link's state and its last error, in the transport's words.
    Down {
        word: String,
        last_error: Option<String>,
    },
}

impl LinkInfo {
    pub fn word(&self) -> &str {
        match self {
            LinkInfo::Connected { word } | LinkInfo::Down { word, .. } => word,
        }
    }

    pub fn connected(&self) -> bool {
        matches!(self, LinkInfo::Connected { .. })
    }
}

/// One host of the fleet, as the exporter takes it: what the fleet says, and (when it is
/// connected) the client that asks it.
#[derive(Clone)]
pub struct HostSpec {
    pub id: HostId,
    /// `local`, `ssh` or `wsl`.
    pub kind: &'static str,
    /// `hub` for the hub's own computer, else `host`.
    pub role: &'static str,
    pub machine: Option<String>,
    pub platform: Option<(String, String, String)>,
    pub link: LinkInfo,
    pub client: Option<RuntimeClient>,
}

impl HostSpec {
    /// A fleet host; `client` is made for it by the caller (a client addressed to the host).
    pub fn from_fleet(h: &FleetHost, client: &dyn Fn(&HostId) -> RuntimeClient) -> HostSpec {
        let (link, local) = match &h.link {
            FleetLink::Local => (
                LinkInfo::Connected {
                    word: "local".into(),
                },
                true,
            ),
            FleetLink::Remote { state } => {
                use wasit_control::LinkState;
                let word = state.code().to_owned();
                let info = if state.is_connected() {
                    LinkInfo::Connected { word }
                } else {
                    let last_error = match state {
                        LinkState::Reconnecting { last_error, .. } => Some(last_error.clone()),
                        // Why the link is down: how it dropped when it did, else what the
                        // attempt that needed a person said.
                        LinkState::AuthNeeded { detail, cause, .. } => {
                            Some(cause.clone().unwrap_or_else(|| detail.clone()))
                        }
                        LinkState::Failed { reason, .. } => Some(reason.clone()),
                        _ => None,
                    };
                    LinkInfo::Down { word, last_error }
                };
                (info, false)
            }
        };
        let connected = link.connected();
        HostSpec {
            id: h.host.id.clone(),
            kind: match h.host.kind {
                HostKind::Local => "local",
                HostKind::Ssh { .. } => "ssh",
                HostKind::Wsl { .. } => "wsl",
            },
            role: if local { "hub" } else { "host" },
            machine: h.host.machine.clone(),
            platform: h
                .host
                .platform
                .as_ref()
                .map(|p| (p.os.clone(), p.arch.clone(), p.target.clone())),
            link,
            client: connected.then(|| client(&h.host.id)),
        }
    }
}

/// A host after its facts were gathered: the manifest entry, what it offers to read, and the
/// sessions active in the window (newest first).
pub struct HostRun {
    pub spec: HostSpec,
    pub facts: HostFacts,
    pub clock: Arc<HostClock>,
    pub logs: Option<Arc<dyn Logs>>,
    pub sources: Vec<DiagSource>,
    pub sessions: Vec<SessionId>,
    /// Set when the host is connected but its sources could not be listed: nothing of it can be
    /// read.
    pub unreadable: Option<Fail>,
}

/// Asks a host its facts. Every call has the deadline of `clock`; a question that fails is a
/// `failures` entry of the facts, not the end of the export.
pub async fn gather(
    spec: HostSpec,
    since: u64,
    sessions_max: usize,
    call: Duration,
    host: Duration,
) -> HostRun {
    let clock = Arc::new(HostClock::new(call, host));
    let Some(client) = spec.client.clone().filter(|_| spec.link.connected()) else {
        let facts = host_facts(&spec, Gathered::default());
        return HostRun {
            spec,
            facts,
            clock,
            logs: None,
            sources: Vec::new(),
            sessions: Vec::new(),
            unreadable: None,
        };
    };
    let logs = Arc::new(HostLogs {
        client: client.clone(),
        clock: clock.clone(),
    });
    let mut g = Gathered::default();
    let mut unreadable = None;
    let mut sources = Vec::new();

    // The clock: one round trip of `diag.sources`, bracketed by this machine's own clock.
    let sent = now_ms();
    let listed = logs.sources().await;
    let received = now_ms();
    match listed {
        Ok(s) => {
            if let Some(remote) = s.now_ms {
                let mid = sent.saturating_add(received.saturating_sub(sent) / 2);
                g.clock_offset_ms = Some(remote as i64 - mid as i64);
                g.rtt_ms = Some(received.saturating_sub(sent));
            }
            sources = s.sources;
        }
        Err(e) => {
            g.failures
                .push(("diag.sources", failure_info("diag.sources", &e)));
            unreadable = Some(e);
        }
    }
    g.info = ask(
        &mut g.failures,
        "host.info",
        clock.within(client.host_info()),
    )
    .await;
    g.drivers = settled_inventory(&client, &clock, &mut g.failures).await;
    g.env = ask(
        &mut g.failures,
        "host.environment",
        clock.within(client.host_environment()),
    )
    .await
    .flatten();
    g.sessions = ask(
        &mut g.failures,
        "sessions.list",
        clock.within(client.list()),
    )
    .await;

    let mut active: Vec<(u64, SessionId)> = g
        .sessions
        .iter()
        .flatten()
        .filter(|s| s.last_activity_ms >= since)
        .filter_map(|s| SessionId::parse(s.id.as_str()).map(|id| (s.last_activity_ms, id)))
        .collect();
    active.sort_by_key(|a| std::cmp::Reverse(a.0));
    active.truncate(sessions_max);
    let in_window = active.len();
    g.sessions_in_window = Some(in_window as u32);
    let facts = host_facts(&spec, g);
    HostRun {
        spec,
        facts,
        clock,
        logs: Some(logs),
        sources,
        sessions: active.into_iter().map(|(_, id)| id).collect(),
        unreadable,
    }
}

/// The host's agents once discovery has finished probing them (a host that has just connected is
/// still probing: its first inventory says `refreshing`). Waits for the topic's next inventory
/// within the call deadline; what discovery had found when the time ran out is returned too,
/// with the failure recorded, so the manifest says that it did not settle.
async fn settled_inventory(
    client: &RuntimeClient,
    clock: &HostClock,
    failures: &mut Vec<(&'static str, ErrorInfo)>,
) -> Option<Vec<DriverInventory>> {
    use wasit_runtime::ClientError;
    let mut last: Option<InventoryReply> = None;
    let settled = clock
        .within(async {
            let mut sub = client.subscribe_inventory(None);
            loop {
                match sub.next().await {
                    Some(Ok((_, inv))) => {
                        let done = !inv.drivers.iter().any(|d| d.refreshing);
                        last = Some(inv);
                        if done {
                            return Ok(());
                        }
                    }
                    Some(Err(e)) => return Err(e),
                    None => {
                        return Err(ClientError::Call(wasit_control::CallError::NotConnected));
                    }
                }
            }
        })
        .await;
    if let Err(e) = settled {
        failures.push(("drivers.inventory", failure_info("drivers.inventory", &e)));
    }
    last.map(|i| i.drivers)
}

/// One question: its answer, or `None` and a failure for the manifest.
async fn ask<T>(
    failures: &mut Vec<(&'static str, ErrorInfo)>,
    call: &'static str,
    fut: impl std::future::Future<Output = Result<T, Fail>>,
) -> Option<T> {
    match fut.await {
        Ok(v) => Some(v),
        Err(e) => {
            failures.push((call, failure_info(call, &e)));
            None
        }
    }
}

/// A failed question as the manifest shows it: the host's answer by its code, length and
/// fingerprint; a deadline by its word.
fn failure_info(call: &str, e: &Fail) -> ErrorInfo {
    e.info()
        .unwrap_or_else(|| ErrorInfo::foreign(Code::new(&fail_word(e)), call))
}

fn fail_word(e: &Fail) -> String {
    match e {
        Fail::CallDeadline => "call_deadline".into(),
        Fail::HostDeadline => "host_deadline".into(),
        Fail::Client(c) => client_code(c),
        Fail::Io(_) => "io".into(),
    }
}

#[derive(Default)]
struct Gathered {
    info: Option<HostInfo>,
    drivers: Option<Vec<DriverInventory>>,
    env: Option<HostEnvironment>,
    sessions: Option<Vec<SessionSnapshot>>,
    sessions_in_window: Option<u32>,
    clock_offset_ms: Option<i64>,
    rtt_ms: Option<u64>,
    failures: Vec<(&'static str, ErrorInfo)>,
}

fn host_facts(spec: &HostSpec, g: Gathered) -> HostFacts {
    let ident = |s: &str| Ident::new(s);
    let last_error = match &spec.link {
        LinkInfo::Down { last_error, .. } => last_error.as_deref().map(diag::transport),
        LinkInfo::Connected { .. } => None,
    };
    let (os, arch) = match (&g.info, &spec.platform) {
        (Some(i), _) => (Some(i.os.clone()), Some(i.arch.clone())),
        (None, Some((os, arch, _))) => (Some(os.clone()), Some(arch.clone())),
        (None, None) => (None, None),
    };
    HostFacts::Host {
        id: ident(spec.id.as_str()),
        kind: Code::st(spec.kind),
        role: Code::st(spec.role),
        link: Code::new(spec.link.word()),
        drivers: g.drivers.iter().flatten().map(driver_facts).collect(),
        failures: g
            .failures
            .into_iter()
            .map(|(call, error)| FactFailure::Failure {
                call: Code::new(call),
                error,
            })
            .collect(),
        machine: spec.machine.as_deref().map(ident),
        os: os.as_deref().map(ident),
        arch: arch.as_deref().map(ident),
        target: spec.platform.as_ref().map(|(_, _, t)| ident(t)),
        build: g.info.as_ref().map(|i| ident(&i.build)),
        started_at: g.info.as_ref().map(|i| TimeMs(i.started_at_ms)),
        pid: g.info.as_ref().map(|i| i.pid),
        last_error,
        clock_offset_ms: g.clock_offset_ms,
        rtt_ms: g.rtt_ms.map(Ms),
        sessions: g.sessions.as_ref().map(|s| s.len() as u32),
        sessions_in_window: g.sessions_in_window,
        env: g.env.as_ref().map(env_facts),
    }
}

fn driver_facts(d: &DriverInventory) -> DriverFacts {
    DriverFacts::Driver {
        driver: Ident::new(d.driver.as_str()),
        installed: d.installed,
        command_set: d.command.is_some(),
        refreshing: d.refreshing,
        probes: d
            .probes
            .iter()
            .map(|p| ProbeFacts::Probe {
                kind: Code::new(
                    serde_json::to_value(p.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .as_deref()
                        .unwrap_or("unknown"),
                ),
                ok: p.ok,
                ms: Ms(p.duration_ms),
                error: p.error.as_deref().map(|t| diag::foreign("probe", t)),
            })
            .collect(),
        // The first word of what the CLI printed for `--version` ("2.1.293 (Claude Code)").
        version: d
            .version
            .as_deref()
            .and_then(|v| v.split_whitespace().next())
            .map(Ident::new),
        warning: d.warning.as_deref().map(|t| diag::foreign("warning", t)),
        error: d.error.as_deref().map(|t| diag::foreign("error", t)),
        refreshed_at: (d.refreshed_at_ms > 0).then_some(TimeMs(d.refreshed_at_ms)),
    }
}

fn env_facts(e: &HostEnvironment) -> EnvFacts {
    EnvFacts::Env {
        source: Code::st(match e.source {
            EnvSource::LoginInteractiveShell => "login_interactive_shell",
            EnvSource::LoginShell => "login_shell",
            EnvSource::Registry => "registry",
            EnvSource::Process => "process",
        }),
        ms: Ms(e.duration_ms),
        captured_at: TimeMs(e.captured_at_ms),
        notes: e
            .notes
            .iter()
            .take(NOTES_MAX)
            .map(|n| diag::host_note(n))
            .collect(),
        path: e
            .path
            .iter()
            .take(PATH_MAX)
            .map(|p| diag::host_path(p))
            .collect(),
        variables: u32::try_from(e.variables.len()).unwrap_or(u32::MAX),
        // The shell's name, not its path.
        shell: e
            .shell
            .as_deref()
            .and_then(|s| s.rsplit(['/', '\\']).next())
            .map(Ident::new),
        revision: e.revision.as_deref().map(Ident::new),
        setup: e.setup.as_ref().map(|s| SetupFacts::Setup {
            ok: s.ok,
            applied: s.applied,
            ms: Ms(s.duration_ms),
            exit_code: s.exit_code,
            error: s.error.as_deref().map(diag::host_note),
        }),
    }
}
