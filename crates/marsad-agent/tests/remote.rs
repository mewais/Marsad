//! Remote hosts over REAL ssh to a real sshd on localhost (separate HOME = the "remote"
//! machine): provisioning from scratch and on build mismatch, the fleet across local + ssh
//! host with a link drop and no lost session events, git on the remote host, `auth_needed`
//! for a wrong key, unknown host key confirmation through askpass, and (as root, opt-in)
//! password / password-as-second-factor sign-in with wrong answer, cancel, timeout and no
//! automatic reconnect after the network to an interactive host is lost (its shared connection
//! dies).
#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

use common::*;
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::git::GitTarget;
use marsad_proto::hosts::{HostDraft, HostKind, PromptKind};
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, Command, CommandFailureKind, GateDecision, HostId, LinkState};
use serde_json::json;
use wasit_fake_cli::scenario::{Scenario, Turn};

/// The network to a host is lost: the shared connection of the host `id` of route `kind` dies
/// without a word (its process is killed, its socket is left behind), and the link that rode it
/// ends with it.
async fn lose_network(data: &Path, id: &str, kind: &HostKind) {
    kill_master(&master_socket(data, id, kind)).await;
}

async fn session_event(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(T, sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        other => panic!(
            "no session event: {:?}",
            other.map(|o| o.map(|r| r.map(|(c, _)| c)))
        ),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_host_provision_fleet_git_and_link_drop() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&d.path().join("l"), &[]);
    let mut fleet = c.fleet(None);
    match next(&mut fleet).await.1 {
        FleetEvent::Reset { hosts } => assert_eq!(hosts[0].host.id, HostId::new("hub")),
        other => panic!("expected reset, got {other:?}"),
    }
    let box_id = HostId::new("box");
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(box_id.clone()),
    })
    .await
    .unwrap();
    // From scratch: nothing installed; the one connection uploads and starts the daemon.
    until(&mut fleet, "box connected", |e| {
        link_is(e, "box", LinkState::is_connected)
    })
    .await;
    let v = std::process::Command::new(sshd.installed_agent())
        .arg("--version")
        .output()
        .unwrap();
    let ours = std::process::Command::new(agent())
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        v.stdout, ours.stdout,
        "the hub's build is installed on the host"
    );
    let hosts = c.hosts().await.unwrap();
    let entry = hosts.iter().find(|h| h.id == box_id).unwrap();
    assert_eq!(entry.platform.as_ref().unwrap().os, "linux");
    assert!(!entry.interactive_auth);

    // git on the remote host (routed by host id).
    let repo = sshd.remote_home.join("repo");
    git_repo(&repo);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    let st = c
        .git_status(Some(&box_id), GitTarget::cwd(repo.to_string_lossy()))
        .await
        .unwrap();
    assert_eq!(st.files[0].path, "a.txt");
    c.git_commit(
        Some(&box_id),
        GitTarget::cwd(repo.to_string_lossy()),
        "remote commit",
        vec!["a.txt".into()],
    )
    .await
    .unwrap();
    assert!(c
        .git_status(Some(&box_id), GitTarget::cwd(repo.to_string_lossy()))
        .await
        .unwrap()
        .files
        .is_empty());

    // A session on the remote host, waiting on a gate.
    let work = sshd.remote_home.join("w");
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
    req.host = Some(box_id.clone());
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &sc),
        ..OverrideLayer::default()
    };
    let launched = c.launch(req).await.unwrap();
    assert_eq!(launched.host, box_id);
    let id = launched.id;
    let rt = c.runtime(Some(box_id.clone()));
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
    until(&mut fleet, "the remote session in the fleet", |e| {
        session_is(e, box_id.as_str(), &id, |s| s.phase.name() == "needs_you")
    })
    .await;

    // The network is lost (the shared connection dies, and the link with it): key auth
    // reconnects on its own, over a new connection; nothing is lost.
    let old_master = master_pid(&master_socket(
        &d.path().join("l"),
        "box",
        &sshd.kind(&sshd.key),
    ));
    lose_network(&d.path().join("l"), "box", &sshd.kind(&sshd.key)).await;
    until(&mut fleet, "box reconnecting", |e| {
        link_is(e, "box", |s| !s.is_connected())
    })
    .await;
    until(&mut fleet, "box connected again", |e| {
        link_is(e, "box", LinkState::is_connected)
    })
    .await;
    let new_master = master_pid(&master_socket(
        &d.path().join("l"),
        "box",
        &sshd.kind(&sshd.key),
    ));
    assert!(
        new_master.is_some() && new_master != old_master,
        "a new shared connection carries the link: {old_master:?} -> {new_master:?}"
    );
    rt.command(
        &id,
        Command::AnswerGate {
            request_id,
            decision: GateDecision::choose("allow"),
        },
    )
    .await
    .unwrap();
    loop {
        let e = session_event(&mut sub).await;
        let done = matches!(
            &e.kind,
            SessionEventKind::Agent {
                event: AgentEvent::TurnCompleted { .. }
            }
        );
        seen.push(e);
        if done {
            break;
        }
    }
    let page = rt.history(&id, None, 2000).await.unwrap();
    assert_eq!(
        &page.events[..seen.len()],
        &seen[..],
        "no gap, no duplicate across the link drop"
    );
    until(&mut fleet, "the finished turn in the fleet", |e| {
        session_is(e, box_id.as_str(), &id, |s| s.turns_completed == 1)
    })
    .await;
    rt.delete(&id).await.unwrap();
}

