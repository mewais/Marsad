//! A host that needs the user at login (a key with a passphrase, asked for through the real
//! askpass helper by a real ssh to a real sshd on localhost, as a password or a one-time code
//! would be): it is never reconnected on its own, its link says so and why it dropped, one click
//! asks once, sessions on it keep running and come back; and the facts that decide it are
//! learned and un-learned from what a sign-in actually asked. No root: a passphrase is asked
//! the same way a password is (an entry through askpass), and what the hub does with it does
//! not depend on which. The password and second-factor tests that need root stay in
//! `remote.rs`.
#![cfg(target_os = "linux")]

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::{FleetEvent, FleetLink};
use marsad_proto::hosts::{HostDraft, HostKind, PromptKind, SignInRepeat};
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
        }
    }

    async fn add(&mut self, kind: HostKind) {
        self.c
            .add_host(HostDraft {
                name: "pp".into(),
                kind,
                id: Some(self.id.clone()),
            })
            .await
            .unwrap();
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
        } = self;
        drop(fleet);
        drop(tail);
        drop(c);
        stop(&data).await;
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
/// connect and the host is marked. Its ssh dies: the link parks as "sign-in needed" at once
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

    // The link drops.
    let pid = r.link_pid().await;
    let from = r.tail.seen.len();
    kill_pid(pid);
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

/// A hub that starts with a stored passphrase host does not ask for it: no attempt, no ssh, the
/// link is parked at once with the same published state as after a drop (there is no drop to
/// quote), and one click connects it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hub_that_starts_with_a_passphrase_host_parks_it_without_an_ssh() {
    let mut r = Rig::new().await;
    r.sign_in().await;
    let mut r = r.restart_hub().await;
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

    // Its ssh dies: it reconnects by itself, through "reconnecting", asking nobody.
    let pid = r.link_pid().await;
    let from = r.tail.seen.len();
    kill_pid(pid);
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
    let pid = r.link_pid().await;
    kill_pid(pid);
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

    // Its next drop is an ordinary one.
    let pid = r.link_pid().await;
    kill_pid(pid);
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
    let pid = r.link_pid().await;
    kill_pid(pid);
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
    // Spent: the next drop waits for the user again.
    let pid = r.link_pid().await;
    kill_pid(pid);
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
    let pid = r.link_pid().await;
    kill_pid(pid);
    until(&mut r.fleet, "pp parked", |e| parked(e, "pp", true)).await;

    r.c.reconnect_host(&id).await.unwrap();
    let p1 = prompt(&mut r.fleet, "pp").await;
    // The attempt dies under it.
    let pid = r.link_pid().await;
    kill_pid(pid);
    until(
        &mut r.fleet,
        "the prompt withdrawn",
        |e| matches!(e, FleetEvent::AuthPrompt { host, prompt: None } if *host == id),
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
    // The attempt failed after asking: the link waits, and this time because it failed.
    until(&mut r.fleet, "pp parked after the failure", |e| {
        parked(e, "pp", false)
    })
    .await;

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
    let pid = r.link_pid().await;
    kill_pid(pid);
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

/// The one click covers a sign-in that needs a second connection (the hub restarts the helper on
/// the host after the first connect showed it runs another build): the second prompt comes
/// without a second click, and says why it is a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_connection_within_one_sign_in_says_why_it_asks_again() {
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
    // No click in between: the hub connects again to restart the helper, and the user is asked
    // again, told why.
    let p2 = prompt(&mut r.fleet, "pp").await;
    assert_ne!(p2.id, p1.id);
    assert_eq!(p2.repeat, Some(SignInRepeat::HelperRestart));
    assert_eq!(p2.attempt, 1, "a new connection, not a wrong answer");
    r.c.answer_auth(&r.id, p2.id, PASSPHRASE).await.unwrap();
    until(&mut r.fleet, "pp connected", |e| connected(e, "pp")).await;
    let st = tokio::time::timeout(T, old.wait())
        .await
        .expect("the old daemon exited")
        .unwrap();
    assert!(st.success());

    let m = r.marker().await;
    let prompts = evs(&r.tail.seen[..m], "pp", "auth.prompt");
    assert_eq!(prompts.len(), 2, "{prompts:#?}");
    assert!(prompts[0]["repeat"].is_null());
    assert_eq!(prompts[1]["repeat"], "helper_restart");
    assert_eq!(evs(&r.tail.seen[..m], "pp", "boot.build_mismatch").len(), 1);
}
