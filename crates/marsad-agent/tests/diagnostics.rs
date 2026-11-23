//! The daemon's, the bridge's and `stop`'s diagnostics through real processes (design S4):
//! what files they write and what is in them, that the files are private, that stderr stays
//! silent unless a developer asks, and (the first leak test) that no content of a real
//! session reaches any diagnostic file, the daemon's stderr file or a shim's log.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::*;
use marsad_proto::agentconfig::LaunchRequest;
use marsad_proto::layout::Layout;
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, Command};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use wasit_fake_cli::scenario::{Scenario, Turn};

fn layout(data: &Path) -> Layout {
    Layout::resolve(data.to_path_buf(), None).unwrap()
}

/// Every line of an NDJSON diagnostics file, parsed (a line that is not JSON fails the test).
fn records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("{}: not JSON ({e}): {l:?}", path.display()))
        })
        .collect()
}

fn of<'a>(recs: &'a [Value], ev: &str) -> Vec<&'a Value> {
    recs.iter().filter(|r| r["ev"] == ev).collect()
}

fn one<'a>(recs: &'a [Value], ev: &str) -> &'a Value {
    let found = of(recs, ev);
    assert_eq!(found.len(), 1, "expected one `{ev}` in {recs:#?}");
    found[0]
}

/// The file starts with its header line.
fn has_header(recs: &[Value]) {
    assert_eq!(recs[0]["diag_file"], 1, "{:?}", recs[0]);
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Starts `serve --hub` on `data`, returns the child once it printed its readiness line.
async fn start_daemon(data: &Path, envs: &[(&str, &str)], stderr: Stdio) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--hub")
        .arg("--data")
        .arg(data)
        .args(serve_args())
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        T,
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .expect("readiness line")
    .unwrap();
    let ready: marsad_proto::daemon::Readiness = serde_json::from_str(line.trim()).unwrap();
    assert!(ready.ready);
    child
}

async fn stop(data: &Path) -> std::process::Output {
    let out = tokio::time::timeout(
        T,
        tokio::process::Command::new(agent())
            .arg("stop")
            .arg("--data")
            .arg(data)
            .output(),
    )
    .await
    .expect("stop ended")
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[tokio::test]
async fn daemon_stop_and_a_losing_daemon_leave_their_records() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let mut child = start_daemon(&data, &[], Stdio::piped()).await;

    // A second daemon on the same data loses the lock race.
    let second = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--data")
        .arg(&data)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap();
    assert_eq!(second.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&second.stderr).contains("another daemon is running"));

    stop(&data).await;
    let status = tokio::time::timeout(T, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    // Without `MARSAD_LOG` nothing is written to stderr: no `tracing` text, no colour codes.
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .await
        .unwrap();
    assert!(
        stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );

    // The daemon's own file: header, then its life.
    let daemon = records(&l.diag().join("daemon.ndjson"));
    has_header(&daemon);
    let start = one(&daemon, "daemon.start");
    assert_eq!(start["src"], "hub");
    assert_eq!(start["hub"], true);
    assert_eq!(start["host_id"], "hub");
    assert_eq!(start["free_text_logging"], false);
    assert_eq!(start["machine"], l.machine.as_str());
    assert!(start["data"].as_str().unwrap().ends_with(&l.machine));
    let boot = start["b"].as_str().unwrap();
    let ready = one(&daemon, "daemon.ready");
    for phase in ["total_ms", "env_ms", "adopt_ms", "node_ms", "hub_ms"] {
        assert!(ready[phase].is_u64(), "{phase} in {ready}");
    }
    assert_eq!(ready["line_written"], true);
    assert!(ready["total_ms"].as_u64() >= ready["env_ms"].as_u64());
    let shutdown = one(&daemon, "daemon.shutdown");
    assert_eq!(shutdown["why"], "request");
    one(&daemon, "process.start");
    // One process wrote it, in order, and nothing of the loser is in it.
    assert!(daemon.iter().skip(1).all(|r| r["b"] == boot));
    assert!(of(&daemon, "daemon.lock_held").is_empty());
    let seq = |ev: &str| one(&daemon, ev)["seq"].as_u64().unwrap();
    assert!(seq("daemon.start") < seq("daemon.ready"));
    assert!(seq("daemon.ready") < seq("daemon.shutdown"));
    // The `tracing` bridge writes counts, never text.
    for r in of(&daemon, "trace.counted") {
        assert!(r["n"].is_u64() && r["target"].is_string() && r.get("message").is_none());
    }

    // The shared file: the loser's `daemon.lock_held` and `stop`'s record.
    let aux = records(&l.diag().join("aux.ndjson"));
    has_header(&aux);
    let held = one(&aux, "daemon.lock_held");
    assert_eq!(held["src"], "daemon");
    assert_ne!(held["b"], boot);
    assert!(held["lock"].as_str().unwrap().ends_with("marsad.sock.lock"));
    let stopped = one(&aux, "stop.end");
    assert_eq!(stopped["src"], "stop");
    assert_eq!(stopped["outcome"], "stopped");
    assert!(of(&aux, "daemon.start").is_empty());

    // Private: the directory and its files.
    #[cfg(unix)]
    {
        assert_eq!(mode(&l.diag()), 0o700);
        for f in ["daemon.ndjson", "aux.ndjson"] {
            assert_eq!(mode(&l.diag().join(f)), 0o600, "{f}");
        }
    }

    // `stop` with nothing running records that too.
    let again = tokio::process::Command::new(agent())
        .arg("stop")
        .arg("--data")
        .arg(&data)
        .output()
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&again.stdout).contains("no daemon running"));
    let aux = records(&l.diag().join("aux.ndjson"));
    let outcomes: Vec<_> = of(&aux, "stop.end")
        .iter()
        .map(|r| r["outcome"].clone())
        .collect();
    assert_eq!(outcomes, [json!("stopped"), json!("not_running")]);
}

/// `MARSAD_LOG=<level>` is the developer override: plain `tracing` text on stderr, never colour
/// codes (the daemon's stderr is a file).
#[tokio::test]
async fn marsad_log_prints_plain_text_to_stderr() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let mut child = start_daemon(&data, &[("MARSAD_LOG", "info")], Stdio::piped()).await;
    stop(&data).await;
    tokio::time::timeout(T, child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .await
        .unwrap();
    let text = String::from_utf8(stderr).unwrap();
    assert!(text.contains("adopted sessions"), "stderr: {text:?}");
    assert!(!text.contains('\u{1b}'), "colour codes in {text:?}");
    let daemon = records(&l.diag().join("daemon.ndjson"));
    assert_eq!(one(&daemon, "daemon.start")["free_text_logging"], true);
    // The text is not also in the diagnostics file.
    assert!(!std::fs::read_to_string(l.diag().join("daemon.ndjson"))
        .unwrap()
        .contains("adopted sessions"));
}

