//! The guard of the test hygiene: a test that fails leaves nothing behind. The processes a
//! daemon test starts are detached on purpose (a session's shim, its guard and the CLI survive
//! their daemon; the hub's daemon outlives its client; a "remote" machine's daemon outlives the
//! ssh that started it), so the harness ends them itself (`TempDir`'s drop, which runs while a
//! failing test unwinds). This test runs a scenario with all of those — a hub daemon started by
//! its client with a session, and an ssh host (a real sshd on localhost) with its daemon and a
//! session — and panics in the middle; then it asserts that every process the scenario started
//! has ended (including the ssh and sshd processes of the link), that its temporary directory
//! is gone, and that no runtime directory of its data root is left in the machine's default
//! place (`/tmp/marsad-<uid>/<tag>`, where the real installation's daemon lives). It checks
//! only the processes and directories it started itself, named by their own reports.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::*;
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::HostDraft;
use marsad_proto::layout::Layout;
use marsad_proto::{HostId, LinkState};
use wasit_fake_cli::harness::PidWatch;
use wasit_fake_cli::scenario::Scenario;

/// What the failing scenario started, noted as it goes.
#[derive(Default)]
struct Started {
    root: PathBuf,
    /// The place the hub client's own layout puts its runtime directory when it is not told
    /// another one.
    default_run: PathBuf,
    /// A handle on each process, opened while it was alive.
    watches: Vec<(String, PidWatch)>,
}

impl Started {
    fn note(&mut self, what: impl Into<String>, pid: u32) {
        let what = what.into();
        let watch = PidWatch::open(pid).unwrap_or_else(|| panic!("{what} ({pid}) is not alive"));
        self.watches.push((what, watch));
    }
}

/// The parent of `pid` (the guard is the parent of the CLI).
fn parent_of(pid: u32) -> u32 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // `pid (comm) state ppid …`; the name may hold spaces and parentheses.
    let after = &stat[stat.rfind(')').unwrap() + 2..];
    after.split(' ').nth(1).unwrap().parse().unwrap()
}

/// The processes below `pid` (children, their children, …): the ssh of a host's link, the
/// connections of an sshd.
fn descendants(pid: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut todo = vec![pid];
    while let Some(p) = todo.pop() {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{p}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let kids = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
            for kid in kids
                .split_whitespace()
                .filter_map(|k| k.parse::<u32>().ok())
            {
                found.push(kid);
                todo.push(kid);
            }
        }
    }
    found
}

/// The directory of the shim of session `id` below `base` (`<base>/**/sessions/<id>`).
fn session_dir(base: &Path, id: &str) -> PathBuf {
    let mut todo = vec![base.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == id) && dir.ends_with("sessions") {
                if p.join(wasit_shim::LOCK_FILE).exists() {
                    return p;
                }
            } else if e.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push(p);
            }
        }
    }
    panic!("no shim directory of session {id} below {}", base.display());
}

/// An idle session on `host` (`None`: the hub itself), its shim's processes noted.
async fn session(
    started: &Mutex<Started>,
    c: &marsad_client::MarsadClient,
    host: Option<HostId>,
    work: &Path,
    shims_below: &Path,
    label: &str,
) {
    let mut req = LaunchRequest::new("claude");
    req.host = host.clone();
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(work, &Scenario::default()),
        ..OverrideLayer::default()
    };
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(host);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        let e = match tokio::time::timeout(T, sub.next()).await {
            Ok(Some(Ok((_, e)))) => e,
            other => panic!(
                "no session event: {:?}",
                other.map(|o| o.map(|r| r.map(|(c, _)| c)))
            ),
        };
        if matches!(&e.kind, marsad_proto::runtime::SessionEventKind::Phase { phase, .. } if phase.name() == "idle")
        {
            break;
        }
    }
    let shim = wasit_shim::ShimClient::connect(&session_dir(shims_below, id.as_str()))
        .await
        .unwrap();
    let (shim_pid, cli) = (shim.shim_pid(), shim.child_pid());
    let mut s = started.lock().unwrap();
    s.note(format!("{label} shim"), shim_pid);
    s.note(format!("{label} guard"), parent_of(cli));
    s.note(format!("{label} CLI"), cli);
}

async fn failing_scenario(started: Arc<Mutex<Started>>) {
    let d = TempDir::new();
    let data = d.path().join("l");
    {
        let mut s = started.lock().unwrap();
        s.root = d.path().to_owned();
        s.default_run = default_run_dir(&data);
    }
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&data, &[]);

    // The hub's own session.
    session(&started, &c, None, &work, &layout(&data).run, "hub").await;
    let hub = c.daemon_info(None).await.unwrap().pid;
    started.lock().unwrap().note("hub daemon", hub);

    // An ssh host: its link (ssh and the sshd's connection), its daemon and a session.
    let mut fleet = c.fleet(None);
    let box_id = HostId::new("box");
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(box_id.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "box connected", |e| {
        matches!(e, FleetEvent::Link { host, state } if *host == box_id && LinkState::is_connected(state))
    })
    .await;
    let remote_work = sshd.remote_home.join("w");
    std::fs::create_dir_all(&remote_work).unwrap();
    session(
        &started,
        &c,
        Some(box_id.clone()),
        &remote_work,
        &sshd.dir.join("run"),
        "box",
    )
    .await;
    let remote = c.daemon_info(Some(&box_id)).await.unwrap().pid;
    {
        let mut s = started.lock().unwrap();
        s.note("box daemon", remote);
        s.note("sshd", sshd.pid());
        for p in descendants(hub).into_iter().chain(descendants(sshd.pid())) {
            if let Some(w) = PidWatch::open(p) {
                s.watches.push((format!("link process {p}"), w));
            }
        }
    }

    panic!("the test fails in the middle");
}

/// Where `Layout::resolve(root, None)` puts the runtime directory of `root`, without creating
/// it: the formula is checked against the product on a root nobody uses.
fn default_run_dir(root: &Path) -> PathBuf {
    let tag = |r: &Path| {
        wasit_runtime::machine::key_from_raw(&format!("root:{}", r.display()))[..8].to_string()
    };
    let base = wasit_runtime::machine::runtime_dir("marsad", None).unwrap();
    let probe = std::env::temp_dir().join(format!("mrs-probe-{}", std::process::id()));
    let actual = Layout::resolve(probe.clone(), None).unwrap().run;
    assert_eq!(
        actual,
        base.join(tag(&probe)),
        "the runtime directory formula changed"
    );
    std::fs::remove_dir(&actual).unwrap();
    base.join(tag(root))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_test_leaves_no_process_and_no_directory_behind() {
    let started = Arc::new(Mutex::new(Started::default()));
    // On a task of its own, so that its panic unwinds its locals (the harness's drop runs)
    // while this test goes on.
    let outcome = tokio::spawn(failing_scenario(started.clone())).await;
    assert!(outcome.unwrap_err().is_panic(), "the scenario must panic");

    let started = started.lock().unwrap();
    assert!(
        started.watches.len() >= 10,
        "the scenario started {} processes",
        started.watches.len()
    );
    for (what, watch) in &started.watches {
        assert!(
            watch.is_gone(),
            "the {what} is still running when its test has failed"
        );
    }
    assert!(
        !started.root.exists(),
        "{} was not removed",
        started.root.display()
    );
    assert!(
        !started.default_run.exists(),
        "{} was left in the machine's runtime directory",
        started.default_run.display()
    );
}
