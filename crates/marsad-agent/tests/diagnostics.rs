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

    // The files that must be free of them.
    let mut scanned = Vec::new();
    files(&l.diag(), &mut scanned);
    let diag_files = scanned.len();
    for name in ["marsad-agent.log"] {
        if let Ok(b) = std::fs::read(l.data.join(name)) {
            scanned.push((l.data.join(name), b));
        }
    }
    for e in std::fs::read_dir(l.data.join("sessions"))
        .unwrap()
        .flatten()
    {
        for f in std::fs::read_dir(e.path()).unwrap().flatten() {
            let n = f.file_name().to_string_lossy().into_owned();
            if n.starts_with("shim.log") || n == "shim.stderr" {
                scanned.push((f.path(), std::fs::read(f.path()).unwrap()));
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

    // Not vacuous: the diagnostics hold the expected events, the shim wrote its own log, and
    // the daemon's old stderr file was cut.
    assert!(diag_files >= 2, "{scanned:?}");
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