/// Runs `bridge` with `args` on `data` (stdin closed), returns its output.
async fn bridge(data: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    tokio::time::timeout(
        T,
        tokio::process::Command::new(agent())
            .arg("bridge")
            .arg("--data")
            .arg(data)
            .args(args)
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .expect("the bridge ended")
    .unwrap()
}

/// The goodbye frame a bridge that cannot reach its daemon writes for the hub (unchanged wire).
fn assert_bridge_unavailable_goodbye(out: &std::process::Output) {
    let line = String::from_utf8_lossy(&out.stdout);
    let frame: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(frame["reason"]["code"], "bridge_unavailable", "{frame}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("bridge:"));
    assert_eq!(out.status.code(), Some(1));
}

#[tokio::test]
async fn a_bridge_whose_daemon_cannot_start_records_why() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    // An old, large stderr file: the starter cuts it before the new daemon writes to it.
    std::fs::create_dir_all(&l.data).unwrap();
    std::fs::write(l.log(), vec![b'x'; 2 << 20]).unwrap();

    // The daemon the bridge starts refuses its options and exits with 2.
    let out = bridge(&data, &["--driver-bin", "nosuch=/x"], &[]).await;
    assert_bridge_unavailable_goodbye(&out);

    let aux = records(&l.diag().join("aux.ndjson"));
    has_header(&aux);
    let start = one(&aux, "bridge.start");
    assert_eq!(start["src"], "bridge");
    let daemon = one(&aux, "bridge.daemon");
    assert_eq!(daemon["outcome"], "start_failed");
    assert_eq!(daemon["reason"], "exited_before_ready");
    assert_eq!(daemon["exit"], 2);
    assert!(daemon["ms"].is_u64());
    // The failed daemon says why itself.
    let failed = one(&aux, "daemon.failed");
    assert_eq!(failed["src"], "daemon");
    assert_eq!(failed["stage"], "config");
    assert_eq!(failed["exit"], 2);
    assert_eq!(failed["error"]["code"], "config");
    assert!(failed["error"]["text"].as_str().unwrap().contains("nosuch"));
    let end = one(&aux, "bridge.end");
    assert_eq!(end["reason"], "no_daemon");
    assert_eq!(end["error"]["code"], "unavailable");
    let seq = |r: &Value| r["seq"].as_u64().unwrap();
    assert!(seq(start) < seq(daemon) && seq(daemon) < seq(end));
    assert!(of(&aux, "bridge.daemon")
        .iter()
        .all(|r| r["outcome"] != "running"));

    // The daemon's stderr file: cut, private, the raw reason, no colour.
    let log = std::fs::read_to_string(l.log()).unwrap();
    assert!(log.len() < 1 << 20, "{} bytes", log.len());
    assert!(!log.starts_with('x'));
    assert!(log.contains("nosuch"), "{log:?}");
    assert!(!log.contains('\u{1b}'));
    #[cfg(unix)]
    assert_eq!(mode(&l.log()), 0o600);
    // A daemon that never held the lock wrote no `daemon.ndjson`.
    assert!(!l.diag().join("daemon.ndjson").exists());
}

#[tokio::test]
async fn a_bridge_that_cannot_connect_records_unavailable() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    // A daemon of this machine holds the lock but does not serve this endpoint: the bridge's
    // own start attempt ends with "another daemon won", and the connect after it fails.
    let lock = wasit_control::InstanceLock::try_acquire(wasit_control::lock_path_for(&l.socket()))
        .await
        .unwrap();
    let out = bridge(&data, &[], &[]).await;
    drop(lock);
    assert_bridge_unavailable_goodbye(&out);

    let aux = records(&l.diag().join("aux.ndjson"));
    let mine: Vec<_> = aux
        .iter()
        .filter(|r| r["src"] == "bridge" && r.get("ev").is_some())
        .collect();
    let evs: Vec<_> = mine.iter().map(|r| r["ev"].as_str().unwrap()).collect();
    assert_eq!(
        evs,
        [
            "process.start",
            "bridge.start",
            "bridge.daemon",
            "bridge.daemon",
            "bridge.end"
        ]
    );
    assert_eq!(mine[2]["outcome"], "running", "{}", mine[2]);
    assert_eq!(mine[3]["outcome"], "unavailable");
    assert!(mine[3]["ms"].is_u64());
    assert_eq!(mine[3]["error"]["code"], "unavailable");
    assert!(mine[3]["error"]["text"]
        .as_str()
        .unwrap()
        .contains("bridge cannot reach the daemon"));
    assert_eq!(mine[4]["reason"], "no_daemon");
    // The daemon the bridge tried to start found the lock held.
    let held = one(&aux, "daemon.lock_held");
    assert_ne!(held["b"], mine[0]["b"]);
}

/// A daemon whose starter closed the readiness pipe stays up: the readiness write used to be a
/// `println!`, which panics on a closed pipe, killing a daemon that had started completely.
#[tokio::test]
async fn a_closed_readiness_pipe_does_not_end_the_daemon() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    // The pipe's reading end is closed before the daemon exists: its write must fail.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--hub")
        .arg("--data")
        .arg(&data)
        .args(serve_args())
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // The starter finds the daemon by connecting (the client's supervised link retries on its
    // backoff schedule until the daemon accepts).
    let mut cfg = marsad_client::ClientConfig::new(
        &data,
        wasit_control::Hello::new("test", "gui-test", "gui"),
    );
    cfg.start_daemon = false;
    cfg.agent_bin = Some(agent());
    cfg.supervise.backoff.initial = std::time::Duration::from_millis(50);
    cfg.supervise.backoff.max = std::time::Duration::from_millis(500);
    let c = marsad_client::MarsadClient::connect(cfg);
    let info = c.daemon_info(None).await.unwrap();
    assert_eq!(info.pid, child.id().unwrap());
    assert!(
        child.try_wait().unwrap().is_none(),
        "the daemon is serving, not ended"
    );
    drop(c);

    stop(&data).await;
    let status = tokio::time::timeout(T, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "{status:?}");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .await
        .unwrap();
    assert!(
        stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );

    let daemon = records(&l.diag().join("daemon.ndjson"));
    let unwritten = one(&daemon, "daemon.readiness_unwritten");
    assert_eq!(unwritten["lvl"], "warn");
    assert_eq!(unwritten["why"], "pipe_closed");
    assert_eq!(unwritten["error"]["code"], "broken_pipe");
    let ready = one(&daemon, "daemon.ready");
    assert_eq!(ready["line_written"], false);
    assert_eq!(one(&daemon, "daemon.shutdown")["why"], "request");
}

const PANIC_MARKER: &str = "PANIC-CONTENT-MARKER-4e7a";

/// Every file under `dir`, with its bytes.
fn files(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if let Ok(b) = std::fs::read(&p) {
            out.push((p, b));
        }
    }
}

/// A panic in the daemon leaves `diag.panic` (location, kind and length of the payload) and
/// nothing of its message: not in the diagnostics files, not in `marsad-agent.log`. The daemon
/// is started the way a remote one is, by `bridge`, so its stderr is the real log file.
#[tokio::test]
async fn a_daemon_panic_is_recorded_without_its_message() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let out = bridge(&data, &["--panic-test", PANIC_MARKER], &[]).await;
    assert_bridge_unavailable_goodbye(&out);

    let daemon = records(&l.diag().join("daemon.ndjson"));
    let panics = of(&daemon, "diag.panic");
    assert_eq!(panics.len(), 1, "{daemon:#?}");
    let p = panics[0];
    assert_eq!(p["lvl"], "error");
    assert_eq!(p["payload_kind"], "string");
    assert_eq!(
        p["len"],
        format!("daemon panic test: {PANIC_MARKER}").chars().count() as u64
    );
    assert!(p.get("static_text").is_none(), "{p}");
    assert!(p["location"].as_str().unwrap().contains("serve.rs:"));
    assert_eq!(one(&daemon, "daemon.start")["b"], p["b"]);
    // The bridge saw the daemon die with the panic's status.
    let aux = records(&l.diag().join("aux.ndjson"));
    let started = one(&aux, "bridge.daemon");
    assert_eq!(started["outcome"], "start_failed");
    assert_eq!(started["exit"], 101);

    // The daemon's stderr file is empty: the default hook printed nothing.
    assert_eq!(std::fs::metadata(l.log()).unwrap().len(), 0);
    let mut all = Vec::new();
    files(&data, &mut all);
    assert!(all.iter().any(|(p, _)| p.ends_with("daemon.ndjson")));
    for (path, bytes) in all {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(PANIC_MARKER),
            "marker in {}",
            path.display()
        );
    }
}