/// A host set up by an earlier Marsad: its helper in the flat layout of builds before per-build
/// directories, and its daemon of another build running, started without a hub id. ONE
/// connection upgrades it: the report says what runs, the hub uploads this build, the new
/// helper restarts the old daemon (waiting for its exit) and starts this build's, under this
/// host's id. No provisioning round, no second connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_helper_and_daemon_are_upgraded_on_one_connection() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let legacy = sshd
        .remote_home
        .join(".marsad/bin")
        .join(platform_dir())
        .join("marsad-agent");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::copy(agent(), &legacy).unwrap();
    let mut old = tokio::process::Command::new(&legacy)
        .arg("serve")
        .env("HOME", &sshd.remote_home)
        .env("MARSAD_RUNTIME_DIR", sshd.runtime_dir())
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

    let data = d.path().join("l");
    let c = hub_client(&data, &[]);
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "box".into(),
        kind: sshd.kind(&sshd.key),
        id: None,
    })
    .await
    .unwrap();
    let connected = until(&mut fleet, "connected", |e| {
        assert!(
            !link_is(e, "box", |s| matches!(s, LinkState::Provisioning { .. })),
            "no provisioning round: {e:?}"
        );
        link_is(e, "box", LinkState::is_connected)
    })
    .await;
    let FleetEvent::Link {
        state: LinkState::Connected { hello },
        ..
    } = connected
    else {
        unreachable!()
    };
    assert_ne!(hello.build, "0.0.0+old");
    assert_eq!(
        hello.host_id.as_str(),
        "box",
        "the restarted daemon knows itself by the hub's id for it"
    );
    let st = tokio::time::timeout(T, old.wait())
        .await
        .expect("the old daemon exited")
        .unwrap();
    assert!(st.success());
    assert!(sshd.installed_agent().is_file());
    drop(fleet);
    drop(c);
    let out = tokio::process::Command::new(agent())
        .arg("stop")
        .args(data_args(&data))
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    // One attempt: an upload, then a restart, on the same connection.
    let log = std::fs::read_to_string(layout(&data).diag().join("daemon.ndjson")).unwrap();
    let recs: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|r| r["host"] == "box")
        .collect();
    let actions: Vec<(&serde_json::Value, &serde_json::Value)> = recs
        .iter()
        .filter(|r| r["ev"] == "boot.action")
        .map(|r| (&r["n"], &r["action"]))
        .collect();
    assert_eq!(
        actions,
        [
            (&json!(1), &json!("upload")),
            (&json!(1), &json!("restart"))
        ],
        "{log}"
    );
    assert!(recs.iter().all(|r| r["ev"] != "boot.build_mismatch"));
}

