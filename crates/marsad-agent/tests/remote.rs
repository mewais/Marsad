//! Remote hosts over REAL ssh to a real sshd on localhost (separate HOME = the "remote"
//! machine): provisioning from scratch and on build mismatch, the fleet across local + ssh
//! host with a link drop and no lost session events, git on the remote host, `auth_needed`
//! for a wrong key, unknown host key confirmation through askpass, and (as root, opt-in)
//! password / password-as-second-factor sign-in with wrong answer, cancel, timeout and no
//! automatic reconnect after an interactive link drops.
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

fn kill_ssh(port: u16) {
    pkill(&format!("^ssh -T .*-p {port} "));
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
        matches!(e, FleetEvent::SessionUpserted { host, session } if *host == box_id && session.id == id && session.phase.name() == "needs_you")
    })
    .await;

    // Drop the link (kill ssh): key auth reconnects on its own; nothing is lost.
    kill_ssh(sshd.port);
    until(&mut fleet, "box reconnecting", |e| {
        link_is(e, "box", |s| !s.is_connected())
    })
    .await;
    until(&mut fleet, "box connected again", |e| {
        link_is(e, "box", LinkState::is_connected)
    })
    .await;
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
        matches!(e, FleetEvent::SessionUpserted { host, session } if *host == box_id && session.id == id && session.turns_completed == 1)
    })
    .await;
    rt.delete(&id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn build_mismatch_restarts_the_remote_daemon() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    // The host already has our binary installed, but a daemon claiming another build runs.
    let bin = sshd.installed_agent();
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(agent(), &bin).unwrap();
    let mut old = tokio::process::Command::new(&bin)
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

    let c = hub_client(&d.path().join("l"), &[]);
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "box".into(),
        kind: sshd.kind(&sshd.key),
        id: None,
    })
    .await
    .unwrap();
    until(&mut fleet, "provisioning for a build mismatch", |e| {
        link_is(e, "box", |s| {
            matches!(
                s,
                LinkState::Provisioning {
                    reason: marsad_proto::ProvisionReason::BuildMismatch { .. }
                }
            )
        })
    })
    .await;
    let connected = until(&mut fleet, "connected", |e| {
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
    // The mismatch is on record: what the hub is and what it found running.
    drop(fleet);
    drop(c);
    let data = d.path().join("l");
    let out = tokio::process::Command::new(agent())
        .arg("stop")
        .args(data_args(&data))
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    let log = std::fs::read_to_string(layout(&data).diag().join("daemon.ndjson")).unwrap();
    let mismatch: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|r| r["ev"] == "boot.build_mismatch")
        .collect();
    assert_eq!(mismatch.len(), 1, "{log}");
    assert_eq!(mismatch[0]["host"], "box");
    assert_eq!(mismatch[0]["found"], "0.0.0+old");
    assert_ne!(mismatch[0]["expected"], "0.0.0+old");
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

    // The link drops: no automatic reconnect, no new prompt — it waits for "Connect".
    kill_ssh(sshd.port);
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

/// Runs the real POSIX bootstrap on the sshd host (the exact command the hub sends) with
/// `input` as the hub's side of the conversation, then closes stdin.
async fn bootstrap_with(sshd: &Sshd, input: Vec<u8>) -> std::process::Output {
    use tokio::io::AsyncWriteExt;
    let entry = marsad_proto::hosts::HostEntry {
        id: HostId::new("t"),
        name: "t".into(),
        kind: sshd.kind(&sshd.key),
        platform: None,
        created_at_ms: 0,
        interactive_auth: false,
        local_files: false,
        machine: None,
    };
    let argv = marsad_agent::hub::transport::bootstrap_argv(
        &entry,
        marsad_agent::hub::transport::RemoteShell::Posix,
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
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&input).await.unwrap();
    drop(stdin);
    tokio::time::timeout(T, child.wait_with_output())
        .await
        .expect("the bootstrap ended")
        .unwrap()
}

/// The remote end of an upload installs the helper only when it received exactly the number of
/// bytes announced: `head -c` ends cleanly on a short input, so a truncated upload used to be
/// moved into place. A complete one is still installed and started.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bootstrap_installs_only_a_complete_upload() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let dir = sshd.installed_agent().parent().unwrap().to_path_buf();

    // 400 of the announced 1000 bytes, then the connection ends cleanly.
    let mut short = b"UPLOAD 1000\n".to_vec();
    short.extend(std::iter::repeat_n(b'x', 400));
    let out = bootstrap_with(&sshd, short).await;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(70), "stderr: {stderr}");
    assert!(
        stderr.contains("received 400 of 1000 bytes of the helper; not installed"),
        "stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("MARSAD-BOOT "), "{stdout}");
    assert!(!stdout.contains("MARSAD-OK"), "{stdout}");
    assert!(
        !sshd.installed_agent().exists(),
        "a truncated helper was installed"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");

    // The whole file: installed, answered, started (the "helper" is a script that says so).
    let payload = b"#!/bin/sh\necho PAYLOAD-RAN\n";
    let mut whole = format!("UPLOAD {}\n", payload.len()).into_bytes();
    whole.extend_from_slice(payload);
    let out = bootstrap_with(&sshd, whole).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(lines.len(), 3, "{stdout}");
    assert!(lines[0].starts_with("MARSAD-BOOT "));
    assert_eq!(&lines[1..], ["MARSAD-OK", "PAYLOAD-RAN"]);
    assert_eq!(std::fs::read(sshd.installed_agent()).unwrap(), payload);
}