/// The developer override chains the default panic message to stderr (the log file, never
/// exported) as plain text; the diagnostics files are the same.
#[tokio::test]
async fn the_developer_override_prints_a_panic_message_to_the_log_only() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let out = bridge(
        &data,
        &["--panic-test", PANIC_MARKER],
        &[("MARSAD_LOG", "info")],
    )
    .await;
    assert_bridge_unavailable_goodbye(&out);
    let log = std::fs::read_to_string(l.log()).unwrap();
    assert!(log.contains(PANIC_MARKER), "{log:?}");
    assert!(!log.contains('\u{1b}'), "colour codes in {log:?}");
    let diag = std::fs::read_to_string(l.diag().join("daemon.ndjson")).unwrap();
    assert!(diag.contains("\"ev\":\"diag.panic\""));
    assert!(!diag.contains(PANIC_MARKER));
}

// ── the first leak test ──────────────────────────────────────────────────────

/// A marker is raw, JSON-escaped once and JSON-escaped twice (a CLI's JSON line travels inside
/// the journal's JSON).
fn forms(marker: &str) -> Vec<String> {
    let esc = |s: &str| {
        let q = serde_json::to_string(s).unwrap();
        q[1..q.len() - 1].to_string()
    };
    let once = esc(marker);
    let twice = esc(&once);
    vec![marker.to_string(), once, twice]
}

fn contains_any(haystack: &str, marker: &str) -> bool {
    forms(marker).iter().any(|f| haystack.contains(f.as_str()))
}

async fn next_event(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(T, sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        other => panic!(
            "no session event: {:?}",
            other.map(|o| o.map(|r| r.map(|(c, _)| c)))
        ),
    }
}

fn is_turn_completed(e: &SessionEvent) -> bool {
    matches!(
        &e.kind,
        SessionEventKind::Agent {
            event: AgentEvent::TurnCompleted { .. }
        }
    )
}

/// A local session driven with a unique marker through every content channel — the prompt, the
/// assistant's text and thinking, a tool call's input and result, the CLI's stderr and a CLI
/// failure message, and the project directory's name — and afterwards every file the
/// diagnostics could have reached is scanned: all of `<data>/diag/`, `marsad-agent.log` and
/// every `shim.log*` / `shim.stderr`. The same markers must be in the journal (the one place
/// they belong), so the channels were really driven, and the diagnostics must be non-trivial.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_session_content_reaches_any_diagnostic_file() {
    const PROMPT1: &str = "LEAK\"prompt-one\"7f3a1c";
    const PROMPT2: &str = "LEAK\"prompt-two\"2b9d5e";
    const SAY: &str = "LEAK\"assistant-text\"c41e08";
    const THINK: &str = "LEAK\"thinking\"90ab17";
    const TOOL_INPUT: &str = "LEAK\"tool-input\"5d62f3";
    const TOOL_RESULT: &str = "LEAK\"tool-result\"e8071b";
    const STDERR: &str = "LEAK\"cli-stderr\"a3f640";
    const FAIL: &str = "LEAK\"cli-failure\"6c1d92";
    const PROJECT: &str = "PROJECTDIRLEAK-b57e23";
    let markers = [
        PROMPT1,
        PROMPT2,
        SAY,
        THINK,
        TOOL_INPUT,
        TOOL_RESULT,
        STDERR,
        FAIL,
        PROJECT,
    ];

    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let work = d.path().join(PROJECT);
    std::fs::create_dir_all(&work).unwrap();
    // The starter cuts an old, large stderr file (and the marker survives nowhere in it).
    std::fs::create_dir_all(&l.data).unwrap();
    std::fs::write(l.log(), vec![b'x'; 2 << 20]).unwrap();

    let scenario = Scenario::builder()
        .turn(
            Turn::builder()
                .think(THINK)
                .tool("Bash", json!({ "command": format!("echo {TOOL_INPUT}") }))
                .result_if_allowed(TOOL_RESULT)
                .done()
                .stderr(STDERR)
                .say(SAY),
        )
        .turn(Turn::builder().fail(FAIL))
        .build();

    let c = client(&data); // starts the daemon on demand, the way the GUI does
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides.env = scenario_env(d.path(), &scenario);
    let launched = c.launch(req).await.unwrap();
    let id = launched.id.clone();
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        let e = next_event(&mut sub).await;
        if matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == "idle") {
            break;
        }
    }
    for prompt in [PROMPT1, PROMPT2] {
        rt.command(&id, Command::prompt(prompt)).await.unwrap();
        loop {
            let e = next_event(&mut sub).await;
            if is_turn_completed(&e) {
                break;
            }
        }
    }
    // End the session (the shim writes its last records) and the daemon, then scan.
    rt.stop(&id).await.unwrap();
    loop {
        let e = next_event(&mut sub).await;
        if matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == "exited") {
            break;
        }
    }
    drop(sub);
    drop(rt);
    drop(c);
    stop(&data).await;

    // The channels were driven: every marker is in the journal.
    let session_dir = l.data.join("sessions").join(id.to_string());
    let journal = std::fs::read_to_string(session_dir.join("journal.ndjson")).unwrap();
    for m in [
        PROMPT1,
        PROMPT2,
        SAY,
        THINK,
        TOOL_INPUT,
        TOOL_RESULT,
        STDERR,
        FAIL,
    ] {
        assert!(contains_any(&journal, m), "{m} is not in the journal");
    }

    // The files that must be free of them (and the diagnostics must be non-trivial: the daemon's
    // and the shared file at least).
    assert_no_markers(&l, &markers, 2);

    // Not vacuous: the diagnostics hold the expected events, the shim wrote its own log, and
    // the daemon's old stderr file was cut.
    let daemon = records(&l.diag().join("daemon.ndjson"));
    has_header(&daemon);
    for ev in [
        "process.start",
        "daemon.start",
        "daemon.ready",
        "daemon.shutdown",
    ] {
        one(&daemon, ev);
    }
    assert!(daemon.len() > 8, "{} records", daemon.len());
    let shim = records(&session_dir.join("shim.log"));
    has_header(&shim);
    assert!(!of(&shim, "shim.start").is_empty(), "{shim:#?}");
    assert!(std::fs::metadata(l.log()).unwrap().len() < 1 << 20);
    #[cfg(unix)]
    {
        assert_eq!(mode(&l.diag()), 0o700);
        assert_eq!(mode(&l.log()), 0o600);
    }
}

// ── sign-in prompts ──────────────────────────────────────────────────────────

mod signin {
    use std::time::Duration;

    use marsad_client::{ClientConfig, MarsadClient, Subscription};
    use marsad_proto::fleet::FleetEvent;
    use marsad_proto::hosts::{AuthPrompt, PromptKind};
    use marsad_proto::HostId;

    use super::*;

    pub const HOST: &str = "hub";

    /// A hub daemon (started on demand by its client) with `extra` serve options.
    pub fn hub(data: &Path, extra: &[&str]) -> MarsadClient {
        let mut cfg = ClientConfig::new(data, wasit_control::Hello::new("test", "gui-test", "gui"));
        cfg.agent_bin = Some(agent());
        cfg.serve_args = serve_args();
        cfg.serve_args.extend(extra.iter().map(|s| s.to_string()));
        cfg.supervise.backoff.initial = Duration::from_millis(50);
        cfg.supervise.backoff.max = Duration::from_millis(500);
        MarsadClient::connect(cfg)
    }