/// A second hub on the same remote account is refused, in words, while the first hub's daemon
/// runs there: it is not taken over (and the first hub's link stays up).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_hub_is_refused_while_the_first_hubs_daemon_runs() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let first = hub_client(&d.path().join("one"), &[]);
    let mut f1 = first.fleet(None);
    first
        .add_host(HostDraft {
            name: "box".into(),
            kind: sshd.kind(&sshd.key),
            id: Some(HostId::new("box")),
        })
        .await
        .unwrap();
    until(&mut f1, "the first hub connected", |e| {
        link_is(e, "box", LinkState::is_connected)
    })
    .await;
    let second = hub_client(&d.path().join("two"), &[]);
    let mut f2 = second.fleet(None);
    second
        .add_host(HostDraft {
            name: "box".into(),
            kind: sshd.kind(&sshd.key),
            id: Some(HostId::new("box")),
        })
        .await
        .unwrap();
    let parked = until(&mut f2, "the second hub refused", |e| {
        link_is(e, "box", |s| matches!(s, LinkState::Failed { .. }))
    })
    .await;
    let FleetEvent::Link {
        state: LinkState::Failed { reason, problem },
        ..
    } = parked
    else {
        unreachable!()
    };
    assert!(reason.contains("serves another Marsad hub"), "{reason}");
    assert_eq!(problem, Some(CommandFailureKind::HelperFailed));
    // The first hub's link was never touched.
    assert!(first
        .hosts()
        .await
        .unwrap()
        .iter()
        .any(|h| h.id == HostId::new("box")));
    let rt = first.runtime(Some(HostId::new("box")));
    rt.list()
        .await
        .expect("the first hub still reaches the host");
}

/// A slow host never fails for being slow: its login (here every `uname` takes a second, so the
/// report comes seconds late) and its upload (here `head`, which receives it, starts seconds
/// late) only take longer. There is no deadline on an attempt; its progress is its liveness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_host_connects_however_slow_its_login_and_upload() {
    use std::os::unix::fs::PermissionsExt;
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let bin = sshd.dir.join("bin");
    for (name, real) in [("uname", "uname"), ("head", "head")] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(real))
            .find(|p| p.is_file())
            .unwrap();
        let delay = if name == "uname" { 1 } else { 4 };
        std::fs::write(
            bin.join(name),
            format!("#!/bin/sh\nsleep {delay}\nexec {} \"$@\"\n", real.display()),
        )
        .unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let data = d.path().join("l");
    let c = hub_client(&data, &["--auth-prompt-timeout-ms", "500"]);
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "slow".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(HostId::new("slow")),
    })
    .await
    .unwrap();
    until(&mut fleet, "the slow host connected", |e| {
        assert!(
            !link_is(e, "slow", |s| matches!(
                s,
                LinkState::Failed { .. } | LinkState::Reconnecting { .. }
            )),
            "a slow host failed: {e:?}"
        );
        link_is(e, "slow", LinkState::is_connected)
    })
    .await;
    assert!(sshd.installed_agent().is_file());
}

/// A shared connection left by a host that is gone (removed, or its route edited, while no hub
/// ran) is ended when the hub starts: asked once, then told to exit; nothing is matched by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orphan_shared_connection_is_ended_when_the_hub_starts() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let data = d.path().join("l");
    let ssh_dir = layout(&data).ssh_dir();
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let orphan = ssh_dir.join("0123456789abcdef");
    let HostKind::Ssh {
        target,
        port,
        identity_file,
        options,
    } = sshd.kind(&sshd.key)
    else {
        unreachable!()
    };
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(["-M", "-N", "-f", "-o", "ControlPersist=yes", "-o"])
        .arg(format!("ControlPath={}", orphan.display()))
        .args([
            "-p",
            &port.unwrap().to_string(),
            "-i",
            &identity_file.unwrap(),
        ]);
    for o in options {
        cmd.args(["-o", &o]);
    }
    assert!(cmd.arg(target).status().unwrap().success());
    assert!(master_pid(&orphan).is_some(), "the orphan is open");
    // A socket name that is not ours is left alone.
    std::fs::write(ssh_dir.join("not-ours"), b"").unwrap();
    let c = hub_client(&data, &[]);
    c.hosts().await.unwrap();
    assert!(master_pid(&orphan).is_none(), "the orphan was ended");
    assert!(!orphan.exists());
    assert!(ssh_dir.join("not-ours").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_key_is_auth_needed_and_host_key_confirmation_goes_through_askpass() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&d.path().join("l"), &[]);
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "bad".into(),
        kind: sshd.kind(&sshd.wrong_key),
        id: None,
    })
    .await
    .unwrap();
    let e = until(&mut fleet, "auth needed", |e| {
        link_is(e, "bad", |s| matches!(s, LinkState::AuthNeeded { .. }))
    })
    .await;
    let FleetEvent::Link {
        state: LinkState::AuthNeeded { problem, .. },
        ..
    } = e
    else {
        unreachable!()
    };
    assert_eq!(problem, CommandFailureKind::AuthNeeded);

    // Unknown host key: ssh asks (StrictHostKeyChecking=ask) through the askpass helper.
    let kh = d.path().join("empty_known_hosts");
    std::fs::write(&kh, "").unwrap();
    let HostKind::Ssh {
        target,
        port,
        identity_file,
        ..
    } = sshd.kind(&sshd.key)
    else {
        unreachable!()
    };
    c.add_host(HostDraft {
        name: "fresh".into(),
        kind: HostKind::Ssh {
            target,
            port,
            identity_file,
            options: vec![
                format!("UserKnownHostsFile={}", kh.display()),
                "StrictHostKeyChecking=ask".into(),
                "GlobalKnownHostsFile=/dev/null".into(),
            ],
        },
        id: None,
    })
    .await
    .unwrap();
    let p = prompt(&mut fleet, "fresh").await;
    assert_eq!(p.kind, PromptKind::Confirm, "{}", p.prompt);
    assert!(p.prompt.contains("continue connecting"), "{}", p.prompt);
    c.answer_auth(&HostId::new("fresh"), p.id, "yes")
        .await
        .unwrap();
    until(&mut fleet, "fresh connected", |e| {
        link_is(e, "fresh", LinkState::is_connected)
    })
    .await;
    assert!(std::fs::read_to_string(&kh)
        .unwrap()
        .contains("ssh-ed25519"));
}

