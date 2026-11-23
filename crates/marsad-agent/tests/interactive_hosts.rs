//! A host that needs the user at login (a key with a passphrase, asked for through the real
//! askpass helper by a real ssh to a real sshd on localhost, as a password or a one-time code
//! would be): it is never reconnected on its own, its link says so and why it dropped, one click
//! asks once, sessions on it keep running and come back; and the facts that decide it are
//! learned and un-learned from what a sign-in actually asked. No root: a passphrase is asked
//! the same way a password is (an entry through askpass), and what the hub does with it does
//! not depend on which. The password and second-factor tests that need root stay in
//! `remote.rs`.
//!
//! Every ssh host has ONE shared connection (a "master", `hub/master.rs`) that every command of
//! the host rides, so what ends a host's wait for the user is the connection, not the link:
//! "the network is lost" in these tests means the connection dies ([`Rig::drop_network`]); a link
//! that drops while its connection lives reconnects by itself (the last tests of this file). The
//! shared connection's own behaviour (restarts, sign-out, removal, the reason it ended) is
//! tested at the end of the file.
#![cfg(target_os = "linux")]

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::{FleetEvent, FleetLink};
use marsad_proto::hosts::{HostDraft, HostKind, PromptKind};
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, Command, CommandFailureKind, GateDecision, HostId, LinkState};
use serde_json::{json, Value};
use wasit_fake_cli::scenario::{Scenario, Turn};
use wasit_runtime::wire::{DiagTailEvent, DiagTailParams};
use wasit_runtime::TypedSubscription;

const PASSPHRASE: &str = "pp-secret-7f3a";

// ── the hub's own records, live and in order ────────────────────────────────

/// The hub's diagnostics records of a run, read from its `diag.tail` topic as they are written
/// (the ring first, then every later record): in the order the hub emitted them, so that what
/// happened between two records can be asserted without waiting for anything.
struct Tail {
    sub: TypedSubscription<DiagTailEvent>,
    unread: VecDeque<Value>,
    /// Every record read so far.
    seen: Vec<Value>,
    last_seq: Option<u64>,
}

impl Tail {
    fn open(c: &MarsadClient) -> Tail {
        Tail {
            sub: c
                .runtime(None)
                .subscribe_diag_tail(&DiagTailParams::default(), None),
            unread: VecDeque::new(),
            seen: Vec::new(),
            last_seq: None,
        }
    }

    fn take(&mut self, line: &str) {
        let v: Value = serde_json::from_str(line).expect("a record is JSON");
        let seq = v["seq"].as_u64().expect("a record has a seq");
        // A resubscribe (after a lag) repeats nothing the reader already has.
        if self.last_seq.is_none_or(|l| seq > l) {
            self.last_seq = Some(seq);
            self.unread.push_back(v);
        }
    }

    /// The next record not read yet; its index in `seen`.
    async fn pull(&mut self) -> usize {
        loop {
            if let Some(r) = self.unread.pop_front() {
                self.seen.push(r);
                return self.seen.len() - 1;
            }
            match tokio::time::timeout(T, self.sub.next()).await {
                Ok(Some(Ok((_, DiagTailEvent::Reset { records })))) => {
                    for l in records {
                        self.take(&l);
                    }
                }
                Ok(Some(Ok((_, DiagTailEvent::Record { line })))) => self.take(&line),
                other => panic!(
                    "the hub's record stream gave {:?}",
                    other.map(|o| o.is_some())
                ),
            }
        }
    }

    /// Reads records until one satisfies `pred`; its index in `seen`.
    async fn until(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> usize {
        loop {
            let i = self.pull().await;
            if pred(&self.seen[i]) {
                return i;
            }
            let _ = what;
        }
    }
}

/// Whether `r` is about `host` (a record of its link, its connector or the registry).
fn about(r: &Value, host: &str) -> bool {
    r["host"] == host || r["link"] == format!("hosts:{host}").as_str()
}

/// The records of `recs` about `host` named `ev`.
fn evs<'a>(recs: &'a [Value], host: &str, ev: &str) -> Vec<&'a Value> {
    recs.iter()
        .filter(|r| r["ev"] == ev && about(r, host))
        .collect()
}

/// Fails, saying which, when a record about `host` is one of `evs`.
fn none_of(recs: &[Value], host: &str, which: &[&str]) {
    let found: Vec<&Value> = recs
        .iter()
        .filter(|r| about(r, host) && which.contains(&r["ev"].as_str().unwrap_or("")))
        .collect();
    assert!(
        found.is_empty(),
        "records that must not be there: {found:#?}"
    );
}

// ── the rig ──────────────────────────────────────────────────────────────────

fn with_options(kind: HostKind, more: &[&str]) -> HostKind {
    let HostKind::Ssh {
        target,
        port,
        identity_file,
        mut options,
    } = kind
    else {
        unreachable!()
    };
    options.extend(more.iter().map(|o| o.to_string()));
    HostKind::Ssh {
        target,
        port,
        identity_file,
        options,
    }
}

/// A host whose key has a passphrase and that cannot use (or fill) an agent: ssh has to ask.
fn passphrase_kind(sshd: &Sshd, key: &Path) -> HostKind {
    with_options(sshd.kind(key), &["IdentityAgent=none", "AddKeysToAgent=no"])
}

