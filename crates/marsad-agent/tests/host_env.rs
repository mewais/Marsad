//! Per-host environment settings over a REAL ssh link (and on the local host): setup commands
//! that use a fake module system put `kilo` and a `genie` wrapper on PATH; `genie` stands in
//! for claude in discovery and in a launched session; `lister` (not an agent) is warned about;
//! variables (one secret) reach sessions and terminals; the secret never comes back to the UI
//! and is in no store document but `secrets.json` (0600), no log, no journal; a failing setup is
//! reported plainly and the host stays connected; a restarted host daemon gets the settings
//! again when its link reconnects.
#![cfg(target_os = "linux")]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::git::GitTarget;
use marsad_proto::hosts::{HostDraft, HostEnvSettings, HostVar, SecretsLocation};
use marsad_proto::pty::{PtyEvent, PtyStartRequest};
use marsad_proto::runtime::{DriverInventory, EnvironmentTest};
use marsad_proto::{DriverId, HostId};
use wasit_fake_cli::scenario::{Scenario, Turn};

const SECRET: &str = "hush-4c1d-secret-value";

fn exe(p: &Path, text: &str) {
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `<home>/modules/init.sh` defines `module`; module `tools` has `kilo` (the fake CLI, persona
/// by name), `genie` (a claude wrapper that leaves its environment in `<home>/genie-env`) and
/// `lister` (not an agent).
fn module_system(home: &Path) {
    let m = home.join("modules");
    let bin = m.join("tools/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        m.join("init.sh"),
        format!(
            "module() {{\n  if [ \"$1\" = load ] && [ -d '{m}'/\"$2\" ]; then\n    \
             PATH='{m}'/\"$2\"/bin:$PATH; export PATH; echo \"loaded $2\"\n  else\n    \
             echo \"module: $2 not found\" >&2; return 1\n  fi\n}}\n",
            m = m.display()
        ),
    )
    .unwrap();
    std::os::unix::fs::symlink(fake(), bin.join("kilo")).unwrap();
    exe(
        &bin.join("genie"),
        &format!(
            "#!/bin/sh\nenv > '{}/genie-env'\nWASIT_FAKE_PERSONA=claude exec '{}' \"$@\"\n",
            home.display(),
            fake().display()
        ),
    );
    exe(
        &bin.join("lister"),
        "#!/bin/sh\necho 'lister (GNU tools) 9.4'\n",
    );
}

fn hub_client(dir: &Path) -> MarsadClient {
    let mut cfg = client_config(dir, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.serve_args.extend(serve_args());
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    MarsadClient::connect(cfg)
}

async fn until(f: &mut Subscription<FleetEvent>, what: &str, pred: impl Fn(&FleetEvent) -> bool) {
    let fut = async {
        loop {
            let (_, e) = next(f).await;
            if pred(&e) {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn connected(e: &FleetEvent, host: &str) -> bool {
    host_connected(e, host)
}

fn agent_of<'a>(t: &'a EnvironmentTest, d: &str) -> &'a DriverInventory {
    t.agents.iter().find(|a| a.driver.as_str() == d).unwrap()
}

fn var(name: &str, value: Option<&str>, secret: bool) -> HostVar {
    HostVar {
        name: name.into(),
        value: value.map(str::to_string),
        secret,
    }
}

/// What a terminal on `host` prints for `cmd`.
async fn terminal(
    c: &MarsadClient,
    host: Option<&HostId>,
    cwd: &Path,
    cmd: &str,
    want: &str,
) -> String {
    let info = c
        .pty_start(
            host,
            PtyStartRequest {
                target: GitTarget::cwd(cwd.to_string_lossy()),
                cols: 120,
                rows: 24,
                shell: Some("/bin/sh".into()),
            },
        )
        .await
        .unwrap()
        .pty;
    let mut out = c.pty_output(host, &info.id, None);
    c.pty_input(host, &info.id, &format!("{cmd}\n"))
        .await
        .unwrap();
    let mut text = String::new();
    while !text.contains(want) {
        match tokio::time::timeout(Duration::from_secs(30), out.next()).await {
            Ok(Some(Ok((_, PtyEvent::Output { data, .. })))) => {
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
    c.pty_close(host, &info.id).await.unwrap();
    text
}

/// Every regular file under `dir` (skipping the helper cache) that contains `needle`.
fn files_containing(dir: &Path, needle: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && p.file_name().is_some_and(|n| n != "bin") {
            files_containing(&p, needle, out);
        } else if ft.is_file() {
            if let Ok(b) = std::fs::read(&p) {
                if b.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    out.push(p.display().to_string());
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_environment_settings_over_ssh_and_local() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let home = sshd.remote_home.clone();
    module_system(&home);
    let hub_dir = d.path().join("l");
    let c = hub_client(&hub_dir);
    let mut fleet = c.fleet(None);
    let box_id = HostId::new("box");
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(box_id.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "box connected", |e| connected(e, "box")).await;

    // Nothing configured: kilo is not on the host's PATH.
    let t = c.test_host_env(&box_id).await.unwrap();
    assert!(t.complete);
    assert!(
        !agent_of(&t, "kilo").installed,
        "{:?}",
        agent_of(&t, "kilo")
    );
    assert!(t.environment.setup.is_none());

    // Save: setup (module system), variables (one secret), agent commands.
    let settings = HostEnvSettings {
        host: box_id.clone(),
        setup: format!(
            ". '{}/modules/init.sh'\nmodule load tools\nexport FROM_SETUP=yes",
            home.display()
        ),
        vars: vec![
            var("PLAIN", Some("plain-value"), false),
            var("API_TOKEN", Some(SECRET), true),
        ],
        commands: BTreeMap::from([
            (DriverId::new("claude"), "genie".to_string()),
            (DriverId::new("codex"), "lister".to_string()),
        ]),
        revision: String::new(),
    };
    let saved = c.set_host_env(settings).await.unwrap();
    assert!(saved.not_applied.is_none(), "{saved:?}");
    assert_eq!(saved.settings.vars[1], var("API_TOKEN", None, true));
    let env = saved.environment.clone().expect("applied at once");
    assert_eq!(
        env.revision.as_deref(),
        Some(saved.settings.revision.as_str())
    );
    let setup = env.setup.clone().unwrap();
    assert!(setup.ok && setup.applied, "{setup:?}");
    assert!(setup.output.contains("loaded tools"), "{setup:?}");
    assert_eq!(env.variables, ["PLAIN", "API_TOKEN"]);
    let tools = home.join("modules/tools/bin").display().to_string();
    assert_eq!(env.path.first(), Some(&tools));
    let got = c.host_env(&box_id).await.unwrap();
    assert_eq!(got.vars[1].value, None, "a secret came back to the UI");

    // Test: kilo found on the setup's PATH; claude is `genie`; `lister` is not codex.
    let t = c.test_host_env(&box_id).await.unwrap();
    assert!(t.complete);
    assert!(t.environment.setup.as_ref().unwrap().ok);
    let k = agent_of(&t, "kilo");
    assert!(k.installed && k.version.is_some(), "{k:?}");
    assert_eq!(k.binary_path.as_deref(), Some(&*format!("{tools}/kilo")));
    let cl = agent_of(&t, "claude");
    assert_eq!(cl.command.as_deref(), Some("genie"));
    assert_eq!(cl.binary_path.as_deref(), Some(&*format!("{tools}/genie")));
    assert!(cl.version.is_some() && cl.warning.is_none(), "{cl:?}");
    let cx = agent_of(&t, "codex");
    let w = cx.warning.as_deref().unwrap_or_default();
    assert!(w.contains("`lister` does not answer like"), "{cx:?}");

    // A claude session on the host runs `genie`, with the variables.
    let work = home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let sc = Scenario::builder()
        .turn(Turn::builder().say("hello from genie"))
        .build();
    let mut req = LaunchRequest::new("claude");
    req.host = Some(box_id.clone());
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some("hi".into());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &sc),
        ..OverrideLayer::default()
    };
    let launched = c.launch(req).await.unwrap();
    let rt = c.runtime(Some(box_id.clone()));
    let id = launched.id;
    let mut sub = rt.subscribe_session(&id, None);
    let snap = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let s = rt.snapshot(&id).await.unwrap();
            if s.turns_completed >= 1 {
                return s;
            }
            // Wake on the session's next event.
            sub.next()
                .await
                .expect("session events")
                .expect("session event");
        }
    })
    .await
    .expect("the genie session finished a turn");
    assert_eq!(snap.turns_completed, 1);
    assert_eq!(
        snap.command.as_deref(),
        Some("genie"),
        "the session names its command"
    );
    let genie_env = std::fs::read_to_string(home.join("genie-env")).expect("genie ran");
    assert!(genie_env.contains("PLAIN=plain-value"), "{genie_env}");
    assert!(genie_env.contains("FROM_SETUP=yes"));
    assert!(genie_env.contains(&format!("API_TOKEN={SECRET}")));

    // A new terminal on the host has them too.
    terminal(
        &c,
        Some(&box_id),
        &work,
        "echo \"v=$PLAIN:$FROM_SETUP\"",
        "v=plain-value:yes",
    )
    .await;

    // The local host takes its own settings the same way.
    let local = HostId::new("hub");
    let ls = c
        .set_host_env(HostEnvSettings {
            host: local.clone(),
            setup: "export LOCAL_SETUP=done".into(),
            vars: vec![var("LOCAL_VAR", Some("on-hub"), false)],
            commands: BTreeMap::new(),
            revision: String::new(),
        })
        .await
        .unwrap();
    assert!(ls.environment.unwrap().setup.unwrap().ok);
    terminal(
        &c,
        None,
        d.path(),
        "echo \"l=$LOCAL_VAR:$LOCAL_SETUP\"",
        "l=on-hub:done",
    )
    .await;

    // The secret: in the process environment only. Not in the hub's documents (only in its
    // owner-only secrets.json when there is no system credential store, as the hub says), not
    // in any log, journal or session file on either side.
    let mut found = Vec::new();
    files_containing(&hub_dir, SECRET, &mut found);
    files_containing(&home.join(".marsad"), SECRET, &mut found);
    match c.secrets_location().await.unwrap() {
        SecretsLocation::File { path, reason } => {
            assert!(!reason.is_empty());
            assert_eq!(
                found,
                std::slice::from_ref(&path),
                "files holding the secret"
            );
            assert!(path.ends_with("store/secrets.json"), "{path}");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        SecretsLocation::System { .. } => {
            assert!(found.is_empty(), "files holding the secret: {found:?}")
        }
    }
    for v in [
        serde_json::to_string(&saved).unwrap(),
        serde_json::to_string(&t).unwrap(),
        serde_json::to_string(&got).unwrap(),
    ] {
        assert!(!v.contains(SECRET), "a reply holds the secret");
    }

    // A failing setup: reported plainly, the host stays connected, genie is gone.
    let mut failing = c.host_env(&box_id).await.unwrap();
    failing.setup = format!(". '{}/modules/init.sh'\nmodule load nope", home.display());
    let saved = c.set_host_env(failing).await.unwrap();
    let s = saved.environment.unwrap().setup.unwrap();
    assert!(!s.ok, "{s:?}");
    assert_eq!(s.exit_code, Some(1));
    assert!(s.output.contains("module: nope not found"), "{s:?}");
    let t = c.test_host_env(&box_id).await.unwrap();
    let cl = agent_of(&t, "claude");
    assert!(!cl.installed, "{cl:?}");
    assert!(
        cl.error
            .as_deref()
            .unwrap_or_default()
            .contains("`genie` was not found on PATH"),
        "{cl:?}"
    );
    let hosts = c.hosts().await.unwrap();
    assert!(hosts.iter().any(|h| h.id == box_id));

    // A restarted host daemon has no settings; the hub applies them when the link is back.
    let wanted = c.host_env(&box_id).await.unwrap().revision;
    let pid = c.daemon_info(Some(&box_id)).await.unwrap().pid;
    stop_daemon(pid).await;
    until(&mut fleet, "box reconnecting", |e| {
        link_is(e, "box", |s| !s.is_connected())
    })
    .await;
    until(&mut fleet, "box connected again", |e| connected(e, "box")).await;
    let env = tokio::time::timeout(Duration::from_secs(60), async {
        let mut delay = Duration::from_millis(20);
        loop {
            if let Ok(Some(e)) = c.runtime(Some(box_id.clone())).host_environment().await {
                if e.revision.as_deref() == Some(wanted.as_str()) {
                    return e;
                }
            }
            // Backoff while the hub applies the settings after the reconnect.
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(1));
        }
    })
    .await
    .expect("settings applied again after the daemon restarted");
    assert_eq!(env.setup.unwrap().exit_code, Some(1));
}

/// A terminal or session asked for while a save is still being applied starts in the saved
/// environment: it waits for the apply. The setup holds the apply open on a FIFO until the
/// terminal has been asked for; before the fix the terminal started at once with the
/// previous environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_asked_for_during_a_save_gets_the_saved_environment() {
    let d = TempDir::new();
    let hub_dir = d.path().join("l");
    let c = hub_client(&hub_dir);
    let local = HostId::new("hub");
    let started = d.path().join("started.fifo");
    let release = d.path().join("release.fifo");
    for f in [&started, &release] {
        let st = std::process::Command::new("mkfifo")
            .arg(f)
            .status()
            .unwrap();
        assert!(st.success());
    }
    c.set_host_env(HostEnvSettings {
        host: local.clone(),
        setup: "export ORDERED=old".into(),
        vars: Vec::new(),
        commands: BTreeMap::new(),
        revision: String::new(),
    })
    .await
    .unwrap();

    let save = {
        let c = c.clone();
        let local = local.clone();
        let setup = format!(
            "echo go > '{}'\nread x < '{}'\nexport ORDERED=new",
            started.display(),
            release.display()
        );
        tokio::spawn(async move {
            c.set_host_env(HostEnvSettings {
                host: local,
                setup,
                vars: Vec::new(),
                commands: BTreeMap::new(),
                revision: String::new(),
            })
            .await
        })
    };
    // The setup is running (blocking open of the FIFO until it writes).
    let first = tokio::time::timeout(Duration::from_secs(60), tokio::fs::read_to_string(&started))
        .await
        .expect("the setup started")
        .unwrap();
    assert_eq!(first.trim(), "go");
    let releaser = {
        let release = release.clone();
        tokio::spawn(async move {
            // Gives the terminal request time to reach the hub while the apply is pending; a
            // late request only makes the test weaker, never fail wrongly.
            tokio::time::sleep(Duration::from_millis(500)).await;
            tokio::fs::write(&release, "done\n").await.unwrap();
        })
    };
    let text = terminal(&c, None, d.path(), "echo \"o=$ORDERED.\"", "o=new.").await;
    assert!(!text.contains("o=old."), "{text}");
    releaser.await.unwrap();
    let saved = save.await.unwrap().unwrap();
    assert!(saved.environment.unwrap().setup.unwrap().ok);
}