    /// The real `SSH_ASKPASS` helper process, started the way a hub link's ssh starts it, asking
    /// `prompt` for the host `HOST` (the hub's own entry: no ssh server is needed to be asked).
    pub fn askpass(data: &Path, prompt: &str) -> tokio::process::Child {
        let l = layout(data);
        tokio::process::Command::new(agent())
            .arg(prompt)
            .env("MARSAD_ASKPASS_HOST", HOST)
            .env("MARSAD_ASKPASS_SOCKET", l.socket())
            .env("MARSAD_ASKPASS_DIAG", l.diag())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    /// The next fleet event matching `pred` (bounded).
    pub async fn until(
        f: &mut Subscription<FleetEvent>,
        what: &str,
        pred: impl Fn(&FleetEvent) -> bool,
    ) -> FleetEvent {
        let fut = async {
            loop {
                let (_, e) = next(f).await;
                if pred(&e) {
                    return e;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(120), fut)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    pub async fn prompt(f: &mut Subscription<FleetEvent>) -> AuthPrompt {
        match until(
            f,
            "a sign-in prompt",
            |e| matches!(e, FleetEvent::AuthPrompt { host, prompt: Some(_) } if host.as_str() == HOST),
        )
        .await
        {
            FleetEvent::AuthPrompt {
                prompt: Some(p), ..
            } => p,
            _ => unreachable!(),
        }
    }

    pub async fn withdrawn(f: &mut Subscription<FleetEvent>) {
        until(
            f,
            "the prompt to be withdrawn",
            |e| matches!(e, FleetEvent::AuthPrompt { host, prompt: None } if host.as_str() == HOST),
        )
        .await;
    }

    pub async fn finished(child: tokio::process::Child) -> std::process::Output {
        tokio::time::timeout(T, child.wait_with_output())
            .await
            .expect("the askpass helper ended")
            .unwrap()
    }

    /// The `auth.answered` / `auth.cancelled` records of `prompt`.
    pub fn ended(recs: &[Value], prompt: u64) -> Vec<&Value> {
        recs.iter()
            .filter(|r| {
                r["prompt"] == prompt
                    && r["ev"]
                        .as_str()
                        .is_some_and(|e| e == "auth.answered" || e == "auth.cancelled")
            })
            .collect()
    }

    pub fn host_id() -> HostId {
        HostId::new(HOST)
    }

    pub fn kind_word(k: PromptKind) -> &'static str {
        match k {
            PromptKind::Secret => "secret",
            PromptKind::Code => "code",
            PromptKind::Confirm => "confirm",
            PromptKind::Info => "info",
        }
    }
}

/// Every way a sign-in prompt can end, through the real askpass helper and the real
/// `hosts.askpass` / `hosts.auth_answer` / `hosts.auth_cancel` methods: answered, replaced by a
/// newer prompt, abandoned because the helper went away, cancelled by the user. Each prompt is
/// recorded when it is raised and once when it ends; neither the prompt's text nor the answer
/// is in any file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_sign_in_prompt_is_recorded_and_ends_once_without_text_or_answer() {
    use marsad_proto::hosts::PromptKind;
    use signin::*;
    const ANSWER: &str = "ANSWER\"leak\"3f9a61c0";
    const TEXT_A: &str = "PROMPTTEXT-a1 password:";
    const TEXT_B: &str = "PROMPTTEXT-b2 verification code:";
    const TEXT_C: &str = "PROMPTTEXT-c3 Are you sure you want to continue connecting (yes/no)?";
    const TEXT_D: &str = "PROMPTTEXT-d4 hello";
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let c = hub(&data, &[]);
    // The hub serves (a helper started earlier would find nobody to ask).
    c.daemon_info(None).await.unwrap();
    let mut fleet = c.fleet(None);

    // 1. Answered: the helper prints the answer for ssh.
    let helper = askpass(&data, TEXT_A);
    let p1 = prompt(&mut fleet).await;
    assert_eq!(
        (p1.kind, p1.attempt, p1.prompt.as_str()),
        (PromptKind::Secret, 1, TEXT_A)
    );
    c.answer_auth(&host_id(), p1.id, ANSWER).await.unwrap();
    let out = finished(helper).await;
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), ANSWER);

    // 2. A newer prompt of the same host replaces an older one that nobody answered.
    let first = askpass(&data, TEXT_B);
    let p2 = prompt(&mut fleet).await;
    let second = askpass(&data, TEXT_C);
    let p3 = loop {
        let p = prompt(&mut fleet).await;
        if p.id != p2.id {
            break p;
        }
    };
    assert_eq!(p3.kind, PromptKind::Confirm);
    let replaced = finished(first).await;
    assert_eq!(
        replaced.status.code(),
        Some(1),
        "the replaced prompt is refused"
    );

    // 3. The helper that asked the current prompt goes away (ssh gave up on it, or was
    // killed): the prompt nobody waits for is withdrawn.
    let mut second = second;
    second.kill().await.unwrap();
    withdrawn(&mut fleet).await;

    // 4. The user cancels.
    let helper = askpass(&data, TEXT_D);
    let p4 = prompt(&mut fleet).await;
    assert_eq!(p4.kind, PromptKind::Info);
    c.cancel_auth(&host_id(), p4.id).await.unwrap();
    let cancelled = finished(helper).await;
    assert_eq!(cancelled.status.code(), Some(1));

    drop(fleet);
    drop(c);
    stop(&data).await;

    let hub = records(&l.diag().join("daemon.ndjson"));
    let raised = of(&hub, "auth.prompt");
    let ids: Vec<u64> = raised
        .iter()
        .map(|r| r["prompt"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [p1.id, p2.id, p3.id, p4.id], "{raised:#?}");
    for (r, p) in raised.iter().zip([&p1, &p2, &p3, &p4]) {
        assert_eq!(r["src"], "hub");
        assert_eq!(r["host"], HOST);
        assert_eq!(r["attempt"], 1);
        assert_eq!(r["kind"], kind_word(p.kind));
    }
    // Each ended exactly once, in the way it did.
    let end = |id: u64| {
        let e = ended(&hub, id);
        assert_eq!(e.len(), 1, "prompt {id}: {e:#?}");
        e[0]
    };
    let answered = end(p1.id);
    assert_eq!(answered["ev"], "auth.answered");
    assert!(answered["ms"].is_u64());
    for (id, why) in [
        (p2.id, "superseded"),
        (p3.id, "helper_gone"),
        (p4.id, "user"),
    ] {
        let e = end(id);
        assert_eq!(
            (e["ev"].as_str(), e["why"].as_str()),
            (Some("auth.cancelled"), Some(why))
        );
    }

    // The helper's own records (aux) and everything else: no prompt text, no answer.
    let aux = records(&l.diag().join("aux.ndjson"));
    assert!(of(&aux, "askpass.end")
        .iter()
        .any(|r| r["outcome"] == "answered" && r["host"] == HOST));
    assert_no_markers(&l, &[ANSWER, TEXT_A, TEXT_B, TEXT_C, TEXT_D], 2);
}

/// A prompt nobody answers ends by the timeout, on record, and the helper is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sign_in_prompt_nobody_answers_times_out_on_record() {
    use signin::*;
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let c = hub(&data, &["--auth-prompt-timeout-ms", "150"]);
    c.daemon_info(None).await.unwrap();
    let mut fleet = c.fleet(None);
    let helper = askpass(&data, "PROMPTTEXT-t password:");
    let p = prompt(&mut fleet).await;
    withdrawn(&mut fleet).await;
    assert_eq!(finished(helper).await.status.code(), Some(1));
    drop(fleet);
    drop(c);
    stop(&data).await;
    let hub = records(&l.diag().join("daemon.ndjson"));
    let e = ended(&hub, p.id);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(
        (e[0]["ev"].as_str(), e[0]["why"].as_str()),
        (Some("auth.cancelled"), Some("timeout"))
    );
}

/// The files that must never hold a marker: everything under `<data>/diag/`, the daemon's
/// stderr file and every shim log, each marker raw, JSON-escaped once and twice. `at_least`
/// diagnostics files must exist (so the scan cannot pass on nothing).
fn assert_no_markers(l: &Layout, markers: &[&str], at_least: usize) {
    let mut scanned = Vec::new();
    files(&l.diag(), &mut scanned);
    assert!(scanned.len() >= at_least, "{scanned:?}");
    if let Ok(b) = std::fs::read(l.data.join("marsad-agent.log")) {
        scanned.push((l.data.join("marsad-agent.log"), b));
    }
    if let Ok(rd) = std::fs::read_dir(l.data.join("sessions")) {
        for e in rd.flatten() {
            for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if n.starts_with("shim.log") || n == "shim.stderr" {
                    scanned.push((f.path(), std::fs::read(f.path()).unwrap()));
                }
            }
        }
    }
    for (path, bytes) in &scanned {
        let text = String::from_utf8_lossy(bytes);
        for m in markers {
            assert!(
                !contains_any(&text, m),
                "marker {m} found in {}:\n{text}",
                path.display()
            );
        }
    }
}