const TEST_USER: &str = "marsadt";
const TEST_PASSWORD: &str = "s3cret-Pw";

/// Create the password test user (root only; a deliberate change of the test machine).
fn ensure_test_user() -> std::path::PathBuf {
    assert!(
        String::from_utf8_lossy(
            &std::process::Command::new("id")
                .arg("-u")
                .output()
                .unwrap()
                .stdout
        )
        .trim()
            == "0",
        "interactive sign-in tests need root (they create the user {TEST_USER} with a password)"
    );
    let exists = std::process::Command::new("id")
        .arg(TEST_USER)
        .output()
        .unwrap()
        .status
        .success();
    if !exists {
        let st = std::process::Command::new("useradd")
            .args(["-m", "-s", "/bin/sh", TEST_USER])
            .status()
            .unwrap();
        assert!(st.success());
    }
    let mut ch = std::process::Command::new("chpasswd")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        ch.stdin.as_mut().unwrap(),
        format!("{TEST_USER}:{TEST_PASSWORD}\n").as_bytes(),
    )
    .unwrap();
    assert!(ch.wait().unwrap().success());
    pkill(&format!("/home/{TEST_USER}/.marsad"));
    let home = std::path::PathBuf::from(format!("/home/{TEST_USER}"));
    let _ = std::fs::remove_dir_all(home.join(".marsad"));
    home
}

