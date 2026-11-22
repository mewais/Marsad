//! Windows only.
//!
//! * No console windows: a hub daemon with NO console of its own (as when the GUI starts it)
//!   runs discovery probes, a shim-launched session, a `git` call, an ssh transport command and
//!   a ConPTY terminal; every process involved is the fake CLI (copied as `git.exe` / `ssh.exe`
//!   onto the daemon's PATH, or as the terminal's shell), which reports from inside whether it
//!   got a visible console window (`WASIT_FAKE_CONSOLE_REPORT`).
//! * WSL end to end (opt-in, `--ignored`; CI's windows-e2e job): `MARSAD_E2E_WSL_DISTRO` names
//!   an installed distro whose `/usr/local/bin/claude` is the Linux fake CLI, and
//!   `MARSAD_E2E_LINUX_BIN_DIR` holds `x86_64-unknown-linux-musl/marsad-agent`. The hub lists
//!   the distro, adds it as a host (provisioning the Linux helper into it), launches a session
//!   there, answers its gate, and the link comes back by itself after `wsl --terminate`.
//! * Native Windows over SSH (opt-in; windows-e2e runs OpenSSH Server on the runner and passes
//!   `MARSAD_E2E_WINSSH_KEY`): this machine as a Windows host through Marsad's real ssh link.
#![cfg(windows)]

mod common;

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::git::GitTarget;
use marsad_proto::hosts::{HostDraft, HostKind};
use marsad_proto::pty::PtyStartRequest;
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, Command, GateDecision, HostId, LinkState};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use wasit_fake_cli::scenario::{Scenario, Turn};