// ── a startup failure's stderr ───────────────────────────────────────────────

/// A daemon that cannot start prints on stderr (the daemon's `marsad-agent.log`) what its
/// diagnostic record holds: the stage, a code and a length for an error that may quote stored
/// data (the Wasl node's journal path), not the error text. `MARSAD_LOG` brings the text back.
#[tokio::test]
async fn a_startup_failure_prints_what_the_record_holds_not_the_error_text() {
    let d = TempDir::new();
    let data = d.path().join("DATADIRLEAK-4be1");
    let l = layout(&data);
    // The Wasl node cannot read its journal: it is a directory.
    std::fs::create_dir_all(l.data.join("wasl").join("journal.ndjson")).unwrap();
    let run = |envs: &[(&str, &str)]| {
        let mut cmd = tokio::process::Command::new(agent());
        cmd.arg("serve")
            .arg("--data")
            .arg(&data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .envs(envs.iter().copied());
        async move {
            tokio::time::timeout(T, cmd.output())
                .await
                .expect("serve ended")
                .unwrap()
        }
    };

    let out = run(&[]).await;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("startup failed at Node (exit 1): "),
        "stderr: {stderr:?}"
    );
    assert!(stderr.contains("details withheld"), "stderr: {stderr:?}");
    for quoted in ["journal.ndjson", "DATADIRLEAK", "Is a directory", "wasl"] {
        assert!(!stderr.contains(quoted), "{quoted} in stderr: {stderr:?}");
    }
    // The record says the same, content-free (the daemon held its lock: its own file).
    let daemon = records(&l.diag().join("daemon.ndjson"));
    let failed = of(&daemon, "daemon.failed");
    assert_eq!(failed.len(), 1, "{daemon:#?}");
    assert_eq!(failed[0]["stage"], "node");
    assert_eq!(failed[0]["exit"], 1);
    assert!(failed[0]["error"]["len"].is_u64());
    assert!(failed[0]["error"].get("text").is_none());

    // The developer override prints the error in full.
    let out = run(&[("MARSAD_LOG", "info")]).await;
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("cannot open the Wasl node"),
        "stderr: {stderr:?}"
    );
    assert!(stderr.contains("journal.ndjson"), "stderr: {stderr:?}");

    // An error of ours keeps its words: the OS's reason and the path of Marsad's own files.
    let blocked = d.path().join("file");
    std::fs::write(&blocked, b"x").unwrap();
    let out = tokio::time::timeout(
        T,
        tokio::process::Command::new(agent())
            .arg("serve")
            .arg("--data")
            .arg(blocked.join("sub"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("startup failed at DataDir"), "{stderr:?}");
    assert!(stderr.contains("cannot create "), "{stderr:?}");
}

// ── the connection trail (real sshd on localhost) ────────────────────────────

#[cfg(target_os = "linux")]
mod connect {
    use std::time::Duration;

    use marsad_client::{ClientConfig, MarsadClient};
    use marsad_proto::fleet::FleetEvent;
    use marsad_proto::hosts::{HostDraft, HostEnvSettings, HostKind, HostVar};
    use marsad_proto::{CommandFailureKind, HostId, LinkState};

    use super::signin::until;
    use super::*;

    /// A hub whose daemon is started by its client with `env` (and `extra` serve options).
    fn hub(data: &Path, env: &[(&str, String)], extra: &[&str]) -> MarsadClient {
        let mut cfg = ClientConfig::new(data, wasit_control::Hello::new("test", "gui", "gui"));
        cfg.agent_bin = Some(agent());
        cfg.serve_args = serve_args();
        cfg.serve_args.extend(extra.iter().map(|s| s.to_string()));
        cfg.daemon_env = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        cfg.supervise.backoff.initial = Duration::from_millis(50);
        MarsadClient::connect(cfg)
    }

    fn link_is(e: &FleetEvent, host: &str, p: impl Fn(&LinkState) -> bool) -> bool {
        matches!(e, FleetEvent::Link { host: h, state } if h.as_str() == host && p(state))
    }

    fn draft(name: &str, id: &str, kind: HostKind) -> HostDraft {
        HostDraft {
            name: name.into(),
            kind,
            id: Some(HostId::new(id)),
        }
    }

    /// Ends the hub (its client first, so nothing restarts it) and returns its records.
    async fn hub_records(c: MarsadClient, data: &Path) -> Vec<Value> {
        drop(c);
        stop(data).await;
        records(&layout(data).diag().join("daemon.ndjson"))
    }

    /// The connect-attempt and boot records of attempt `n` of `host`, in file order: the
    /// supervisor's `link.attempt_begin` / `link.attempt_end` and the connector's `boot.*`,
    /// joined on `(host, n)`.
    fn attempt<'a>(recs: &'a [Value], host: &str, n: u64) -> Vec<&'a Value> {
        let link = format!("hosts:{host}");
        recs.iter()
            .filter(|r| r["src"] == "hub" && r["n"] == n)
            .filter(|r| r["host"] == host || r["link"] == link.as_str())
            .filter(|r| {
                let ev = r["ev"].as_str().unwrap_or("");
                ev == "link.attempt_begin" || ev == "link.attempt_end" || ev.starts_with("boot.")
            })
            .collect()
    }

    fn evs<'a>(attempt: &[&'a Value]) -> Vec<&'a str> {
        attempt.iter().map(|r| r["ev"].as_str().unwrap()).collect()
    }

    /// The numbers of the attempts of `host` that ended with `outcome`.
    fn ended_with(recs: &[Value], host: &str, outcome: &str) -> Vec<u64> {
        let link = format!("hosts:{host}");
        of(recs, "link.attempt_end")
            .into_iter()
            .filter(|r| r["link"] == link.as_str() && r["outcome"] == outcome)
            .map(|r| r["n"].as_u64().unwrap())
            .collect()
    }

    /// The one record of `ev` of `attempt`.
    fn step<'a>(attempt: &[&'a Value], ev: &str) -> &'a Value {
        let found: Vec<&&Value> = attempt.iter().filter(|r| r["ev"] == ev).collect();
        assert_eq!(found.len(), 1, "expected one `{ev}` in {attempt:#?}");
        found[0]
    }

    fn our_build() -> String {
        let out = std::process::Command::new(agent())
            .arg("--version")
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["build"].as_str().unwrap().to_owned()
    }

    fn our_target() -> String {
        format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
    }

    /// The directory of the remote's installed helper.
    fn remote_bin_dir(sshd: &Sshd) -> PathBuf {
        sshd.installed_agent().parent().unwrap().to_path_buf()
    }

    fn not_root() {
        let uid = std::process::Command::new("id").arg("-u").output().unwrap();
        assert_ne!(
            String::from_utf8_lossy(&uid.stdout).trim(),
            "0",
            "this test needs a directory the (non-root) test user cannot write; root can"
        );
    }

    /// An sshd whose every session first runs `before` (what a `.profile` or an `rc` file that
    /// prints does): sshd's `ForceCommand` runs this script, which then runs the command the
    /// hub asked for. `before` may end the session itself.
    fn sshd_running_first(dir: &Path, before: &str) -> Sshd {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("before.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\n{before}\nexec /bin/sh -c \"$SSH_ORIGINAL_COMMAND\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Sshd::start_custom(
            dir,
            "ssh",
            &format!("ForceCommand {}", script.display()),
            true,
        )
    }

    async fn parked(
        fleet: &mut marsad_client::Subscription<FleetEvent>,
        host: &str,
    ) -> (String, Option<CommandFailureKind>) {
        match until(fleet, "a parked link", |e| {
            link_is(e, host, |s| matches!(s, LinkState::Failed { .. }))
        })
        .await
        {
            FleetEvent::Link {
                state: LinkState::Failed { reason, problem },
                ..
            } => (reason, problem),
            _ => unreachable!(),
        }
    }

    /// A good connect, step by step: the connector's records come between the supervisor's
    /// begin and end of the same attempt, in the order of the protocol; the registry and the
    /// session feed record what they did for the host.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_good_connect_records_every_step_in_order() {
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        let kind = sshd.kind(&sshd.key);
        c.add_host(draft("Box", "box", kind)).await.unwrap();
        until(&mut fleet, "box connected", |e| {
            link_is(e, "box", LinkState::is_connected)
        })
        .await;
        // The connect taught the registry the host's machine (the platform came with the
        // report): wait for it on the fleet, then apply settings explicitly.
        until(&mut fleet, "the machine learned", |e| {
            matches!(e, FleetEvent::HostUpserted { host } if host.id.as_str() == "box" && host.machine.is_some())
        })
        .await;
        let saved = c
            .set_host_env(HostEnvSettings {
                host: HostId::new("box"),
                setup: String::new(),
                vars: vec![HostVar {
                    name: "BOX_VAR".into(),
                    value: Some("one".into()),
                    secret: false,
                }],
                commands: Default::default(),
                revision: String::new(),
            })
            .await
            .unwrap();
        assert!(saved.not_applied.is_none(), "{saved:?}");
        drop(fleet);
        let hub = hub_records(c, &data).await;

        let connected = ended_with(&hub, "box", "connected");
        assert_eq!(connected.len(), 1, "{connected:?}");
        let n = connected[0];
        let trail = attempt(&hub, "box", n);
        assert_eq!(
            evs(&trail),
            [
                "link.attempt_begin",
                "boot.attempt",
                "boot.preamble",
                "boot.report",
                "boot.action",
                "boot.uploaded",
                "boot.ok",
                "boot.hello",
                "link.attempt_end",
            ]
        );
        let at = step(&trail, "boot.attempt");
        assert_eq!(
            (
                &at["shell"],
                &at["first_contact"],
                &at["interactive"],
                &at["armed"]
            ),
            (&json!("posix"), &json!(true), &json!(false), &json!(false))
        );
        let pre = step(&trail, "boot.preamble");
        assert_eq!((&pre["lines"], &pre["bytes"]), (&json!(0), &json!(0)));
        assert!(pre.get("shape").is_none());
        let report = step(&trail, "boot.report");
        assert_eq!(report["os"], "Linux");
        assert_eq!(report["arch"], std::env::consts::ARCH);
        assert!(
            report.get("installed").is_none(),
            "nothing is installed yet"
        );
        // Nothing was installed: the hub's own build goes up, whole.
        let size = std::fs::metadata(agent()).unwrap().len();
        let action = step(&trail, "boot.action");
        assert_eq!(action["action"], "upload");
        assert_eq!(action["bytes"], size);
        assert_eq!(action["target"], our_target());
        let uploaded = step(&trail, "boot.uploaded");
        assert_eq!(uploaded["bytes"], size);
        assert!(uploaded["ms"].is_u64());
        let hello = step(&trail, "boot.hello");
        assert_eq!(hello["build"], our_build());
        assert_eq!(hello["host_id_matches"], true);
        assert!(hello["machine"].is_string());
        // The timeline grows along the attempt.
        let ms = |r: &Value| r["ms"].as_u64().unwrap();
        assert!(ms(report) <= ms(step(&trail, "boot.ok")));
        assert!(ms(step(&trail, "boot.ok")) <= ms(hello));
        let end = step(&trail, "link.attempt_end");
        assert_eq!(end["outcome"], "connected");
        // The transport's own records carry the same attempt number.
        let spawned: Vec<_> = of(&hub, "transport.spawned")
            .into_iter()
            .filter(|r| r["link"] == "hosts:box" && r["n"] == n)
            .collect();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0]["program"], "ssh");

        // The registry: the host was added (its target only by shape), the connect taught it the
        // platform and the machine, its session feed started, and its environment was applied.
        let added = of(&hub, "hosts.changed")
            .into_iter()
            .find(|r| r["host"] == "box")
            .unwrap();
        assert_eq!(added["op"], "added");
        assert_eq!(added["kind"], "ssh");
        assert_eq!(added["target_kind"], "user_at_host");
        assert_eq!(
            (
                &added["port_set"],
                &added["identity_set"],
                &added["options"]
            ),
            (&json!(true), &json!(true), &json!(3))
        );
        let learned: Vec<_> = of(&hub, "hosts.learned")
            .into_iter()
            .filter(|r| r["host"] == "box")
            .map(|r| r["what"].as_str().unwrap().to_owned())
            .collect();
        assert!(learned.contains(&"platform".into()), "{learned:?}");
        assert!(learned.contains(&"machine".into()), "{learned:?}");
        assert!(of(&hub, "fleet.feed")
            .iter()
            .any(|r| r["host"] == "box" && r["event"] == "started"));
        let apply = of(&hub, "hostenv.apply")
            .into_iter()
            .rfind(|r| r["host"] == "box")
            .unwrap();
        assert_eq!(apply["outcome"], "applied");
        assert_eq!(apply["revision"], saved.settings.revision.as_str());
        assert!(apply["ms"].is_u64());
        assert!(
            !std::fs::read_to_string(layout(&data).diag().join("daemon.ndjson"))
                .unwrap()
                .contains(&sshd.user)
        );
    }

    /// A wrong key: the attempt ends in the report stage with ssh's words, classified as a
    /// sign-in problem, and the link parks in `auth_needed`. There is no prompt (key sign-in).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wrong_key_is_recorded_as_an_auth_failure_without_a_prompt() {
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        c.add_host(draft("Bad", "bad", sshd.kind(&sshd.wrong_key)))
            .await
            .unwrap();
        let state = until(&mut fleet, "auth needed", |e| {
            link_is(e, "bad", |s| matches!(s, LinkState::AuthNeeded { .. }))
        })
        .await;
        let FleetEvent::Link {
            state: LinkState::AuthNeeded { problem, detail },
            ..
        } = state
        else {
            unreachable!()
        };
        assert_eq!(problem, CommandFailureKind::AuthNeeded);
        assert!(detail.contains("Permission denied"), "{detail}");
        drop(fleet);
        let hub = hub_records(c, &data).await;

        let parked = ended_with(&hub, "bad", "park");
        assert_eq!(parked, [1]);
        let trail = attempt(&hub, "bad", 1);
        assert_eq!(
            evs(&trail),
            [
                "link.attempt_begin",
                "boot.attempt",
                "boot.preamble",
                "boot.failed",
                "link.attempt_end"
            ]
        );
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["lvl"], "warn");
        assert_eq!(failed["stage"], "report");
        assert_eq!(failed["kind"], "auth_needed");
        assert_eq!(failed["exit"], 255);
        assert!(failed["stderr"]
            .as_str()
            .unwrap()
            .contains("Permission denied"));
        // The link's own state record says the same.
        let to_auth = of(&hub, "link.state")
            .into_iter()
            .find(|r| r["link"] == "hosts:bad" && r["to"] == "auth_needed")
            .unwrap();
        assert!(to_auth["error"]
            .as_str()
            .unwrap()
            .contains("Permission denied"));
        assert!(of(&hub, "auth.prompt").is_empty());
    }

    /// A host that cannot be reached, and a host entry's life: added, renamed, removed; its
    /// ssh target never in a record, only its shape.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_host_and_the_registry_changes_are_recorded() {
        let d = TempDir::new();
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        // A closed port on this machine: ssh says "Connection refused" at once.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let kind = HostKind::Ssh {
            target: "leaktarget-9d2e@127.0.0.1".into(),
            port: Some(closed),
            identity_file: None,
            options: vec![],
        };
        let entry = c.add_host(draft("Gone", "gone", kind)).await.unwrap();
        until(&mut fleet, "gone reconnecting", |e| {
            link_is(e, "gone", |s| matches!(s, LinkState::Reconnecting { .. }))
        })
        .await;
        let mut renamed = entry.clone();
        renamed.name = "Gone away".into();
        c.update_host(renamed).await.unwrap();
        c.remove_host(&entry.id).await.unwrap();
        drop(fleet);
        let hub = hub_records(c, &data).await;

        let changes: Vec<(&str, &str)> = of(&hub, "hosts.changed")
            .into_iter()
            .filter(|r| r["host"] == "gone")
            .map(|r| {
                (
                    r["op"].as_str().unwrap(),
                    r["target_kind"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            changes,
            [
                ("added", "user_at_host"),
                ("updated", "user_at_host"),
                ("removed", "user_at_host")
            ]
        );
        for r in of(&hub, "hosts.changed") {
            assert!(!r.to_string().contains("leaktarget"), "{r}");
        }
        let first = ended_with(&hub, "gone", "backoff");
        assert!(!first.is_empty());
        let trail = attempt(&hub, "gone", first[0]);
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "report");
        assert_eq!(failed["kind"], "unreachable");
        assert!(failed["stderr"]
            .as_str()
            .unwrap()
            .contains("Connection refused"));
    }

    /// A fake helper that the bootstrap installs and starts and that then ends with status 2
    /// and a marked line on stderr (as a helper refusing its runtime directory would). The
    /// bootstrap said `MARSAD-OK`, so the failure comes after it: the helper's own exit status
    /// and stderr are the reason, in the records and in the link state the UI shows, and since
    /// retrying cannot help the link parks instead of looping.
    ///
    /// Before this step the hello's error was returned as it was ("connection closed during
    /// handshake"), the child's exit status and stderr were dropped, and the link went into
    /// `reconnecting` for ever with no reason.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_helper_that_exits_after_ok_gives_its_reason_and_parks_the_link() {
        use std::os::unix::fs::PermissionsExt;
        const MARK: &str = "FAKE-HELPER-MARKER-7c1e refusing the runtime directory";
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let triple = marsad_agent::hub::transport::helper_target("linux", std::env::consts::ARCH)
            .expect("a helper triple for this machine");
        // The hub installs this instead of itself (it is not built for the musl triple).
        assert_ne!(triple, our_target());
        let helpers = d.path().join("helpers");
        std::fs::create_dir_all(helpers.join(triple)).unwrap();
        let fake = helpers.join(triple).join("marsad-agent");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  --version) echo '{{\"build\":\"fake\",\"target\":\"fake\"}}';;\n  stop) exit 0;;\n  bridge) echo '{MARK}' >&2; exit 2;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let data = d.path().join("l");
        let c = hub(
            &data,
            &[("MARSAD_AGENT_BIN_DIR", helpers.display().to_string())],
            &[],
        );
        let mut fleet = c.fleet(None);
        c.add_host(draft("Fake", "fake", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        // The state the UI shows: parked, with the helper's words.
        let (reason, problem) = parked(&mut fleet, "fake").await;
        assert_eq!(problem, Some(CommandFailureKind::UnexpectedOutput));
        assert!(reason.contains(MARK), "{reason}");
        assert!(reason.contains("exit code 2"), "{reason}");
        drop(fleet);
        let hub = hub_records(c, &data).await;

        // One attempt, not a loop.
        assert_eq!(of(&hub, "link.attempt_begin").len(), 1, "{hub:#?}");
        let trail = attempt(&hub, "fake", 1);
        assert_eq!(
            evs(&trail),
            [
                "link.attempt_begin",
                "boot.attempt",
                "boot.preamble",
                "boot.report",
                "boot.action",
                "boot.uploaded",
                "boot.ok",
                "boot.failed",
                "link.attempt_end"
            ]
        );
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "hello");
        assert_eq!(failed["kind"], "unexpected_output");
        assert_eq!(failed["exit"], 2);
        assert!(
            failed["stderr"].as_str().unwrap().contains(MARK),
            "{failed}"
        );
        assert_eq!(step(&trail, "link.attempt_end")["outcome"], "park");
        // It was the fake that ran: it is what the bootstrap installed.
        assert!(std::fs::read_to_string(sshd.installed_agent())
            .unwrap()
            .contains(MARK));
    }

    /// The remote cannot store the helper (the directory it installs into is read-only): the
    /// remote's own exit status and words are the reason of the upload stage, not the local
    /// `Broken pipe`, and nothing was installed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_upload_the_host_cannot_store_gives_the_hosts_reason() {
        use std::os::unix::fs::PermissionsExt;
        not_root();
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let dir = remote_bin_dir(&sshd);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        c.add_host(draft("Ro", "ro", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        let (reason, problem) = parked(&mut fleet, "ro").await;
        assert_eq!(problem, Some(CommandFailureKind::PermissionDenied));
        assert!(reason.contains("Permission denied"), "{reason}");
        assert!(!reason.contains("Broken pipe"), "{reason}");
        drop(fleet);
        let hub = hub_records(c, &data).await;
        // (The directory is removable again for the temp dir's cleanup.)
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let trail = attempt(&hub, "ro", 1);
        let evs = evs(&trail);
        assert!(evs.contains(&"boot.action"), "{evs:?}");
        assert!(
            !evs.contains(&"boot.uploaded") && !evs.contains(&"boot.ok"),
            "{evs:?}"
        );
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "upload");
        assert_eq!(failed["kind"], "permission_denied");
        assert_eq!(failed["exit"], 70);
        assert!(
            failed["stderr"]
                .as_str()
                .unwrap()
                .contains("Permission denied"),
            "{failed}"
        );
        assert!(!sshd.installed_agent().exists());
    }

    /// A host whose shell startup files print before the report: counted on every attempt
    /// (lines, bytes, a short look), and the connect works.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn banner_lines_before_the_report_are_counted_and_the_connect_works() {
        let d = TempDir::new();
        let banner = "Welcome to the test box\nlast login: never\n";
        let sshd = sshd_running_first(d.path(), &format!("printf '%s' '{banner}'"));
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        c.add_host(draft("Chatty", "chatty", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        until(&mut fleet, "chatty connected", |e| {
            link_is(e, "chatty", LinkState::is_connected)
        })
        .await;
        drop(fleet);
        let hub = hub_records(c, &data).await;
        let n = ended_with(&hub, "chatty", "connected")[0];
        let trail = attempt(&hub, "chatty", n);
        let pre = step(&trail, "boot.preamble");
        assert_eq!(pre["lines"], 2);
        assert_eq!(pre["bytes"], banner.len());
        let shape = pre["shape"].as_str().unwrap();
        assert!(shape.starts_with("Welcome to the test box"), "{shape}");
        assert!(shape.len() <= 130, "bounded: {shape}");
        // The report came and the attempt went on as usual.
        step(&trail, "boot.report");
        step(&trail, "boot.hello");
    }

    /// The report never comes (the shell startup files print and end the session): the lines
    /// and bytes that did arrive are in the records on every shell tried and in the reason the
    /// user is shown.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn when_no_report_comes_what_arrived_is_the_reason() {
        let d = TempDir::new();
        // Two lines, then a third that never ends; then the session ends.
        let junk = "junk-one\njunk-two\npartial-no-newline";
        let sshd = sshd_running_first(d.path(), &format!("printf '%s' '{junk}'\nexit 0"));
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let mut fleet = c.fleet(None);
        c.add_host(draft("Mute", "mute", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        let (reason, problem) = parked(&mut fleet, "mute").await;
        assert_eq!(problem, Some(CommandFailureKind::UnexpectedOutput));
        let seen = format!("3 lines ({} bytes)", junk.len());
        assert!(reason.contains(&seen), "{reason}");
        assert!(
            reason.contains("none of it the Marsad bootstrap's report"),
            "{reason}"
        );
        drop(fleet);
        let hub = hub_records(c, &data).await;

        let trail = attempt(&hub, "mute", 1);
        // POSIX first; a host that never connected is then tried as native Windows.
        assert_eq!(
            evs(&trail),
            [
                "link.attempt_begin",
                "boot.attempt",
                "boot.preamble",
                "boot.windows_retry",
                "boot.preamble",
                "boot.failed",
                "link.attempt_end"
            ]
        );
        for pre in trail.iter().filter(|r| r["ev"] == "boot.preamble") {
            assert_eq!(
                (&pre["lines"], &pre["bytes"]),
                (&json!(3), &json!(junk.len()))
            );
            assert!(pre["shape"].as_str().unwrap().starts_with("junk-one"));
        }
        assert_eq!(step(&trail, "boot.windows_retry")["why"], "no_report");
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "report");
        assert_eq!(failed["kind"], "unexpected_output");
        assert!(
            failed["stderr"].as_str().unwrap().contains(&seen),
            "{failed}"
        );
    }

    /// An attempt that is cancelled while it waits for the report (the host's shell startup
    /// hangs; here the link is stopped, in the field the connect bound runs out) leaves its
    /// records: where it was, that it did not finish, and what the host had printed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_attempt_stopped_while_the_report_is_awaited_says_so() {
        let d = TempDir::new();
        // The host prints a banner, tells the test it is stuck (a FIFO: the write returns when
        // the test reads), and then waits for input the hub never sends before the report.
        let fifo = d.path().join("stuck");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let sshd = sshd_running_first(
            d.path(),
            &format!(
                "printf 'stuck banner\\n'\necho up > {}\nread unused",
                fifo.display()
            ),
        );
        let data = d.path().join("l");
        let c = hub(&data, &[], &[]);
        let entry = c
            .add_host(draft("Stuck", "stuck", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        let reader = fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::read_to_string(reader))
            .await
            .unwrap()
            .unwrap();
        c.remove_host(&entry.id).await.unwrap();
        let hub = hub_records(c, &data).await;

        let trail = attempt(&hub, "stuck", 1);
        let evs = evs(&trail);
        assert_eq!(evs[..2], ["link.attempt_begin", "boot.attempt"], "{evs:?}");
        assert!(evs.contains(&"boot.preamble"), "{evs:?}");
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "report");
        assert_eq!(failed["error"]["code"], "cancelled");
    }

    /// Nothing a sign-in or a secret touched reaches a log: the answer to a prompt and a host
    /// secret variable are masked in the transport text of records (ssh's words that echo a
    /// path containing them), neither the prompt text nor either value is in any file, and the
    /// positive records exist (so the scan cannot pass on nothing).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_secrets_and_sign_in_answers_never_reach_a_log() {
        const SECRET: &str = "HOSTSECRET-leak-5b81d2";
        const ANSWER: &str = "ANSWER-leak-0a47c9e3";
        const PROMPT: &str = "PROMPTTEXT-leak-77 password:";
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let l = layout(&data);
        let c = hub(&data, &[], &[]);
        c.daemon_info(None).await.unwrap();
        let mut fleet = c.fleet(None);

        // A host secret variable, through the host-environment method (applied to the hub).
        let hub_id = HostId::new("hub");
        let saved = c
            .set_host_env(HostEnvSettings {
                host: hub_id.clone(),
                setup: String::new(),
                vars: vec![HostVar {
                    name: "LEAK_SECRET_VAR".into(),
                    value: Some(SECRET.into()),
                    secret: true,
                }],
                commands: Default::default(),
                revision: String::new(),
            })
            .await
            .unwrap();
        assert!(saved.not_applied.is_none(), "{saved:?}");

        // A sign-in prompt, answered (the real askpass helper, the real methods).
        let helper = super::signin::askpass(&data, PROMPT);
        let p = super::signin::prompt(&mut fleet).await;
        c.answer_auth(&hub_id, p.id, ANSWER).await.unwrap();
        assert!(super::signin::finished(helper).await.status.success());

        // Two hosts whose ssh fails with words that echo a path with the secret / the answer in
        // it: ssh warns about an identity file it cannot read.
        for (id, value) in [("s1", SECRET), ("s2", ANSWER)] {
            let HostKind::Ssh {
                target,
                port,
                options,
                ..
            } = sshd.kind(&sshd.key)
            else {
                unreachable!()
            };
            let kind = HostKind::Ssh {
                target,
                port,
                identity_file: Some(
                    d.path()
                        .join(format!("no-such-{value}"))
                        .display()
                        .to_string(),
                ),
                options,
            };
            c.add_host(draft(id, id, kind)).await.unwrap();
            until(&mut fleet, "auth needed", |e| {
                link_is(e, id, |s| matches!(s, LinkState::AuthNeeded { .. }))
            })
            .await;
        }
        drop(fleet);
        let hub = hub_records(c, &data).await;

        // Positive: the failures are on record with ssh's words, the values masked.
        for host in ["s1", "s2"] {
            let n = ended_with(&hub, host, "park")[0];
            let trail = attempt(&hub, host, n);
            let stderr = step(&trail, "boot.failed")["stderr"].as_str().unwrap();
            assert!(stderr.contains("Identity file"), "{host}: {stderr}");
            assert!(stderr.contains("••••"), "{host}: {stderr}");
            assert!(!stderr.contains("leak"), "{host}: {stderr}");
        }
        assert_eq!(of(&hub, "auth.prompt").len(), 1);
        assert_eq!(of(&hub, "auth.answered").len(), 1);
        let apply = of(&hub, "hostenv.apply")
            .into_iter()
            .rfind(|r| r["host"] == "hub")
            .unwrap();
        assert_eq!(apply["outcome"], "applied");
        assert!(apply["revision"].is_string());
        assert_no_markers(&l, &[SECRET, ANSWER, PROMPT], 2);
    }
}