fn user_kind(sshd: &Sshd, key: Option<&Path>) -> HostKind {
    HostKind::Ssh {
        target: format!("{TEST_USER}@127.0.0.1"),
        port: Some(sshd.port),
        identity_file: key.map(|k| k.display().to_string()),
        options: vec![
            format!("UserKnownHostsFile={}", sshd.known_hosts.display()),
            "StrictHostKeyChecking=yes".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "PubkeyAuthentication=".to_string() + if key.is_some() { "yes" } else { "no" },
        ],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs root: creates a local user with a password for real password sign-in"]
async fn password_signin_wrong_answer_cancel_timeout_and_no_auto_reconnect() {
    let home = ensure_test_user();
    let d = TempDir::new();
    let sshd = Sshd::start_custom(
        d.path(),
        "pw",
        "PasswordAuthentication yes\nAuthenticationMethods password",
        false,
    );
    let c = hub_client(&d.path().join("l"), &["--auth-prompt-timeout-ms", "4000"]);
    let mut fleet = c.fleet(None);
    let id = HostId::new("pw");
    c.add_host(HostDraft {
        name: "pw".into(),
        kind: user_kind(&sshd, None),
        id: Some(id.clone()),
    })
    .await
    .unwrap();

    // Wrong answer, then the right one: ssh asks again (attempt 2) on the same connection.
    let p = prompt(&mut fleet, "pw").await;
    assert_eq!((p.kind, p.attempt), (PromptKind::Secret, 1), "{}", p.prompt);
    assert!(p.prompt.contains("password"), "{}", p.prompt);
    c.answer_auth(&id, p.id, "wrong").await.unwrap();
    let p = prompt(&mut fleet, "pw").await;
    assert_eq!(p.attempt, 2);
    c.answer_auth(&id, p.id, TEST_PASSWORD).await.unwrap();
    until(&mut fleet, "pw connected", |e| {
        link_is(e, "pw", LinkState::is_connected)
    })
    .await;
    assert!(home.join(common::common_bin_rel()).is_file());
    assert!(
        c.hosts()
            .await
            .unwrap()
            .iter()
            .find(|h| h.id == id)
            .unwrap()
            .interactive_auth
    );

    // The network is lost: no automatic reconnect, no new prompt — it waits for "Connect".
    lose_network(&d.path().join("l"), "pw", &user_kind(&sshd, None)).await;
    until(&mut fleet, "auth needed after the drop", |e| {
        link_is(e, "pw", |s| matches!(s, LinkState::AuthNeeded { .. }))
    })
    .await;
    let mut fresh = c.fleet(None);
    match next(&mut fresh).await.1 {
        FleetEvent::Reset { hosts } => {
            let h = hosts.iter().find(|h| h.host.id == id).unwrap();
            assert!(h.auth_prompt.is_none(), "no prompt was raised");
            assert!(matches!(
                h.link,
                marsad_proto::fleet::FleetLink::Remote {
                    state: LinkState::AuthNeeded { .. }
                }
            ));
        }
        other => panic!("{other:?}"),
    }

    // Connect → cancel → auth needed.
    c.reconnect_host(&id).await.unwrap();
    let p = prompt(&mut fleet, "pw").await;
    c.cancel_auth(&id, p.id).await.unwrap();
    until(&mut fleet, "auth needed after cancel", |e| {
        link_is(e, "pw", |s| matches!(s, LinkState::AuthNeeded { .. }))
    })
    .await;

    // Connect → nobody answers → the prompt times out → auth needed.
    c.reconnect_host(&id).await.unwrap();
    prompt(&mut fleet, "pw").await;
    until(
        &mut fleet,
        "the prompt timed out",
        |e| matches!(e, FleetEvent::AuthPrompt { host, prompt: None } if *host == id),
    )
    .await;
    until(&mut fleet, "auth needed after timeout", |e| {
        link_is(e, "pw", |s| matches!(s, LinkState::AuthNeeded { .. }))
    })
    .await;

    // Connect → right answer → connected again.
    c.reconnect_host(&id).await.unwrap();
    let p = prompt(&mut fleet, "pw").await;
    c.answer_auth(&id, p.id, TEST_PASSWORD).await.unwrap();
    until(&mut fleet, "pw connected again", |e| {
        link_is(e, "pw", LinkState::is_connected)
    })
    .await;
    c.remove_host(&id).await.unwrap();
    pkill(&format!("/home/{TEST_USER}/.marsad"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs root: creates a local user with a password (second factor after the key)"]
async fn key_plus_password_second_factor() {
    ensure_test_user();
    let d = TempDir::new();
    let sshd = Sshd::start_custom(
        d.path(),
        "mfa",
        "PasswordAuthentication yes\nAuthenticationMethods publickey,password",
        false,
    );
    let c = hub_client(&d.path().join("l"), &[]);
    let mut fleet = c.fleet(None);
    let id = HostId::new("mfa");
    c.add_host(HostDraft {
        name: "mfa".into(),
        kind: user_kind(&sshd, Some(&sshd.key)),
        id: Some(id.clone()),
    })
    .await
    .unwrap();
    let p = prompt(&mut fleet, "mfa").await;
    assert_eq!(p.kind, PromptKind::Secret);
    c.answer_auth(&id, p.id, TEST_PASSWORD).await.unwrap();
    until(&mut fleet, "mfa connected", |e| {
        link_is(e, "mfa", LinkState::is_connected)
    })
    .await;
    c.remove_host(&id).await.unwrap();
    pkill(&format!("/home/{TEST_USER}/.marsad"));
}

/// The real bootstrap on the sshd host (the exact command the hub sends), driven line by line
/// as the hub would.
struct BootSession {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
}

impl BootSession {
    async fn start(sshd: &Sshd) -> BootSession {
        let entry = marsad_proto::hosts::HostEntry {
            id: HostId::new("t"),
            name: "t".into(),
            kind: sshd.kind(&sshd.key),
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        let argv = marsad_agent::hub::transport::bootstrap_argv_via(
            &entry,
            boot_target("t", "0123456789abcdef"),
            marsad_agent::hub::transport::SshRoute::Plain,
        )
        .unwrap();
        let mut child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut s = BootSession {
            child,
            stdin,
            stdout,
        };
        assert_eq!(s.line().await, "MARSAD-SH");
        let script =
            marsad_agent::hub::transport::posix_script(boot_target("t", "0123456789abcdef"));
        s.send(format!("{script}\n").as_bytes()).await;
        s
    }

    async fn line(&mut self) -> String {
        let mut l = String::new();
        tokio::time::timeout(
            T,
            tokio::io::AsyncBufReadExt::read_line(&mut self.stdout, &mut l),
        )
        .await
        .expect("a line from the host")
        .unwrap();
        l.trim_end().to_owned()
    }

    async fn send(&mut self, bytes: &[u8]) {
        use tokio::io::AsyncWriteExt;
        let w = self.stdin.as_mut().unwrap();
        w.write_all(bytes).await.unwrap();
        w.flush().await.unwrap();
    }

    /// Closes stdin and waits for the end: exit status and the rest of stdout.
    async fn end(mut self) -> (std::process::ExitStatus, String, String) {
        use tokio::io::AsyncReadExt;
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).await.unwrap();
        let mut err = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .await
            .unwrap();
        let st = tokio::time::timeout(T, self.child.wait())
            .await
            .expect("the bootstrap ended")
            .unwrap();
        (st, rest, err)
    }
}

fn sha256(bytes: &[u8]) -> String {
    let p = std::env::temp_dir().join(format!("mrs-sha-{}", std::process::id()));
    std::fs::write(&p, bytes).unwrap();
    let h = marsad_agent::bootwire::sha256_file(&p).unwrap();
    std::fs::remove_file(&p).unwrap();
    h
}

/// The remote end of an upload installs the helper only when it received exactly the bytes
/// announced, with the SHA-256 announced: a short upload (`head -c` ends cleanly on a short
/// input) and a corrupted one are refused with `MARSAD-FAIL`, nothing is installed and nothing
/// is left behind. A complete, matching one is renamed into place and started as the helper of
/// the hub's build, told it was just installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bootstrap_installs_only_a_complete_upload_with_the_announced_hash() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let dir = sshd.installed_agent().parent().unwrap().to_path_buf();
    let payload = b"#!/bin/sh\necho PAYLOAD-RAN \"$@\"\n".to_vec();
    let left = || -> Vec<std::ffi::OsString> {
        std::fs::read_dir(&dir)
            .map(|r| r.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default()
    };

    // 400 of the announced 1000 bytes, then the connection ends cleanly.
    let mut s = BootSession::start(&sshd).await;
    let report = s.line().await;
    assert!(report.starts_with("MARSAD-BOOT2 "), "{report}");
    assert!(report.contains(" helper_build=none "), "{report}");
    assert!(report.contains(" hasher=sha256sum"), "{report}");
    let mut short = format!("UPLOAD 1000 {}\n", sha256(&[b'x'; 1000])).into_bytes();
    short.extend(std::iter::repeat_n(b'x', 400));
    s.send(&short).await;
    let (st, out, err) = s.end().await;
    assert_eq!(st.code(), Some(70), "{err}");
    assert_eq!(
        out.trim(),
        "MARSAD-FAIL stage=install reason=received%20400%20of%201000%20bytes%20of%20the%20helper"
    );
    assert!(!sshd.installed_agent().exists());
    assert!(left().is_empty(), "left behind: {:?}", left());

    // All the bytes, another hash: refused.
    let mut s = BootSession::start(&sshd).await;
    s.line().await;
    let mut bad = format!("UPLOAD {} {}\n", payload.len(), sha256(b"something else")).into_bytes();
    bad.extend_from_slice(&payload);
    s.send(&bad).await;
    let (st, out, _) = s.end().await;
    assert_eq!(st.code(), Some(70));
    assert!(
        out.starts_with("MARSAD-FAIL stage=install reason=checksum%20mismatch"),
        "{out}"
    );
    assert!(!sshd.installed_agent().exists());
    assert!(left().is_empty(), "left behind: {:?}", left());

    // The whole file with its hash: installed and started (the "helper" says so).
    let mut s = BootSession::start(&sshd).await;
    s.line().await;
    let mut whole = format!("UPLOAD {} {}\n", payload.len(), sha256(&payload)).into_bytes();
    whole.extend_from_slice(&payload);
    s.send(&whole).await;
    assert_eq!(
        s.line().await,
        "PAYLOAD-RAN boot --host-id t --hub-id 0123456789abcdef --installed"
    );
    let (st, _, err) = s.end().await;
    assert!(st.success(), "{err}");
    assert_eq!(std::fs::read(sshd.installed_agent()).unwrap(), payload);
    assert_eq!(left(), [std::ffi::OsString::from("marsad-agent")]);
}

/// A host without any SHA-256 tool says so in its report (`hasher=none`), and an upload sent
/// anyway is refused: nothing unchecked is ever installed. Run with `sh` here (the same script),
/// with a `PATH` holding only the other tools it uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_checksum_tool_nothing_is_installed() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let d = TempDir::new();
    let home = d.path().join("home");
    let tools = d.path().join("tools");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tools).unwrap();
    for t in ["uname", "head", "wc", "tr", "mkdir", "chmod", "mv", "rm"] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(t))
            .find(|p| p.is_file())
            .unwrap();
        std::os::unix::fs::symlink(real, tools.join(t)).unwrap();
    }
    let script = marsad_agent::hub::transport::posix_script(boot_target("t", "0123456789abcdef"));
    let mut child = tokio::process::Command::new("/bin/sh")
        .args(["-c", marsad_agent::hub::transport::SH_STARTER])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", &tools)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap());
    let mut l = String::new();
    out.read_line(&mut l).await.unwrap();
    assert_eq!(l.trim(), "MARSAD-SH");
    stdin
        .write_all(format!("{script}\n").as_bytes())
        .await
        .unwrap();
    l.clear();
    out.read_line(&mut l).await.unwrap();
    assert!(l.trim_end().ends_with(" hasher=none"), "{l}");
    stdin.write_all(b"UPLOAD 3 abc\nxyz").await.unwrap();
    drop(stdin);
    let mut rest = String::new();
    out.read_to_string(&mut rest).await.unwrap();
    assert_eq!(
        rest.trim(),
        "MARSAD-FAIL stage=install reason=no%20checksum%20tool"
    );
    assert_eq!(child.wait().await.unwrap().code(), Some(70));
    assert!(!home.join(".marsad").exists(), "nothing was written");
}

/// The daemon is ready before the connection becomes the bridge's: when the helper answers
/// `MARSAD-OK` (here after `RUN` with no daemon running), its daemon already serves, started by
/// the helper, which read its readiness line; nothing after that has to wait for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_helper_says_ok_only_once_its_daemon_is_ready() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    std::fs::create_dir_all(sshd.installed_agent().parent().unwrap()).unwrap();
    std::fs::copy(agent(), sshd.installed_agent()).unwrap();
    let mut s = BootSession::start(&sshd).await;
    let report = s.line().await;
    let r = marsad_agent::bootwire::Report::parse(&marsad_agent::bootwire::Line::parse(&report));
    assert_eq!(r.helper_build, Some(marsad_agent::version::build_id()));
    assert_eq!(r.daemon, marsad_agent::bootwire::DaemonState::Absent);
    s.send(b"RUN\n").await;
    assert_eq!(
        s.line().await,
        "MARSAD-OK stop=none install=none daemon=started"
    );
    // Right now, before anything else is sent: the daemon is there (`stop` finds and ends it).
    let remote = remote_data(&sshd);
    let out = tokio::process::Command::new(agent())
        .arg("stop")
        .args(["--data", &remote.display().to_string()])
        .env("HOME", &sshd.remote_home)
        .env("MARSAD_RUNTIME_DIR", sshd.runtime_dir())
        .output()
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "stopped",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    drop(s);
}