fn report_lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Wait until the report has more than `before` lines.
async fn more_reports(p: &Path, before: usize, what: &str) -> Vec<String> {
    let r = tokio::time::timeout(T, async {
        loop {
            let l = report_lines(p);
            if l.len() > before {
                return l;
            }
            // Test-only polling of a file other processes append to.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    r.unwrap_or_else(|_| panic!("no console report for {what}: {:?}", report_lines(p)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_opens_a_console_window_from_a_consoleless_hub() {
    let d = TempDir::new();
    let data = d.path().join("l");
    let report = d.path().join("console.txt");
    let bin = d.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["git.exe", "ssh.exe"] {
        std::fs::copy(fake(), bin.join(name)).unwrap();
    }
    let path = format!(
        "{};{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // The hub daemon gets NO console at all (DETACHED_PROCESS), like one started by a GUI app
    // with plain flags: every console program it starts would get a new, visible window unless
    // it is created without one.
    let mut daemon = std::process::Command::new(agent())
        .args(["serve", "--hub", "--data"])
        .arg(&data)
        .args(serve_args())
        .env("WASIT_FAKE_CONSOLE_REPORT", &report)
        .env("PATH", &path)
        .creation_flags(wasit_process::flags::DETACHED_PROCESS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let out = tokio::process::ChildStdout::from_std(daemon.stdout.take().unwrap()).unwrap();
    tokio::time::timeout(T, BufReader::new(out).read_line(&mut line))
        .await
        .expect("daemon readiness")
        .unwrap();
    assert!(line.contains("ready"), "{line:?}");
    let c = client(&data);

    // 1. Discovery probes.
    let before = report_lines(&report).len();
    c.runtime(None).inventory(None).await.unwrap();
    more_reports(&report, before, "discovery probes").await;

    // 2. A shim-launched fake CLI session.
    let before = report_lines(&report).len();
    let sc = Scenario::builder().turn(Turn::builder().say("hi")).build();
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(d.path().to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &sc),
        ..OverrideLayer::default()
    };
    let launched = c.launch(req).await.unwrap();
    more_reports(&report, before, "the session").await;

    // 3. A git call (`git.exe` is the fake: the call itself fails, which is fine).
    let before = report_lines(&report).len();
    let _ = c
        .git_status(None, GitTarget::cwd(d.path().to_string_lossy()))
        .await;
    more_reports(&report, before, "git").await;

    // 4. The transport command of an ssh host (`ssh.exe` is the fake).
    let before = report_lines(&report).len();
    c.add_host(HostDraft {
        name: "S".into(),
        kind: HostKind::Ssh {
            target: "nobody@127.0.0.1".into(),
            port: None,
            identity_file: None,
            options: vec![],
        },
        id: Some(HostId::new("s")),
    })
    .await
    .unwrap();
    more_reports(&report, before, "the ssh transport").await;

    // 5. A ConPTY terminal whose shell is the fake.
    let before = report_lines(&report).len();
    let pty = c
        .pty_start(
            None,
            PtyStartRequest {
                target: GitTarget::cwd(d.path().to_string_lossy()),
                cols: 80,
                rows: 24,
                shell: Some(fake().to_string_lossy().into_owned()),
            },
        )
        .await
        .unwrap();
    more_reports(&report, before, "the terminal").await;
    let _ = c.pty_close(None, &pty.id).await;

    // Control: the fake started WITHOUT wasit-process, in a console of its own
    // (CREATE_NEW_CONSOLE, what a console-less parent's plain spawn amounts to). The check must
    // see that window, or a clean result above would prove nothing.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let control = d.path().join("control.txt");
    let st = std::process::Command::new(fake())
        .arg("--version")
        .env("WASIT_FAKE_CONSOLE_REPORT", &control)
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    let control_lines = report_lines(&control);
    let lines = report_lines(&report);
    eprintln!("control (no wasit-process, exit {st}): {control_lines:?}");
    eprintln!("reports: {lines:#?}");
    assert!(
        control_lines.iter().any(|l| l.contains(" visible ")),
        "the check did not see the control's console window: {control_lines:?}"
    );
    let visible: Vec<_> = lines.iter().filter(|l| l.contains(" visible ")).collect();
    assert!(
        visible.is_empty(),
        "processes that got a visible console window: {visible:?}"
    );
    let _ = c.runtime(None).delete(&launched.id).await;
    let _ = daemon.kill();
}

// ── WSL end to end ──────────────────────────────────────────────────────────

fn link_is(e: &FleetEvent, host: &str, p: impl Fn(&LinkState) -> bool) -> bool {
    matches!(e, FleetEvent::Link { host: h, state } if h.as_str() == host && p(state))
}

async fn until(
    f: &mut Subscription<FleetEvent>,
    what: &str,
    pred: impl Fn(&FleetEvent) -> bool,
) -> FleetEvent {
    // One deadline for the whole wait: a first connect uploads the (debug) helper, during
    // which the link reports nothing for a while.
    let fut = async {
        loop {
            let e = match f.next().await {
                Some(Ok((_, e))) => e,
                other => panic!("fleet subscription ended: {:?}", other.map(|r| r.is_ok())),
            };
            if let FleetEvent::Link { host, state } = &e {
                eprintln!("link {host}: {state:?}");
            }
            if pred(&e) {
                return e;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(240), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
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

/// Launch the fake claude on `host` (inside WSL), prompt it and allow its gate.
async fn gate_round_trip(c: &MarsadClient, host: &HostId) {
    let sc = Scenario::builder()
        .turn(
            Turn::builder()
                .tool("Bash", json!({"command": "ls"}))
                .needs_permission()
                .result_if_allowed("listed")
                .done()
                .say("done in wsl"),
        )
        .build();
    let mut req = LaunchRequest::new("claude");
    req.host = Some(host.clone());
    req.cwd = Some("/tmp".into());
    req.overrides = OverrideLayer {
        env: vec![("WASIT_FAKE_SCENARIO_JSON".into(), sc.to_json())],
        ..OverrideLayer::default()
    };
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(Some(host.clone()));
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        let e = session_event(&mut sub).await;
        if matches!(&e.kind, SessionEventKind::Phase { phase } if phase.name() == "idle") {
            break;
        }
    }
    rt.command(&id, Command::prompt("go")).await.unwrap();
    let request_id = loop {
        if let SessionEventKind::Agent {
            event: AgentEvent::GateRequested { gate },
        } = session_event(&mut sub).await.kind
        {
            break gate.request_id().to_string();
        }
    };
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
        if let SessionEventKind::Agent {
            event: AgentEvent::TurnCompleted { .. },
        } = session_event(&mut sub).await.kind
        {
            break;
        }
    }
    rt.delete(&id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a WSL distro prepared by CI (MARSAD_E2E_WSL_DISTRO, MARSAD_E2E_LINUX_BIN_DIR)"]
async fn wsl_host_end_to_end() {
    let distro = std::env::var("MARSAD_E2E_WSL_DISTRO").expect("MARSAD_E2E_WSL_DISTRO");
    let bin_dir = std::env::var("MARSAD_E2E_LINUX_BIN_DIR").expect("MARSAD_E2E_LINUX_BIN_DIR");
    let d = TempDir::new();
    let mut cfg = marsad_client::ClientConfig::new(
        d.path().join("l"),
        wasit_control::Hello::new("test", "gui", "gui"),
    );
    cfg.agent_bin = Some(agent());
    cfg.serve_args = serve_args();
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    // The Linux helper was built separately; both claim one build id (WSLENV carries it into
    // the distro, where the bootstrap's `--version` and the daemon read it).
    cfg.daemon_env = vec![
        ("MARSAD_AGENT_BIN_DIR".into(), bin_dir),
        ("MARSAD_BUILD_ID".into(), "e2e-wsl".into()),
        ("WSLENV".into(), "MARSAD_BUILD_ID/u".into()),
    ];
    let c = MarsadClient::connect(cfg);

    let list = c.wsl_distros().await.unwrap();
    eprintln!("distros: {list:?}");
    assert!(list.available, "{list:?}");
    assert!(list.distros.iter().any(|x| x.name == distro), "{list:?}");

    // An unknown distro is refused up front, with the installed ones named.
    let err = c
        .add_host(HostDraft {
            name: "Nope".into(),
            kind: HostKind::Wsl {
                distro: "NoSuchDistro".into(),
            },
            id: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains(&distro), "{err}");

    let mut fleet = c.fleet(None);
    next(&mut fleet).await;
    let host = HostId::new("wsl");
    c.add_host(HostDraft {
        name: distro.clone(),
        kind: HostKind::Wsl {
            distro: distro.clone(),
        },
        id: Some(host.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "the WSL host connected", |e| {
        link_is(e, "wsl", LinkState::is_connected)
    })
    .await;
    let entry = c
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.id == host)
        .unwrap();
    assert_eq!(entry.platform.as_ref().unwrap().os, "linux");
    // A Windows hub reaches a WSL host's files directly.
    assert!(entry.local_files);

    gate_round_trip(&c, &host).await;
    wsl_files_are_reachable(&distro);

    // `kilo` lives only on a PATH that ~/.bashrc adds (CI installs it nvm-style): the host's
    // environment stage (login + interactive shell) makes discovery find it.
    let rt = c.runtime(Some(host.clone()));
    let env = rt.host_environment().await.unwrap();
    eprintln!("WSL environment: {env:?}");
    let kilo = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let inv = rt.inventory(None).await.unwrap();
            if let Some(k) = inv
                .drivers
                .into_iter()
                .find(|d| d.driver.as_str() == "kilo")
            {
                if !k.refreshing {
                    break k;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("kilo inventory");
    eprintln!("kilo: {kilo:?}");
    assert!(
        kilo.binary_path
            .as_deref()
            .is_some_and(|p| p.contains("/.nvm/")),
        "kilo not found on the rc-added PATH: {:?} (environment {env:?})",
        kilo.error
    );
    wsl_host_environment_settings(&c, &host, &distro).await;

    // Terminate the distro under the link: it reconnects by itself (a transport drop).
    let st = std::process::Command::new("wsl.exe")
        .args(["--terminate", &distro])
        .status()
        .unwrap();
    assert!(st.success());
    until(&mut fleet, "the link dropped", |e| {
        link_is(e, "wsl", |s| !s.is_connected())
    })
    .await;
    until(&mut fleet, "the link back", |e| {
        link_is(e, "wsl", LinkState::is_connected)
    })
    .await;
    gate_round_trip(&c, &host).await;
    if let Ok(i) = c.daemon_info(None).await {
        stop_pid(i.pid);
    }
}

/// A file created inside the distro is readable from Windows at the translated path, and the
/// shell's open/reveal resolution (`resolve_wsl`) accepts it (no app is launched); a missing
/// file and a relative path are refused with their own errors.
fn wsl_files_are_reachable(distro: &str) {
    use marsad_proto::paths::{resolve_wsl, wsl_to_windows, PathError};
    let dir = "/tmp/marsad e2e \u{e9}";
    let file = format!("{dir}/hello world.txt");
    let st = std::process::Command::new("wsl.exe")
        .args(["-d", distro, "--exec", "sh", "-c"])
        .arg(format!("mkdir -p '{dir}' && printf 'from-wsl' > '{file}'"))
        .status()
        .unwrap();
    assert!(st.success());
    let win = wsl_to_windows(distro, &file).unwrap();
    assert!(
        win.starts_with(&format!("\\\\wsl.localhost\\{distro}\\tmp\\")),
        "{win}"
    );
    assert_eq!(std::fs::read_to_string(&win).unwrap(), "from-wsl");
    let resolved = resolve_wsl(distro, &file).expect("desktop.open/reveal accept the path");
    assert_eq!(resolved.to_string_lossy(), win);
    // The directory is what `reveal` shows.
    assert!(resolve_wsl(distro, dir).unwrap().is_dir());
    assert!(matches!(
        resolve_wsl(distro, "/tmp/marsad-e2e-missing"),
        Err(PathError::NotReadable { .. })
    ));
    assert!(matches!(
        resolve_wsl(distro, "tmp/x"),
        Err(PathError::NotAbsolute(_))
    ));
}

// ── SSH from Windows to Linux ───────────────────────────────────────────────

/// The hub on Windows reaches a Linux machine over ssh (Windows' own `ssh.exe`): sshd runs
/// inside the WSL distro on port 2222 with key auth (CI's windows-e2e job installs it and
/// sets `SetEnv MARSAD_BUILD_ID=e2e-ssh`). The bootstrap reports `Linux x86_64`, the hub picks
/// the bundled musl helper (the owner's report: it looked for an `x86_64-unknown-linux-gnu`
/// build), uploads it, and a fake-CLI session's gate round-trips over the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs sshd in a WSL distro prepared by CI (MARSAD_E2E_WSL_DISTRO, MARSAD_E2E_SSH_KEY)"]
async fn ssh_host_from_windows_end_to_end() {
    let distro = std::env::var("MARSAD_E2E_WSL_DISTRO").expect("MARSAD_E2E_WSL_DISTRO");
    let bin_dir = std::env::var("MARSAD_E2E_LINUX_BIN_DIR").expect("MARSAD_E2E_LINUX_BIN_DIR");
    let key = std::env::var("MARSAD_E2E_SSH_KEY").expect("MARSAD_E2E_SSH_KEY");
    // sshd in the foreground for the test's duration (it also keeps the distro running).
    let mut sshd = std::process::Command::new("wsl.exe")
        .args([
            "-d",
            &distro,
            "-u",
            "root",
            "--exec",
            "sh",
            "-c",
            "mkdir -p /run/sshd && exec /usr/sbin/sshd -D -e -p 2222",
        ])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();

    let d = TempDir::new();
    let mut cfg = marsad_client::ClientConfig::new(
        d.path().join("l"),
        wasit_control::Hello::new("test", "gui", "gui"),
    );
    cfg.agent_bin = Some(agent());
    cfg.serve_args = serve_args();
    cfg.supervise.backoff.initial = Duration::from_millis(200);
    cfg.daemon_env = vec![
        ("MARSAD_AGENT_BIN_DIR".into(), bin_dir),
        ("MARSAD_BUILD_ID".into(), "e2e-ssh".into()),
    ];
    let c = MarsadClient::connect(cfg);
    let mut fleet = c.fleet(None);
    next(&mut fleet).await;
    let host = HostId::new("lin");
    c.add_host(HostDraft {
        name: "linux over ssh".into(),
        kind: HostKind::Ssh {
            target: "root@127.0.0.1".into(),
            port: Some(2222),
            identity_file: Some(key),
            options: vec![
                "StrictHostKeyChecking=accept-new".into(),
                "BatchMode=yes".into(),
            ],
        },
        id: Some(host.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "the ssh host connected", |e| {
        link_is(e, "lin", LinkState::is_connected)
    })
    .await;
    let entry = c
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.id == host)
        .unwrap();
    let platform = entry.platform.as_ref().unwrap();
    assert_eq!(platform.os, "linux", "{platform:?}");

    gate_round_trip(&c, &host).await;

    if let Ok(i) = c.daemon_info(None).await {
        stop_pid(i.pid);
    }
    let _ = sshd.kill();
}

/// Host environment settings on the WSL host: setup commands put a `genie` wrapper (standing in
/// for claude) on PATH; Test finds it there, answering like Claude Code.
async fn wsl_host_environment_settings(c: &MarsadClient, host: &HostId, distro: &str) {
    let st = std::process::Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--exec", "sh", "-c"])
        .arg(
            "mkdir -p /opt/site/genie && printf '#!/bin/sh\\nexec /usr/local/bin/claude \"$@\"\\n' \
             > /opt/site/genie/genie && chmod 755 /opt/site/genie/genie && cat /opt/site/genie/genie",
        )
        .status()
        .unwrap();
    assert!(st.success());
    let saved = c
        .set_host_env(marsad_proto::hosts::HostEnvSettings {
            host: host.clone(),
            setup: "export PATH=/opt/site/genie:$PATH".into(),
            vars: vec![marsad_proto::hosts::HostVar {
                name: "SITE".into(),
                value: Some("wsl".into()),
                secret: false,
            }],
            commands: [(marsad_proto::DriverId::new("claude"), "genie".to_string())].into(),
            revision: String::new(),
        })
        .await
        .unwrap();
    eprintln!("WSL settings applied: {saved:?}");
    assert!(saved.environment.unwrap().setup.unwrap().ok);
    let t = c.test_host_env(host).await.unwrap();
    let cl = t
        .agents
        .iter()
        .find(|a| a.driver.as_str() == "claude")
        .unwrap();
    eprintln!("claude via genie: {cl:?}");
    assert_eq!(
        cl.binary_path.as_deref(),
        Some("/opt/site/genie/genie"),
        "{cl:?}"
    );
    assert!(cl.version.is_some() && cl.warning.is_none(), "{cl:?}");
}

// ── Native Windows over SSH ─────────────────────────────────────────────────

/// What a terminal on `host` prints for `cmd` (cmd.exe, the default shell), until `want`.
async fn terminal_prints(c: &MarsadClient, host: &HostId, cwd: &Path, cmd: &str, want: &str) {
    use base64::Engine;
    let info = c
        .pty_start(
            Some(host),
            PtyStartRequest {
                target: GitTarget::cwd(cwd.to_string_lossy()),
                cols: 120,
                rows: 30,
                shell: None,
            },
        )
        .await
        .unwrap();
    let mut out = c.pty_output(Some(host), &info.id, None);
    c.pty_input(Some(host), &info.id, &format!("{cmd}\r\n"))
        .await
        .unwrap();
    let mut text = String::new();
    while !text.contains(want) {
        match tokio::time::timeout(T, out.next()).await {
            Ok(Some(Ok((_, marsad_proto::pty::PtyEvent::Output { data, .. })))) => {
                text.push_str(&String::from_utf8_lossy(
                    &base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                ))
            }
            other => panic!(
                "terminal: {:?}; printed {text:?}",
                other.map(|o| o.map(|r| r.is_ok()))
            ),
        }
    }
    c.pty_close(Some(host), &info.id).await.unwrap();
}

/// This runner as a native Windows host over Marsad's real ssh link (OpenSSH Server on
/// localhost, key auth; CI's windows-e2e job sets it up). The first attempt runs the POSIX
/// bootstrap and finds Windows (cmd.exe has no `sh`, or Git's `sh` reports MINGW), the
/// PowerShell bootstrap uploads this build's helper into `%LOCALAPPDATA%\Marsad\bin`, and the
/// host daemon serves: discovery; per-host setup commands in PowerShell with non-ASCII output
/// and `$env:` values; variables (one secret); an agent command (the fake CLI, which only acts
/// as claude because a host variable says so) found by Test; a claude session that runs it and
/// sees the setup's non-ASCII value; a terminal (cmd.exe in ConPTY) with the variables.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs OpenSSH Server on this machine prepared by CI (MARSAD_E2E_WINSSH_KEY, _PORT)"]
async fn native_windows_host_over_ssh_end_to_end() {
    let key = std::env::var("MARSAD_E2E_WINSSH_KEY").expect("MARSAD_E2E_WINSSH_KEY");
    let port: u16 = std::env::var("MARSAD_E2E_WINSSH_PORT")
        .expect("MARSAD_E2E_WINSSH_PORT")
        .parse()
        .unwrap();
    let user = std::env::var("USERNAME").expect("USERNAME");
    let d = TempDir::new();
    let c = client(&d.path().join("l"));
    let mut fleet = c.fleet(None);
    next(&mut fleet).await;
    let host = HostId::new("winssh");
    c.add_host(HostDraft {
        name: "this PC over ssh".into(),
        kind: HostKind::Ssh {
            target: format!("{user}@127.0.0.1"),
            port: Some(port),
            identity_file: Some(key),
            options: vec![
                "StrictHostKeyChecking=accept-new".into(),
                "BatchMode=yes".into(),
            ],
        },
        id: Some(host.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "the Windows ssh host connected", |e| {
        link_is(e, "winssh", LinkState::is_connected)
    })
    .await;
    let entry = c
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.id == host)
        .unwrap();
    let platform = entry.platform.clone().unwrap();
    assert_eq!(platform.os, "windows", "{platform:?}");
    assert_eq!(platform.target, "x86_64-pc-windows-msvc", "{platform:?}");

    // The helper was provisioned into the user's %LOCALAPPDATA% and serves this host.
    let info = c.daemon_info(Some(&host)).await.unwrap();
    eprintln!("Windows host daemon: {info:?}");
    let installed = Path::new(&std::env::var("LOCALAPPDATA").unwrap())
        .join("Marsad")
        .join("bin")
        .join("marsad-agent.exe");
    assert!(installed.is_file(), "{}", installed.display());
    assert!(
        info.exe
            .to_ascii_lowercase()
            .ends_with("\\marsad\\bin\\marsad-agent.exe"),
        "{info:?}"
    );
    assert_eq!(info.host_id, "winssh");

    // Discovery: every driver reported; claude is not installed on the runner.
    let rt = c.runtime(Some(host.clone()));
    let inv = rt.inventory(None).await.unwrap();
    for drv in DRIVERS {
        assert!(
            inv.drivers.iter().any(|x| x.driver.as_str() == drv),
            "{drv}: {inv:?}"
        );
    }

    // Setup commands (PowerShell, non-ASCII), variables, an agent command.
    let transcript = d.path().join("transcript grüße ✓.ndjson");
    let secret = "hush-win-secret-71";
    let saved = c
        .set_host_env(marsad_proto::hosts::HostEnvSettings {
            host: host.clone(),
            setup: format!(
                "Write-Output 'héllo ✓ 日本語'\n$env:FROM_SETUP = 'grüße'\n\
                 $env:WASIT_FAKE_TRANSCRIPT = '{}'\n$env:Path = \"C:\\marsad-e2e\\tools;$env:Path\"",
                transcript.display()
            ),
            vars: vec![
                marsad_proto::hosts::HostVar {
                    name: "WASIT_FAKE_PERSONA".into(),
                    value: Some("claude".into()),
                    secret: false,
                },
                marsad_proto::hosts::HostVar {
                    name: "PLAIN".into(),
                    value: Some("plain-value".into()),
                    secret: false,
                },
                marsad_proto::hosts::HostVar {
                    name: "API_TOKEN".into(),
                    value: Some(secret.into()),
                    secret: true,
                },
            ],
            commands: [(
                marsad_proto::DriverId::new("claude"),
                fake().display().to_string(),
            )]
            .into(),
            revision: String::new(),
        })
        .await
        .unwrap();
    eprintln!("Windows settings applied: {saved:?}");
    assert!(saved.not_applied.is_none(), "{saved:?}");
    let env = saved.environment.clone().unwrap();
    let s = env.setup.clone().unwrap();
    assert!(s.ok && s.applied, "{s:?}");
    assert_eq!(s.shell, "powershell -NoProfile");
    assert!(s.output.contains("héllo ✓ 日本語"), "{s:?}");
    assert_eq!(
        env.path.first().map(String::as_str),
        Some("C:\\marsad-e2e\\tools"),
        "{env:?}"
    );
    assert!(!serde_json::to_string(&saved).unwrap().contains(secret));

    // Test: claude is the fake CLI (answering like claude only through the host variable).
    let t = c.test_host_env(&host).await.unwrap();
    let cl = t
        .agents
        .iter()
        .find(|a| a.driver.as_str() == "claude")
        .unwrap();
    eprintln!("claude on the Windows host: {cl:?}");
    assert!(
        cl.installed && cl.version.is_some() && cl.warning.is_none(),
        "{cl:?}"
    );
    assert_eq!(cl.command.as_deref(), Some(&*fake().display().to_string()));

    // A claude session runs it, in the setup's environment (the transcript path is a non-ASCII
    // value the setup left).
    let sc = Scenario::builder()
        .turn(Turn::builder().say("hello from windows"))
        .build();
    let mut req = LaunchRequest::new("claude");
    req.host = Some(host.clone());
    req.cwd = Some(d.path().to_string_lossy().into_owned());
    req.first_prompt = Some("hi".into());
    req.overrides = OverrideLayer {
        env: vec![("WASIT_FAKE_SCENARIO_JSON".into(), sc.to_json())],
        ..OverrideLayer::default()
    };
    let id = c.launch(req).await.unwrap().id;
    let mut sub = rt.subscribe_session(&id, None);
    let snap = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let s = rt.snapshot(&id).await.unwrap();
            if s.turns_completed >= 1 {
                return s;
            }
            session_event(&mut sub).await;
        }
    })
    .await
    .expect("the session on the Windows host finished a turn");
    assert_eq!(
        snap.command.as_deref(),
        Some(&*fake().display().to_string())
    );
    let lines = std::fs::read_to_string(&transcript)
        .unwrap_or_else(|e| panic!("transcript at {}: {e}", transcript.display()));
    assert!(lines.contains("hello from windows"), "{lines}");
    rt.delete(&id).await.unwrap();

    // A terminal (cmd.exe in ConPTY) has the variables and the setup's values.
    terminal_prints(&c, &host, d.path(), "echo [%PLAIN%]", "[plain-value]").await;
    terminal_prints(&c, &host, d.path(), "echo [%FROM_SETUP%]", "[grüße]").await;

    stop_pid(info.pid);
    if let Ok(i) = c.daemon_info(None).await {
        stop_pid(i.pid);
    }
}