// ── a session feed that ends ─────────────────────────────────────────────────

/// A host whose daemon refuses the `host.sessions` subscription for good (an old daemon that
/// does not know the topic): the feed ends, and it is on record, with the service's code. (The
/// fleet then no longer follows the host's sessions: recorded here, not retried.)
#[tokio::test]
async fn a_session_feed_that_ends_on_a_refusal_is_recorded() {
    use marsad_agent::hub::fleet::Fleet;
    use wasit_control::{
        connector, ClientOptions, ConnConfig, ControlClient, ControlServer, Hello, NoService,
        Supervised, SupervisedOptions,
    };

    let d = TempDir::new();
    let dir = d.path().join("feed");
    let cfg = wasit_diag::SinkConfig::new(
        wasit_diag::Code::st("hub"),
        wasit_diag::Ident::st("feedtest"),
        &dir,
        "feed.ndjson",
    );
    wasit_diag::install(cfg).unwrap();

    let server = ControlServer::in_process(
        std::sync::Arc::new(NoService),
        Hello::new("b", "old", "daemon"),
        ConnConfig::default(),
    );
    let hello = Hello::new("b", "hub", "hub");
    let conn = connector(move || {
        let server = server.clone();
        let hello = hello.clone();
        async move {
            let (a, b) = tokio::io::duplex(1 << 16);
            drop(server.serve_connection(Box::new(a)));
            ControlClient::connect(Box::new(b), ClientOptions::new(hello)).await
        }
    });
    let link = Supervised::spawn(conn, SupervisedOptions::default());
    let fleet = Fleet::spawn();
    let feed = fleet.feed_remote(marsad_proto::HostId::new("old"), link.clone());
    // The feed task ends by itself.
    tokio::time::timeout(T, feed)
        .await
        .expect("the feed ended")
        .unwrap();
    assert!(wasit_diag::flush(T), "the records are written");
    let recs = records(&dir.join("feed.ndjson"));
    let feed: Vec<(&str, Option<&str>)> = of(&recs, "fleet.feed")
        .into_iter()
        .map(|r| (r["event"].as_str().unwrap(), r["error"]["code"].as_str()))
        .collect();
    assert_eq!(
        feed,
        [("started", None), ("ended", Some("topic_not_found"))]
    );
    for r in of(&recs, "fleet.feed") {
        assert_eq!(r["host"], "old");
    }
    link.stop().await;
}