fn remote_data(sshd: &Sshd) -> std::path::PathBuf {
    sshd.remote_home.join(".marsad")
}

/// Something holds the host daemon's endpoint and does not answer as a daemon: the report says
/// so, the hub decides on a restart, and the helper's stop cannot end it. That failure is the
/// host's answer (`MARSAD-FAIL stage=stop`), shown as the reason of the parked link; the helper
/// does not go on to start a second daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_that_fails_is_the_reason_of_the_link() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let socket = remote_socket(&sshd);
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let squatter = tokio::spawn(async move {
        loop {
            let Ok((mut conn, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = conn.write_all(&[0xff; 64]).await;
                let mut sink = [0u8; 256];
                while tokio::io::AsyncReadExt::read(&mut conn, &mut sink)
                    .await
                    .is_ok_and(|n| n > 0)
                {}
            });
        }
    });
    let data = d.path().join("l");
    let c = hub_client(&data, &[]);
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(HostId::new("box")),
    })
    .await
    .unwrap();
    let parked = until(&mut fleet, "parked", |e| {
        link_is(e, "box", |s| matches!(s, LinkState::Failed { .. }))
    })
    .await;
    let FleetEvent::Link {
        state: LinkState::Failed { reason, .. },
        ..
    } = parked
    else {
        unreachable!()
    };
    assert!(
        reason.contains("the host's bootstrap failed (stop): cannot reach the daemon"),
        "{reason}"
    );
    squatter.abort();
}