/// Waits until every predicate has matched some event; every event read on the way.
async fn until_all(
    f: &mut Subscription<FleetEvent>,
    what: &str,
    preds: &[&dyn Fn(&FleetEvent) -> bool],
) -> Vec<FleetEvent> {
    let mut done = vec![false; preds.len()];
    let mut all = Vec::new();
    let fut = async {
        while !done.iter().all(|d| *d) {
            let (_, e) = next(f).await;
            for (i, p) in preds.iter().enumerate() {
                done[i] |= p(&e);
            }
            all.push(e);
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    all
}

/// The events up to and including the first that satisfies `pred`.
async fn collect_until(
    f: &mut Subscription<FleetEvent>,
    what: &str,
    pred: impl Fn(&FleetEvent) -> bool,
) -> Vec<FleetEvent> {
    let mut all = Vec::new();
    let fut = async {
        loop {
            let (_, e) = next(f).await;
            let done = pred(&e);
            all.push(e);
            if done {
                return;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    all
}

/// The state of `host`'s link, from the fleet's first snapshot or its later link events, the
/// first time it satisfies `pred`.
async fn until_link(
    f: &mut Subscription<FleetEvent>,
    host: &str,
    pred: impl Fn(&LinkState) -> bool,
) -> LinkState {
    let fut = async {
        loop {
            match next(f).await.1 {
                FleetEvent::Reset { hosts } => {
                    let state = hosts
                        .iter()
                        .find(|h| h.host.id.as_str() == host)
                        .and_then(|h| match &h.link {
                            FleetLink::Remote { state } => Some(state.clone()),
                            FleetLink::Local => None,
                        });
                    if let Some(s) = state.filter(&pred) {
                        return s;
                    }
                }
                FleetEvent::Link { host: h, state } if h.as_str() == host && pred(&state) => {
                    return state
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for the link of {host}"))
}

fn connected(e: &FleetEvent, host: &str) -> bool {
    link_is(e, host, LinkState::is_connected)
}

fn interactive_mark(e: &FleetEvent, host: &str, on: bool) -> bool {
    matches!(e, FleetEvent::HostUpserted { host: h } if h.id.as_str() == host && h.interactive_auth == on)
}

fn parked(e: &FleetEvent, host: &str, held: bool) -> bool {
    link_is(
        e,
        host,
        |s| matches!(s, LinkState::AuthNeeded { held: h, .. } if *h == held),
    )
}

fn reconnecting(e: &FleetEvent, host: &str) -> bool {
    link_is(e, host, |s| matches!(s, LinkState::Reconnecting { .. }))
}

/// Events that say somebody is asked or the host waits for somebody.
fn asks_or_waits(e: &FleetEvent, host: &str) -> bool {
    matches!(e, FleetEvent::AuthPrompt { host: h, prompt: Some(_) } if h.as_str() == host)
        || link_is(e, host, |s| matches!(s, LinkState::AuthNeeded { .. }))
}

/// One hub, one sshd, one passphrase host `pp` (not added yet).
struct Rig {
    sshd: Sshd,
    key: PathBuf,
    data: PathBuf,
    c: MarsadClient,
    fleet: Subscription<FleetEvent>,
    tail: Tail,
    id: HostId,
    markers: u32,
    /// The route of the host once added (its shared connection's socket is named by it).
    kind: Option<HostKind>,
    /// Last: what the rig started goes before the directory it lives in.
    d: TempDir,
}

impl Rig {
    async fn new() -> Rig {
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let key = sshd.passphrase_key(PASSPHRASE);
        let data = d.path().join("l");
        let c = hub_client(&data, &[]);
        let mut fleet = c.fleet(None);
        match next(&mut fleet).await.1 {
            FleetEvent::Reset { hosts } => assert_eq!(hosts[0].host.id, HostId::new("hub")),
            other => panic!("expected reset, got {other:?}"),
        }
        let tail = Tail::open(&c);
        Rig {
            d,
            sshd,
            key,
            data,
            c,
            fleet,
            tail,
            id: HostId::new("pp"),
            markers: 0,
            kind: None,
        }
    }

    async fn add(&mut self, kind: HostKind) {
        self.kind = Some(kind.clone());
        self.c
            .add_host(HostDraft {
                name: "pp".into(),
                kind,
                id: Some(self.id.clone()),
            })
            .await
            .unwrap();
    }

    /// The control socket of the host's shared connection, named as the hub names it.
    fn master(&self) -> PathBuf {
        master_socket(
            &self.data,
            self.id.as_str(),
            self.kind.as_ref().expect("the host was added"),
        )
    }

    /// The network to the host is lost: the shared connection dies without a word (its process
    /// is killed and its socket left behind, as a lost network or a crash leaves it), and with it
    /// the link that rode it. Returns when the connection is gone, so that what the hub does
    /// next can only find it gone.
    async fn drop_network(&mut self) {
        kill_master(&self.master()).await;
    }

    /// How many times ssh asked a helper for an answer so far, over every hub of this data (the
    /// helper records each call in the shared diagnostics file).
    fn askpass_calls(&self) -> usize {
        let path = layout(&self.data).diag().join("aux.ndjson");
        std::fs::read_to_string(path).map_or(0, |t| {
            t.lines()
                .filter(|l| l.contains("\"ev\":\"askpass.end\""))
                .count()
        })
    }

    /// The first connect: ssh asks for the passphrase, the user answers, the host connects, and
    /// the hub has learned that it needs the user at login.
    async fn sign_in(&mut self) {
        self.add(passphrase_kind(&self.sshd, &self.key)).await;
        let p = prompt(&mut self.fleet, "pp").await;
        assert_eq!(p.kind, PromptKind::Secret, "{}", p.prompt);
        assert_eq!((p.attempt, p.repeat), (1, None));
        self.c
            .answer_auth(&self.id, p.id, PASSPHRASE)
            .await
            .unwrap();
        until_all(
            &mut self.fleet,
            "pp connected and marked as needing the user",
            &[&|e| connected(e, "pp"), &|e| {
                interactive_mark(e, "pp", true)
            }],
        )
        .await;
    }

    /// A record of the hub made after everything the test has seen of it: renaming the host
    /// changes nothing about its link. What the hub emitted before this call is in `tail.seen`
    /// up to the returned index.
    async fn marker(&mut self) -> usize {
        self.markers += 1;
        let mut e = self
            .c
            .hosts()
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.id == self.id)
            .unwrap();
        e.name = format!("pp-{}", self.markers);
        self.c.update_host(e).await.unwrap();
        let id = self.id.as_str().to_owned();
        self.tail
            .until("the marker", |r| {
                r["ev"] == "hosts.changed" && r["host"] == id.as_str() && r["op"] == "updated"
            })
            .await
    }

    /// The hub is stopped and started again over the same data (the host's own daemon is not
    /// touched). The new fleet subscription has not read its snapshot.
    async fn restart_hub(self) -> Rig {
        self.restart_hub_with(false).await
    }

    /// As [`Rig::restart_hub`]; `lose_connection`: the shared connection dies while the hub is
    /// down.
    async fn restart_hub_with(self, lose_connection: bool) -> Rig {
        let Rig {
            d,
            sshd,
            key,
            data,
            c,
            fleet,
            tail,
            id,
            markers,
            kind,
        } = self;
        drop(fleet);
        drop(tail);
        drop(c);
        stop(&data).await;
        if lose_connection {
            let sock = master_socket(&data, id.as_str(), kind.as_ref().unwrap());
            kill_master(&sock).await;
        }
        let c = hub_client(&data, &[]);
        let fleet = c.fleet(None);
        let tail = Tail::open(&c);
        Rig {
            d,
            sshd,
            key,
            data,
            c,
            fleet,
            tail,
            id,
            markers,
            kind,
        }
    }

    /// The pid of the ssh the link of `pp` runs now (its own process, started by the hub).
    async fn link_pid(&mut self) -> u32 {
        let m = self.marker().await;
        self.tail.seen[..m]
            .iter()
            .rev()
            .find(|r| r["ev"] == "transport.spawned" && about(r, "pp") && r["program"] == "ssh")
            .and_then(|r| r["pid"].as_u64())
            .expect("a spawned ssh of pp") as u32
    }
}

// ── a session on the host, waiting on a gate ─────────────────────────────────

struct Gated {
    id: wasit_runtime::wire::SessionId,
    rt: wasit_runtime::RuntimeClient,
    sub: TypedSubscription<SessionEvent>,
    seen: Vec<SessionEvent>,
    request_id: String,
}

async fn session_event(sub: &mut TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(T, sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        other => panic!(
            "no session event: {:?}",
            other.map(|o| o.map(|r| r.map(|(c, _)| c)))
        ),
    }
}

/// Launches a session on `pp` whose turn waits for the user to allow a tool.
async fn launch_gated(r: &Rig) -> Gated {
    let work = r.sshd.remote_home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let sc = Scenario::builder()
        .turn(
            Turn::builder()
                .tool("Bash", json!({"command": "ls"}))
                .needs_permission()
                .result_if_allowed("listed")
                .done()
                .say("done remotely"),
        )
        .build();
    let mut req = LaunchRequest::new("claude");
    req.host = Some(r.id.clone());
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(r.d.path(), &sc),
        ..OverrideLayer::default()
    };
    let launched = r.c.launch(req).await.unwrap();
    let id = launched.id;
    let rt = r.c.runtime(Some(r.id.clone()));
    let mut sub = rt.subscribe_session(&id, None);
    let mut seen = Vec::new();
    loop {
        let e = session_event(&mut sub).await;
        let idle =
            matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == "idle");
        seen.push(e);
        if idle {
            break;
        }
    }
    rt.command(&id, Command::prompt("go")).await.unwrap();
    let request_id = loop {
        let e = session_event(&mut sub).await;
        let rid = match &e.kind {
            SessionEventKind::Agent {
                event: AgentEvent::GateRequested { gate },
            } => Some(gate.request_id().to_string()),
            _ => None,
        };
        seen.push(e);
        if let Some(r) = rid {
            break r;
        }
    };
    Gated {
        id,
        rt,
        sub,
        seen,
        request_id,
    }
}

// ── the tests ────────────────────────────────────────────────────────────────

/// The whole story of the owner's change for one host. The passphrase is asked at the first
/// connect and the host is marked. The network is lost (its shared connection dies, and the link
/// that rode it): the link parks as "sign-in needed" at once
/// (never "reconnecting"), keeping the drop's own reason next to the fact that it waits for the
/// user, and no ssh is started (the hub's records, in order, up to a later record of its own
/// show none). A fresh subscriber sees the same state. One click asks exactly once, connects
/// with one attempt, and the session that was waiting for a gate before the drop is live again:
/// its events resume from the cursor with no gap and no duplicate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_passphrase_host_waits_for_the_user_and_one_click_brings_its_session_back() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    let mut g = launch_gated(&r).await;
    until(&mut r.fleet, "the session in the fleet", |e| {
        matches!(e, FleetEvent::SessionUpserted { host, session } if *host == id && session.id == g.id && session.phase.name() == "needs_you")
    })
    .await;

    // The network is lost: the connection the link rides dies, and the link with it.
    r.marker().await;
    let from = r.tail.seen.len();
    r.drop_network().await;
    let ev = until(
        &mut r.fleet,
        "the next link state of pp",
        |e| matches!(e, FleetEvent::Link { host, .. } if *host == id),
    )
    .await;
    let FleetEvent::Link {
        state:
            LinkState::AuthNeeded {
                problem,
                detail,
                cause,
                held,
            },
        ..
    } = ev
    else {
        panic!("the first state after the drop must be the parked one, got {ev:?}");
    };
    assert!(
        held,
        "parked because it waits for the user, not because an attempt failed"
    );
    assert_eq!(problem, CommandFailureKind::AuthNeeded);
    let cause = cause.expect("the drop's own reason is kept");
    assert!(!cause.is_empty());
    assert!(
        detail.contains("shared connection") && detail.contains("ended"),
        "the wait says the shared connection ended: {detail:?}"
    );
    assert!(
        !cause.contains(&detail) && !detail.contains(&cause),
        "the cause is not overwritten by the generic text: {cause:?} / {detail:?}"
    );

    // A subscriber that arrives now is told the same, sessions as last known.
    let mut fresh = r.c.fleet(None);
    match next(&mut fresh).await.1 {
        FleetEvent::Reset { hosts } => {
            let h = hosts.iter().find(|h| h.host.id == id).unwrap();
            assert!(h.host.interactive_auth);
            assert_eq!(
                h.link,
                FleetLink::Remote {
                    state: LinkState::AuthNeeded {
                        problem,
                        detail: detail.clone(),
                        cause: Some(cause.clone()),
                        held: true,
                    }
                }
            );
            assert!(!h.sessions_live);
            assert_eq!(h.sessions.len(), 1, "the session is listed as last known");
            assert!(h.auth_prompt.is_none(), "nobody was asked");
        }
        other => panic!("{other:?}"),
    }
    drop(fresh);

    // No ssh was started: everything the hub emitted between the drop and a record of its own
    // made afterwards says it parked, and nothing says it tried.
    let m = r.marker().await;
    let window = &r.tail.seen[from..m];
    none_of(
        window,
        "pp",
        &["boot.attempt", "transport.spawned", "link.attempt_begin"],
    );
    assert_eq!(evs(window, "pp", "link.closed").len(), 1, "{window:#?}");
    assert_eq!(evs(window, "pp", "hosts.held").len(), 1, "{window:#?}");
    let states: Vec<&str> = evs(window, "pp", "link.state")
        .iter()
        .map(|r| r["to"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["auth_needed"], "never reconnecting or connecting");

    // One click: one attempt, one prompt.
    r.c.reconnect_host(&id).await.unwrap();
    let p = prompt(&mut r.fleet, "pp").await;
    assert_eq!((p.kind, p.attempt, p.repeat), (PromptKind::Secret, 1, None));
    r.c.answer_auth(&id, p.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    let m2 = r.marker().await;
    let click = &r.tail.seen[m + 1..m2];
    let attempts = evs(click, "pp", "boot.attempt");
    assert_eq!(attempts.len(), 1, "{click:#?}");
    assert_eq!(
        (&attempts[0]["armed"], &attempts[0]["interactive"]),
        (&json!(true), &json!(true))
    );
    assert_eq!(evs(click, "pp", "auth.prompt").len(), 1, "one prompt");
    assert_eq!(evs(click, "pp", "hosts.held").len(), 0);

    // The session is live again and nothing was lost.
    g.rt.command(
        &g.id,
        Command::AnswerGate {
            request_id: g.request_id.clone(),
            decision: GateDecision::choose("allow"),
        },
    )
    .await
    .unwrap();
    loop {
        let e = session_event(&mut g.sub).await;
        let done = matches!(
            &e.kind,
            SessionEventKind::Agent {
                event: AgentEvent::TurnCompleted { .. }
            }
        );
        g.seen.push(e);
        if done {
            break;
        }
    }
    let page = g.rt.history(&g.id, None, 2000).await.unwrap();
    assert_eq!(
        &page.events[..g.seen.len()],
        &g.seen[..],
        "no gap, no duplicate across the drop and the click"
    );
    until(&mut r.fleet, "the finished turn in the fleet", |e| {
        matches!(e, FleetEvent::SessionUpserted { host, session } if *host == id && session.id == g.id && session.turns_completed == 1)
    })
    .await;
    g.rt.delete(&g.id).await.unwrap();
}

/// A hub that starts with a stored passphrase host whose shared connection is gone (it died while
/// the hub was down) does not ask for it: no attempt, no ssh, the link is parked at once with the
/// same published state as after a drop (there is no drop to quote), and one click connects it.
/// (A hub that finds the connection open reconnects without asking: see
/// `a_hub_restart_finds_the_open_connection_and_reconnects_without_a_prompt`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hub_that_starts_with_a_passphrase_host_parks_it_without_an_ssh() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let mut r = r.restart_hub_with(true).await;
    let state = until_link(&mut r.fleet, "pp", |s| {
        matches!(s, LinkState::AuthNeeded { .. })
    })
    .await;
    assert!(
        matches!(
            &state,
            LinkState::AuthNeeded {
                held: true,
                cause: None,
                problem: CommandFailureKind::AuthNeeded,
                ..
            }
        ),
        "{state:?}"
    );
    // Nothing was tried: the records of this hub's whole life so far, up to a record made now.
    let m = r.marker().await;
    let all = &r.tail.seen[..m];
    none_of(
        all,
        "pp",
        &["boot.attempt", "transport.spawned", "link.attempt_begin"],
    );
    assert_eq!(evs(all, "pp", "hosts.held").len(), 1, "{all:#?}");
    // Resubscribing shows the same.
    let mut again = r.c.fleet(None);
    assert_eq!(until_link(&mut again, "pp", |_| true).await, state);
    drop(again);

    // One click, one prompt, connected (the host's daemon was never stopped).
    r.c.reconnect_host(&r.id).await.unwrap();
    let p = prompt(&mut r.fleet, "pp").await;
    r.c.answer_auth(&r.id, p.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;
}

/// A host whose only question was ssh's confirmation of its key is not a host that needs the
/// user: it is not marked, and when its link drops it reconnects on its own, with nobody asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_that_only_asked_to_trust_its_key_reconnects_on_its_own() {
    let mut r = Rig::new().await;
    let kh = r.d.path().join("empty_known_hosts");
    std::fs::write(&kh, "").unwrap();
    let HostKind::Ssh {
        target,
        port,
        identity_file,
        ..
    } = r.sshd.kind(&r.sshd.key)
    else {
        unreachable!()
    };
    r.add(HostKind::Ssh {
        target,
        port,
        identity_file,
        options: vec![
            format!("UserKnownHostsFile={}", kh.display()),
            "StrictHostKeyChecking=ask".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "IdentityAgent=none".into(),
            "AddKeysToAgent=no".into(),
        ],
    })
    .await;
    let p = prompt(&mut r.fleet, "pp").await;
    assert_eq!(p.kind, PromptKind::Confirm, "{}", p.prompt);
    r.c.answer_auth(&r.id, p.id, "yes").await.unwrap();
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;

    // The network is lost: it reconnects by itself, through "reconnecting", asking nobody (it
    // opens a new shared connection, with no prompt: its host key is known now).
    r.marker().await;
    let from = r.tail.seen.len();
    r.drop_network().await;
    let events = collect_until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    assert!(events.iter().any(|e| reconnecting(e, "pp")), "{events:?}");
    assert!(
        !events.iter().any(|e| asks_or_waits(e, "pp")),
        "nobody is asked and nothing waits: {events:?}"
    );

    let m = r.marker().await;
    let all = &r.tail.seen[..m];
    assert!(
        evs(all, "pp", "hosts.learned")
            .iter()
            .all(|l| l["what"] != "interactive"),
        "the confirmation did not mark the host: {all:#?}"
    );
    assert_eq!(evs(&all[from..], "pp", "hosts.held").len(), 0);
    assert_eq!(
        evs(all, "pp", "auth.prompt").len(),
        1,
        "only the confirmation was asked"
    );
    let attempts = evs(&all[from..], "pp", "boot.attempt");
    assert!(!attempts.is_empty());
    assert!(attempts
        .iter()
        .all(|a| a["interactive"] == false && a["armed"] == false));
    let entry = r.c.hosts().await.unwrap();
    assert!(
        !entry
            .iter()
            .find(|h| h.id == r.id)
            .unwrap()
            .interactive_auth
    );
}

/// A host marked as needing the user that later connects without asking anything (the user
/// took the passphrase off the key, or an agent has it) is no longer marked: the mark is
/// cleared on record, and its next drop is reconnected on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marked_host_that_connects_without_a_prompt_is_unmarked_and_reconnects_on_its_own() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    r.drop_network().await;
    until(&mut r.fleet, "pp parked", |e| parked(e, "pp", true)).await;

    // The user removes the passphrase from the key (the public key, and so the host's
    // authorization, stays), then connects.
    let out = std::process::Command::new("ssh-keygen")
        .args(["-q", "-p", "-P", PASSPHRASE, "-N", "", "-f"])
        .arg(&r.key)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    r.c.reconnect_host(&id).await.unwrap();
    let events = until_all(
        &mut r.fleet,
        "pp connected and unmarked",
        &[&|e| connected(e, "pp"), &|e| {
            interactive_mark(e, "pp", false)
        }],
    )
    .await;
    assert!(
        !events.iter().any(|e| matches!(
            e,
            FleetEvent::AuthPrompt {
                prompt: Some(_),
                ..
            }
        )),
        "nobody was asked: {events:?}"
    );
    let m = r.marker().await;
    let learned: Vec<&Value> = evs(&r.tail.seen[..m], "pp", "hosts.learned")
        .into_iter()
        .map(|r| &r["what"])
        .collect();
    let at = |word: &str| learned.iter().rposition(|w| **w == json!(word));
    assert!(
        at("not_interactive") > at("interactive"),
        "cleared after it was set: {learned:?}"
    );

    // Its next loss of the network is an ordinary one.
    r.drop_network().await;
    let events = collect_until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    assert!(events.iter().any(|e| reconnecting(e, "pp")), "{events:?}");
    assert!(!events.iter().any(|e| asks_or_waits(e, "pp")), "{events:?}");
}

/// The user's click is spent when an attempt asks them something, not when it starts: an armed
/// attempt that fails before it reaches the host (nothing listens) asked nobody, so it is tried
/// again on the normal schedule, armed, and nobody is shown a prompt or told to sign in; when
/// the host is reachable again the very next attempt asks, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_armed_attempt_that_never_reached_the_host_is_retried_armed_and_asks_once_when_it_does()
{
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    r.drop_network().await;
    until(&mut r.fleet, "pp parked", |e| parked(e, "pp", true)).await;
    let m = r.marker().await;

    r.sshd.stop();
    r.c.reconnect_host(&id).await.unwrap();
    let seen = std::cell::Cell::new(0);
    let events = collect_until(&mut r.fleet, "two failed attempts", |e| {
        if reconnecting(e, "pp") {
            seen.set(seen.get() + 1);
        }
        seen.get() == 2
    })
    .await;
    assert!(
        !events.iter().any(|e| asks_or_waits(e, "pp")),
        "no prompt, and not 'sign-in needed' for a network problem: {events:?}"
    );
    let m2 = r.marker().await;
    let down = &r.tail.seen[m + 1..m2];
    let attempts = evs(down, "pp", "boot.attempt");
    assert!(attempts.len() >= 2, "{down:#?}");
    assert!(
        attempts.iter().all(|a| a["armed"] == true),
        "still armed after every failure: {attempts:#?}"
    );
    assert_eq!(evs(down, "pp", "auth.prompt").len(), 0);
    assert_eq!(evs(down, "pp", "hosts.held").len(), 0);

    // Reachable again: the next attempt asks, once.
    r.sshd.start_again();
    let p = prompt(&mut r.fleet, "pp").await;
    r.c.answer_auth(&id, p.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;
    let m3 = r.marker().await;
    let after = &r.tail.seen[m + 1..m3];
    assert_eq!(
        evs(after, "pp", "auth.prompt").len(),
        1,
        "asked once in all"
    );
    // Spent: the next loss of the network waits for the user again.
    r.drop_network().await;
    until(&mut r.fleet, "pp parked again", |e| parked(e, "pp", true)).await;
}

/// A prompt belongs to its attempt. The attempt's ssh is killed while the user has not answered:
/// the prompt is taken off the fleet at that moment, answering it is refused, the helper that
/// outlived ssh is sent away (the prompt's end is on record as its attempt ending), and the
/// next click's prompt is a new one that the old one cannot touch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prompt_of_an_ended_attempt_is_withdrawn_and_cannot_be_answered() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    r.drop_network().await;
    until(&mut r.fleet, "pp parked", |e| parked(e, "pp", true)).await;

    r.c.reconnect_host(&id).await.unwrap();
    let p1 = prompt(&mut r.fleet, "pp").await;
    // The attempt dies under it (its sign-in step is the ssh that asked).
    let pid = r.link_pid().await;
    kill_pid(pid);
    // The prompt is taken off, and the attempt that failed after asking parks the link (this time
    // because it failed): the fleet reports the two in either order, the prompt's end by its own
    // task and the link's state by the supervisor's.
    until_all(
        &mut r.fleet,
        "the prompt withdrawn and pp parked after the failure",
        &[
            &|e| matches!(e, FleetEvent::AuthPrompt { host, prompt: None } if *host == id),
            &|e| parked(e, "pp", false),
        ],
    )
    .await;
    assert!(
        r.c.answer_auth(&id, p1.id, PASSPHRASE).await.is_err(),
        "a prompt of an ended attempt cannot be answered"
    );
    let end = r
        .tail
        .until("the prompt's end", |x| {
            x["ev"] == "auth.cancelled" && x["prompt"] == p1.id
        })
        .await;
    assert_eq!(r.tail.seen[end]["why"], "attempt_ended");

    // The next click asks again, and its prompt is its own.
    r.c.reconnect_host(&id).await.unwrap();
    let p2 = prompt(&mut r.fleet, "pp").await;
    assert_ne!(p2.id, p1.id);
    assert!(r.c.answer_auth(&id, p1.id, "x").await.is_err());
    r.c.answer_auth(&id, p2.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;
}

/// Removing a host takes its prompt off at once and refuses what is left of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_host_withdraws_its_prompt() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    r.drop_network().await;
    until(&mut r.fleet, "pp parked", |e| parked(e, "pp", true)).await;
    r.c.reconnect_host(&id).await.unwrap();
    let p = prompt(&mut r.fleet, "pp").await;

    r.c.remove_host(&id).await.unwrap();
    until(
        &mut r.fleet,
        "pp removed",
        |e| matches!(e, FleetEvent::HostRemoved { id: h } if *h == id),
    )
    .await;
    let end = r
        .tail
        .until("the prompt's end", |x| {
            x["ev"] == "auth.cancelled" && x["prompt"] == p.id
        })
        .await;
    assert_eq!(r.tail.seen[end]["why"], "attempt_ended");
    assert!(r.c.answer_auth(&id, p.id, PASSPHRASE).await.is_err());
    // Nothing of it is left in a new subscriber's snapshot.
    let mut fresh = r.c.fleet(None);
    match next(&mut fresh).await.1 {
        FleetEvent::Reset { hosts } => assert!(hosts.iter().all(|h| h.host.id != id)),
        other => panic!("{other:?}"),
    }
}

/// The one click covers the restart of a helper that runs another build: its second connect
/// attempt rides the shared connection the first one opened, so the user is asked once, with no
/// second prompt (a hub without a shared connection asks again, and says why: the `repeat` of the
/// prompt, covered in `auth.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helper_restart_within_one_sign_in_rides_the_shared_connection_and_asks_nothing_more() {
    let mut r = Rig::new().await;
    // The host already has our binary installed, but a daemon claiming another build runs.
    let bin = r.sshd.installed_agent();
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(agent(), &bin).unwrap();
    let mut old = tokio::process::Command::new(&bin)
        .arg("serve")
        .env("HOME", &r.sshd.remote_home)
        .env("MARSAD_RUNTIME_DIR", r.sshd.runtime_dir())
        .env("MARSAD_BUILD_ID", "0.0.0+old")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(
        &mut tokio::io::BufReader::new(old.stdout.take().unwrap()),
        &mut line,
    )
    .await
    .unwrap();
    assert!(line.contains("0.0.0+old"), "{line}");

    r.add(passphrase_kind(&r.sshd, &r.key)).await;
    let p1 = prompt(&mut r.fleet, "pp").await;
    assert_eq!(p1.repeat, None);
    r.c.answer_auth(&r.id, p1.id, PASSPHRASE).await.unwrap();
    // No second prompt: the restart attempt rides the connection the first one opened (a
    // second prompt would have to arrive before the host connects).
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;
    let st = tokio::time::timeout(T, old.wait())
        .await
        .expect("the old daemon exited")
        .unwrap();
    assert!(st.success());

    let m = r.marker().await;
    let all = &r.tail.seen[..m];
    let prompts = evs(all, "pp", "auth.prompt");
    assert_eq!(prompts.len(), 1, "{prompts:#?}");
    assert!(prompts[0]["repeat"].is_null());
    assert_eq!(r.askpass_calls(), 1, "ssh asked the helper once in all");
    // Two attempts (the first saw the old build), one sign-in step, one shared connection.
    assert!(evs(all, "pp", "boot.attempt").len() >= 2);
    assert_eq!(evs(all, "pp", "ssh.signin_started").len(), 1, "{all:#?}");
    assert_eq!(evs(&r.tail.seen[..m], "pp", "boot.build_mismatch").len(), 1);
}

// ── the shared connection ────────────────────────────────────────────────────

use wasit_fake_cli::harness::PidWatch;

impl Rig {
    /// Adds the host as a key host (no passphrase: nobody is ever asked) and waits for it.
    async fn connect_key_host(&mut self) {
        self.add(self.sshd.kind(&self.sshd.key)).await;
        until(&mut self.fleet, "pp connected", |e| connected(e, "pp")).await;
    }

    /// The pid of the process that holds the host's shared connection.
    fn master_pid(&self) -> u32 {
        master_pid(&self.master()).expect("the shared connection is open")
    }

    /// Records of `ev` about the host made after `from` and before a record made now.
    async fn window(&mut self, from: usize) -> Vec<Value> {
        let m = self.marker().await;
        self.tail.seen[from..m].to_vec()
    }
}

/// Waits for `pid` to end (a pidfd: the process's own end, not a schedule).
async fn gone(pid: u32) {
    if let Some(w) = PidWatch::open(pid) {
        assert!(w.exited(T).await, "process {pid} did not end");
    }
}

/// THE ask. The passphrase is asked once; the hub daemon is stopped and started again (the
/// shared connection is ssh's, and outlives it): the host connects again with no prompt, no
/// sign-in step and no wait for the user, over the very same connection, and the session that
/// waited for a gate before the restart is reachable and finishes its turn with no gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hub_restart_finds_the_open_connection_and_reconnects_without_a_prompt() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    assert_eq!(r.askpass_calls(), 1);
    let pid = r.master_pid();
    let id = r.id.clone();
    let g = launch_gated(&r).await;
    until(&mut r.fleet, "the session in the fleet", |e| {
        matches!(e, FleetEvent::SessionUpserted { host, session } if *host == id && session.id == g.id && session.phase.name() == "needs_you")
    })
    .await;
    let seen = g.seen.clone();
    let gid = g.id.clone();
    let request_id = g.request_id.clone();
    drop(g);

    let mut r = r.restart_hub().await;
    // Everything the fleet says until the host is connected: nobody is asked, nothing waits.
    let events = collect_until(&mut r.fleet, "pp connected", |e| {
        connected(e, "pp")
            || matches!(e, FleetEvent::Reset { hosts } if hosts.iter().any(|h| h.host.id.as_str() == "pp" && matches!(&h.link, FleetLink::Remote { state } if state.is_connected())))
    })
    .await;
    assert!(
        !events.iter().any(|e| asks_or_waits(e, "pp")),
        "nobody is asked and nothing waits: {events:?}"
    );
    assert_eq!(r.master_pid(), pid, "the very same connection");
    assert_eq!(r.askpass_calls(), 1, "no further askpass call");
    let m = r.marker().await;
    let all = &r.tail.seen[..m];
    assert_eq!(evs(all, "pp", "auth.prompt").len(), 0, "{all:#?}");
    assert_eq!(evs(all, "pp", "ssh.signin_started").len(), 0);
    assert_eq!(evs(all, "pp", "hosts.held").len(), 0);
    let checks = evs(all, "pp", "ssh.master_check");
    assert!(
        checks.iter().any(|c| c["state"] == "alive"),
        "the hub asked the connection: {checks:#?}"
    );
    let attempts = evs(all, "pp", "boot.attempt");
    assert!(!attempts.is_empty());
    assert!(
        attempts
            .iter()
            .all(|a| a["interactive"] == true && a["armed"] == false),
        "an interactive host, tried without anybody's click: {attempts:#?}"
    );

    // The session is reachable again through the new hub and finishes with no gap.
    let rt = r.c.runtime(Some(r.id.clone()));
    rt.command(
        &gid,
        Command::AnswerGate {
            request_id,
            decision: GateDecision::choose("allow"),
        },
    )
    .await
    .unwrap();
    let mut sub = rt.subscribe_session(&gid, None);
    loop {
        let e = session_event(&mut sub).await;
        if matches!(
            &e.kind,
            SessionEventKind::Agent {
                event: AgentEvent::TurnCompleted { .. }
            }
        ) {
            break;
        }
    }
    let page = rt.history(&gid, None, 2000).await.unwrap();
    assert_eq!(
        &page.events[..seen.len()],
        &seen[..],
        "no gap, no duplicate across the hub's restart"
    );
    rt.delete(&gid).await.unwrap();
}

/// How a test ends a shared connection while the host is connected.
#[derive(Clone, Copy)]
enum End {
    /// Its owner asks it to (`ssh -O exit`): it leaves no socket and says nothing.
    Asked,
    /// It is killed (a crash, a lost network): it leaves its socket behind and says nothing.
    Killed,
}

/// A connection that ends while the host is connected: the link drops and the host waits for the
/// user, with no prompt, saying that the shared connection ended; one Sign in click asks exactly
/// once and the host connects again.
async fn the_connection_ends(how: End) {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let id = r.id.clone();
    let calls = r.askpass_calls();
    r.marker().await;
    let from = r.tail.seen.len();
    match how {
        End::Asked => {
            let pid = r.master_pid();
            assert!(master_exit(&r.master()), "ssh -O exit");
            gone(pid).await;
        }
        End::Killed => {
            kill_master(&r.master()).await;
        }
    }
    let state = until_link(&mut r.fleet, "pp", |s| {
        matches!(s, LinkState::AuthNeeded { .. })
    })
    .await;
    let LinkState::AuthNeeded {
        detail,
        cause,
        held,
        ..
    } = state
    else {
        unreachable!()
    };
    assert!(held, "it waits for the user; no attempt failed");
    assert!(
        detail.contains("shared connection") && detail.contains("ended"),
        "{detail:?}"
    );
    assert!(
        cause.is_some_and(|c| !c.is_empty()),
        "the drop's own reason is kept"
    );
    assert_eq!(r.askpass_calls(), calls, "nobody was asked");
    let window = r.window(from).await;
    none_of(
        &window,
        "pp",
        &["boot.attempt", "auth.prompt", "ssh.signin_started"],
    );
    let ended = evs(&window, "pp", "ssh.master_ended");
    assert_eq!(ended.len(), 1, "{window:#?}");
    assert!(
        ended[0]["n"].is_null(),
        "found by the supervisor's question"
    );
    let found = evs(&window, "pp", "ssh.master_check");
    // A connection asked to end removes its socket as it goes (`absent`), or has not quite when the
    // hub looks (`stale`: nothing listens any more); a killed one leaves its socket behind.
    let want: &[&str] = match how {
        End::Asked => &["absent", "stale"],
        End::Killed => &["stale"],
    };
    assert!(
        found
            .iter()
            .any(|c| want.contains(&c["state"].as_str().unwrap_or(""))),
        "{want:?}: {found:#?}"
    );

    // One click: one sign-in step, one prompt, connected.
    r.c.reconnect_host(&id).await.unwrap();
    let p = prompt(&mut r.fleet, "pp").await;
    assert_eq!((p.kind, p.attempt, p.repeat), (PromptKind::Secret, 1, None));
    r.c.answer_auth(&id, p.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    assert_eq!(r.askpass_calls(), calls + 1, "exactly one more");
    let window = r.window(from).await;
    assert_eq!(evs(&window, "pp", "auth.prompt").len(), 1);
    assert_eq!(evs(&window, "pp", "ssh.signin_started").len(), 1);
    let done = evs(&window, "pp", "ssh.signin_done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["asked"], 1);
    assert!(
        master_pid(&r.master()).is_some(),
        "a connection is open again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_ended_by_its_owner_parks_the_host_and_one_click_asks_exactly_once() {
    the_connection_ends(End::Asked).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_killed_connection_parks_the_host_and_one_click_asks_exactly_once() {
    the_connection_ends(End::Killed).await;
}

/// A connection that dies because the network does not answer (every process of the host's sshd
/// stops, so nothing ever arrives): ssh's own keep-alive ends it (15 s × 3 of silence) and writes
/// why into the connection's log; the hub reads that, shows it in the words of the wait for the
/// user, and records it in the trail (scrubbed, bounded). No prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_that_times_out_says_why_in_the_wait_and_the_trail() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let calls = r.askpass_calls();
    r.marker().await;
    let from = r.tail.seen.len();
    let _frozen = Frozen::stop(r.sshd.connection_pids());
    let state = until_link(&mut r.fleet, "pp", |s| {
        matches!(s, LinkState::AuthNeeded { .. })
    })
    .await;
    let LinkState::AuthNeeded {
        detail,
        held,
        cause,
        problem,
    } = state
    else {
        unreachable!()
    };
    // The link's own silence limit (45 s) and ssh's keep-alive (15 s x 3) end about together: the
    // supervisor either asks the connection first (held) or an attempt that rode it for a
    // moment fails when it ends. Either way the wait says what ssh wrote.
    let _ = (held, &problem, &cause);
    assert!(
        detail.contains("not responding"),
        "ssh's own reason is in the words: {detail:?}"
    );
    assert_eq!(r.askpass_calls(), calls);
    let window = r.window(from).await;
    let ended = evs(&window, "pp", "ssh.master_ended");
    assert_eq!(ended.len(), 1, "{window:#?}");
    let reason = ended[0]["reason"].as_str().expect("a reason");
    assert!(reason.contains("not responding"), "{reason:?}");
    assert!(reason.chars().count() <= 400 + 1, "bounded: {reason:?}");
    none_of(&window, "pp", &["auth.prompt", "ssh.signin_started"]);
}

/// A link that drops while its shared connection stays open (the remote daemon crashed, the
/// bridge ended: here the riding ssh is killed) reconnects by itself, with no prompt and no
/// sign-in, although the host needs the user at login: no factor is needed to ride a connection
/// that is open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_drop_with_an_open_connection_reconnects_by_itself_even_for_a_passphrase_host() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let calls = r.askpass_calls();
    let master = r.master_pid();
    let link = r.link_pid().await;
    assert_ne!(link, master);
    let from = r.tail.seen.len();
    kill_pid(link);
    let events = collect_until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    assert!(events.iter().any(|e| reconnecting(e, "pp")), "{events:?}");
    assert!(
        !events.iter().any(|e| asks_or_waits(e, "pp")),
        "nobody is asked and nothing waits: {events:?}"
    );
    assert_eq!(r.askpass_calls(), calls);
    assert_eq!(r.master_pid(), master, "the same connection carries it");
    let window = r.window(from).await;
    none_of(
        &window,
        "pp",
        &["hosts.held", "auth.prompt", "ssh.signin_started"],
    );
    let attempts = evs(&window, "pp", "boot.attempt");
    assert!(!attempts.is_empty());
    assert!(attempts
        .iter()
        .all(|a| a["interactive"] == true && a["armed"] == false));
    assert!(evs(&window, "pp", "ssh.master_check")
        .iter()
        .any(|c| c["state"] == "alive"));
}

/// A key host gets a shared connection too, and keeps it across a restart of the hub: the same
/// process carries the new hub's link, whose attempt opens nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_host_gets_a_shared_connection_and_keeps_it_across_a_hub_restart() {
    let mut r = Rig::new().await;
    r.connect_key_host().await;
    let sock = r.master();
    let pid = r.master_pid();
    assert_eq!(r.askpass_calls(), 0, "a key host is asked nothing");
    // The socket is private to the user, in the runtime directory's own `ssh` directory.
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = sock.parent().unwrap();
        assert_eq!(dir.file_name().unwrap(), "ssh");
        assert_eq!(
            std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let mut r = r.restart_hub().await;
    until_link(&mut r.fleet, "pp", LinkState::is_connected).await;
    assert_eq!(r.master_pid(), pid, "the connection of the first hub");
    let m = r.marker().await;
    let all = &r.tail.seen[..m];
    assert_eq!(evs(all, "pp", "ssh.signin_started").len(), 0, "{all:#?}");
    assert!(evs(all, "pp", "ssh.master_check")
        .iter()
        .any(|c| c["state"] == "alive"));
    let hosts = r.c.hosts().await.unwrap();
    let e = hosts.iter().find(|h| h.id == r.id).unwrap();
    assert!(e.shared_connection, "the hub says the host has one");
}

/// Removing a host ends its shared connection (the socket and its log are gone); editing its route
/// ends the old one and the new route has its own: another socket, another process. Both are
/// on record, and the link of the edited host is connected again over the new connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_host_ends_its_connection_and_an_edited_route_gets_a_new_one() {
    let mut r = Rig::new().await;
    r.connect_key_host().await;
    let id = r.id.clone();
    let (sock1, pid1) = (r.master(), r.master_pid());
    let from = r.tail.seen.len();

    // An edit of the route (an ssh option).
    let mut e =
        r.c.hosts()
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.id == id)
            .unwrap();
    let HostKind::Ssh { options, .. } = &mut e.kind else {
        unreachable!()
    };
    options.push("LogLevel=ERROR".into());
    r.kind = Some(e.kind.clone());
    r.c.update_host(e).await.unwrap();
    gone(pid1).await;
    assert!(master_pid(&sock1).is_none(), "the old connection is gone");
    assert!(!sock1.exists(), "and its socket");
    assert!(!marsad_agent::hub::transport::master_log(&sock1).exists());
    until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    let (sock2, pid2) = (r.master(), r.master_pid());
    assert_ne!(sock1, sock2);
    assert_ne!(pid1, pid2);

    // Removing the host.
    r.c.remove_host(&id).await.unwrap();
    gone(pid2).await;
    assert!(master_pid(&sock2).is_none());
    assert!(!sock2.exists());
    assert!(!marsad_agent::hub::transport::master_log(&sock2).exists());
    let end = r
        .tail
        .until("the removal", |x| {
            x["ev"] == "hosts.changed" && x["host"] == "pp" && x["op"] == "removed"
        })
        .await;
    let window = r.tail.seen[from..=end].to_vec();
    let exits = evs(&window, "pp", "ssh.master_exit");
    let words: Vec<(&str, &str)> = exits
        .iter()
        .map(|x| (x["why"].as_str().unwrap(), x["outcome"].as_str().unwrap()))
        .collect();
    assert_eq!(words, [("retargeted", "ended"), ("removed", "ended")]);
}

/// Signing out ends the connection, the link drops and the host waits for the user, key host or
/// not, and is not tried again by itself: no attempt, no ssh, until the user connects it; then it
/// signs in again over a new connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_ends_the_connection_and_the_host_waits_and_is_not_retried() {
    let mut r = Rig::new().await;
    r.connect_key_host().await;
    let id = r.id.clone();
    let (sock, pid) = (r.master(), r.master_pid());
    r.marker().await;
    let from = r.tail.seen.len();

    r.c.sign_out_host(&id).await.unwrap();
    gone(pid).await;
    let state = until_link(&mut r.fleet, "pp", |s| {
        matches!(s, LinkState::AuthNeeded { .. })
    })
    .await;
    let LinkState::AuthNeeded { detail, held, .. } = state else {
        unreachable!()
    };
    assert!(held && detail.contains("signed out"), "{held} {detail:?}");
    assert!(master_pid(&sock).is_none());
    assert!(!sock.exists());
    let window = r.window(from).await;
    none_of(
        &window,
        "pp",
        &["boot.attempt", "transport.spawned", "link.attempt_begin"],
    );
    let exits = evs(&window, "pp", "ssh.master_exit");
    assert_eq!(exits.len(), 1, "{window:#?}");
    assert_eq!(
        (&exits[0]["why"], &exits[0]["outcome"]),
        (&json!("signed_out"), &json!("ended"))
    );
    assert_eq!(evs(&window, "pp", "hosts.held").len(), 1);

    // Not an ssh host, or not a host: refused.
    assert!(r.c.sign_out_host(&HostId::new("hub")).await.is_err());
    assert!(r.c.sign_out_host(&HostId::new("nobody")).await.is_err());

    // The user connects it again: a new connection, a new sign-in step.
    r.c.reconnect_host(&id).await.unwrap();
    until(&mut r.fleet, "pp connected again", |e| connected(e, "pp")).await;
    assert_ne!(r.master_pid(), pid);
    let window = r.window(from).await;
    assert_eq!(evs(&window, "pp", "ssh.signin_started").len(), 1);
}