/// The socket of the remote account's daemon (its layout as the helper resolves it there).
fn remote_socket(sshd: &Sshd) -> std::path::PathBuf {
    let data = remote_data(sshd);
    let base = wasit_runtime::machine::runtime_dir("marsad", Some(&sshd.runtime_dir())).unwrap();
    let abs = std::path::absolute(&data).unwrap();
    let tag = wasit_runtime::machine::key_from_raw(&format!("root:{}", abs.display()));
    marsad_proto::layout::Layout::resolve(data, Some(base.join(&tag[..8])))
        .unwrap()
        .socket()
}

/// A link step is the bootstrap command riding a shared connection with `BatchMode=yes`. When the
/// connection is not there (it died between the hub's look and the step): a key host connects
/// directly, and a host that needs the user (here a passphrase) fails without asking anybody:
/// the helper that would ask is never called.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_step_without_a_connection_connects_a_key_and_never_asks_for_a_passphrase() {
    use marsad_agent::hub::transport::{bootstrap_argv_via, SshRoute};
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let pp_key = sshd.passphrase_key("pp-secret");
    let asked = d.path().join("asked");
    let helper = d.path().join("askpass.sh");
    std::fs::write(
        &helper,
        format!(
            "#!/bin/sh\necho \"$1\" >> {}\necho pp-secret\n",
            asked.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let control = d.path().join("ssh").join("none-here");
    let run = |kind: HostKind| {
        let helper = helper.clone();
        let control = control.clone();
        async move {
            let entry = marsad_proto::hosts::HostEntry {
                id: HostId::new("t"),
                name: "t".into(),
                kind,
                platform: None,
                created_at_ms: 0,
                interactive_auth: false,
                local_files: false,
                shared_connection: true,
                signed_out: false,
                machine: None,
            };
            let argv = bootstrap_argv_via(
                &entry,
                boot_target("t", "0123456789abcdef"),
                SshRoute::Link { control: &control },
            )
            .unwrap();
            let mut child = tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .env("SSH_ASKPASS", &helper)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", ":0")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let _ = child.stdin.take();
            tokio::time::timeout(T, child.wait_with_output())
                .await
                .expect("the link step ended")
                .unwrap()
        }
    };
    // A key host: no connection to ride, so it connects by itself (and reports).
    let out = run(sshd.kind(&sshd.key)).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("MARSAD-SH"),
        "{stdout} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A passphrase host: refused, nobody asked.
    let HostKind::Ssh {
        target,
        port,
        identity_file: _,
        mut options,
    } = sshd.kind(&pp_key)
    else {
        unreachable!()
    };
    options.extend(["IdentityAgent=none".into(), "AddKeysToAgent=no".into()]);
    let out = run(HostKind::Ssh {
        target,
        port,
        identity_file: Some(pp_key.display().to_string()),
        options,
    })
    .await;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(255), "{stderr}");
    assert!(stderr.contains("Permission denied"), "{stderr}");
    assert!(out.stdout.is_empty());
    assert!(
        !asked.exists(),
        "the helper was asked: {:?}",
        std::fs::read_to_string(&asked)
    );
}
