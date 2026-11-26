//! The daemon's, the bridge's and `stop`'s diagnostics through real processes (design S4):
//! what files they write and what is in them, that the files are private, that stderr stays
//! silent unless a developer asks, and (the first leak test) that no content of a real
//! session reaches any diagnostic file, the daemon's stderr file or a shim's log.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::*;
use marsad_proto::agentconfig::{LaunchReply, LaunchRequest};
use marsad_proto::layout::Layout;
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{methods, Command};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use wasit_fake_cli::scenario::{Scenario, Turn};

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
        .args(data_args(data))
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

#[tokio::test]
async fn daemon_stop_and_a_losing_daemon_leave_their_records() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let mut child = start_daemon(&data, &[], Stdio::piped()).await;

    // A second daemon on the same data loses the lock race.
    let second = tokio::process::Command::new(agent())
        .arg("serve")
        .args(data_args(&data))
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
        .args(data_args(&data))
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
    assert!(text.contains("MARSAD_LOG is set"), "stderr: {text:?}");
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
            .args(data_args(data))
            .args(args)
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .expect("the bridge ended")
    .unwrap()
}

/// The frames a bridge that cannot reach its daemon writes for the hub: an informational
/// `progress` frame saying it is starting the daemon (every such bridge tried to start one, so
/// it must be there), then the `goodbye` frame with `bridge_unavailable` (unchanged wire).
fn assert_bridge_unavailable_goodbye(out: &std::process::Output) {
    let frames: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let started = frames
        .iter()
        .any(|f| f["type"] == "progress" && f["step"] == wasit_control::STEP_STARTING_DAEMON);
    assert!(started, "no progress frame before the goodbye: {frames:?}");
    let goodbye = frames
        .iter()
        .find(|f| f["type"] != "progress")
        .unwrap_or_else(|| panic!("no goodbye frame: {frames:?}"));
    assert_eq!(goodbye["type"], "goodbye", "{goodbye}");
    assert_eq!(goodbye["reason"]["code"], "bridge_unavailable", "{goodbye}");
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
        .args(data_args(&data))
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
    cfg.socket = Some(layout(&data).socket());
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

async fn next_event(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(T, sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        other => panic!(
            "no session event: {:?}",
            other.map(|o| o.map(|r| r.map(|(c, _)| c)))
        ),
    }
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

// ── traces ───────────────────────────────────────────────────────────────────

/// A `sessions.launch` sent with a trace id through the client: the hub records the launch
/// under that trace (`launch`) and the request it served (`rpc.traced`); the session's first
/// prompt is an input carrying the trace; and the prompt's marker is in no diagnostic file.
#[tokio::test]
async fn a_traced_launch_is_recorded_under_its_trace_without_its_prompt() {
    const TRACE: &str = "u-0123456789abcdef";
    const PROMPT: &str = "TRACED-PROMPT-MARKER-6d20a9";
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let work = d.path().join("traced-project");
    std::fs::create_dir_all(&work).unwrap();

    let c = client(&data); // starts the daemon on demand, the way the GUI does
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some(PROMPT.into());
    let trace = wasit_control::TraceId::new(TRACE).unwrap();
    let launched: LaunchReply = c
        .call_traced(None, methods::SESSIONS_LAUNCH, req, Some(trace))
        .await
        .unwrap();
    let id = launched.id.clone();

    // The first prompt is an input of the session, carrying the launch's trace.
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        let e = next_event(&mut sub).await;
        if matches!(&e.kind, SessionEventKind::Input { trace: Some(t), .. } if t.as_str() == TRACE)
        {
            let text = serde_json::to_string(&e.kind).unwrap();
            assert!(
                text.contains(PROMPT),
                "the input is the first prompt: {text}"
            );
            break;
        }
    }
    drop(sub);
    drop(rt);
    drop(c);
    stop(&data).await;

    let daemon = records(&l.diag().join("daemon.ndjson"));
    // The launch, once, under its trace, with what it did.
    let launches: Vec<_> = of(&daemon, "launch")
        .into_iter()
        .filter(|r| r["trace"] == TRACE)
        .collect();
    assert_eq!(launches.len(), 1, "{daemon:#?}");
    let lr = launches[0];
    assert_eq!(lr["src"], "hub");
    assert_eq!(lr["outcome"], "ok");
    assert_eq!(lr["session"], id.as_str());
    assert_eq!(lr["host"], "hub");
    assert_eq!(lr["driver"], "claude");
    assert_eq!(lr["resume"], "fresh");
    assert_eq!(lr["first_prompt"], true);
    assert!(lr["cwd"]["hash"].is_string(), "{lr}");
    assert!(!lr.to_string().contains("traced-project"), "{lr}");
    assert!(lr["ms"].is_u64());
    assert!(lr.get("error").is_none(), "{lr}");
    // The request the hub served under the trace.
    assert!(
        of(&daemon, "rpc.traced")
            .iter()
            .any(|r| r["trace"] == TRACE && r["method"] == "sessions.launch"),
        "{daemon:#?}"
    );

    // The marker of the prompt is in no diagnostic file, nor in any other file under `diag/`.
    let mut all = Vec::new();
    files(&l.diag(), &mut all);
    assert!(all.iter().any(|(p, _)| p.ends_with("daemon.ndjson")));
    for (path, bytes) in &all {
        assert!(
            !contains_any(&String::from_utf8_lossy(bytes), PROMPT),
            "marker in {}",
            path.display()
        );
    }
}

// ── sign-in prompts ──────────────────────────────────────────────────────────

mod signin {
    use std::time::Duration;

    use marsad_client::{MarsadClient, Subscription};
    use marsad_proto::fleet::FleetEvent;
    use marsad_proto::hosts::{AuthPrompt, PromptKind};
    use marsad_proto::HostId;

    use super::*;

    pub const HOST: &str = "hub";

    /// A hub daemon (started on demand by its client) with `extra` serve options.
    pub fn hub(data: &Path, extra: &[&str]) -> MarsadClient {
        let mut cfg = client_config(data, wasit_control::Hello::new("test", "gui-test", "gui"));
        cfg.serve_args.extend(serve_args());
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
            // No connect attempt is running for the hub's own entry.
            .env("MARSAD_ASKPASS_ATTEMPT", "0")
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
        match until(f, "a sign-in prompt", |e| {
            auth_prompt_is(e, HOST, |p| p.is_some())
        })
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
            .args(data_args(&data))
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
            .args(data_args(&blocked.join("sub")))
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

    use marsad_client::MarsadClient;
    use marsad_proto::fleet::FleetEvent;
    use marsad_proto::hosts::{HostDraft, HostEnvSettings, HostKind, HostVar};
    use marsad_proto::{CommandFailureKind, HostId, LinkState};

    use super::signin::until;
    use super::*;

    /// A hub whose daemon is started by its client with `env` (and `extra` serve options).
    fn hub(data: &Path, env: &[(&str, String)], extra: &[&str]) -> MarsadClient {
        let mut cfg = client_config(data, wasit_control::Hello::new("test", "gui", "gui"));
        cfg.serve_args.extend(serve_args());
        cfg.serve_args.extend(extra.iter().map(|s| s.to_string()));
        cfg.daemon_env = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        cfg.supervise.backoff.initial = Duration::from_millis(50);
        MarsadClient::connect(cfg)
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

    /// The shared connection's records (`ssh.*`) of attempt `n` of `host`, in order (the ones the
    /// supervisor's question made before the attempt carry no attempt number).
    fn ssh_steps<'a>(recs: &'a [Value], host: &str, n: u64) -> Vec<&'a Value> {
        recs.iter()
            .filter(|r| r["src"] == "hub" && r["host"] == host && r["n"] == n)
            .filter(|r| r["ev"].as_str().unwrap_or("").starts_with("ssh."))
            .collect()
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
        // Connected, and the registry taught the host's machine (from the report, before the
        // link was up, or after: either order).
        let (up, known) = (std::cell::Cell::new(false), std::cell::Cell::new(false));
        until(&mut fleet, "box connected and its machine learned", |e| {
            up.set(up.get() || link_is(e, "box", LinkState::is_connected));
            known.set(known.get() || host_is(e, "box", |h| h.machine.is_some()));
            up.get() && known.get()
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
                "boot.report",
                "boot.action",
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
            (
                &json!("universal"),
                &json!(true),
                &json!(false),
                &json!(false)
            )
        );
        let pre = step(&trail, "boot.preamble");
        assert_eq!((&pre["lines"], &pre["bytes"]), (&json!(0), &json!(0)));
        assert!(pre.get("shape").is_none());
        // The script's report, then the installed helper's, each answered once.
        let all =
            |ev: &str| -> Vec<&Value> { trail.iter().copied().filter(|r| r["ev"] == ev).collect() };
        let reports = all("boot.report");
        let report = reports[0];
        assert_eq!(report["os"], "Linux");
        assert_eq!(report["arch"], std::env::consts::ARCH);
        assert!(
            report.get("installed").is_none(),
            "nothing is installed yet"
        );
        assert_eq!(reports[1]["installed"], our_build());
        // Nothing was installed: the hub's own build goes up, whole; then the new helper runs.
        let size = std::fs::metadata(agent()).unwrap().len();
        let actions = all("boot.action");
        let action = actions[0];
        assert_eq!(action["action"], "upload");
        assert_eq!(action["bytes"], size);
        assert_eq!(action["target"], our_target());
        assert_eq!(actions[1]["action"], "run");
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
        // Two ssh commands: the sign-in step that opened the host's shared connection, and the
        // link step that rode it (before the shared connection there was one command).
        assert_eq!(spawned.len(), 2, "{spawned:#?}");
        assert!(spawned.iter().all(|s| s["program"] == "ssh"));
        // The shared connection's own steps, in order: none was open, the sign-in step opened
        // one (nobody was asked: a key), and it was found open afterwards.
        let ssh = ssh_steps(&hub, "box", n);
        let words: Vec<(&str, Option<&str>)> = ssh
            .iter()
            .map(|r| (r["ev"].as_str().unwrap(), r["state"].as_str()))
            .collect();
        assert_eq!(
            words,
            [
                ("ssh.master_check", Some("absent")),
                ("ssh.signin_started", None),
                ("ssh.master_check", Some("alive")),
                ("ssh.signin_done", None),
            ],
            "{ssh:#?}"
        );
        assert_eq!(ssh[3]["asked"], 0);

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

    /// A wrong key: the attempt ends in the sign-in stage (the step that opens the shared
    /// connection) with ssh's words, classified as a sign-in problem, and the link parks in `auth_needed`. There is no prompt (key sign-in).
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
            state: LinkState::AuthNeeded {
                problem, detail, ..
            },
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
        // The wrong key fails the sign-in step that opens the shared connection, before any
        // link step ran: no preamble (before the shared connection it failed in the one
        // command, after its preamble record).
        assert_eq!(
            evs(&trail),
            [
                "link.attempt_begin",
                "boot.attempt",
                "boot.failed",
                "link.attempt_end"
            ]
        );
        assert_eq!(step(&trail, "boot.failed")["stage"], "sign_in");
        let ssh: Vec<&str> = ssh_steps(&hub, "bad", 1)
            .iter()
            .map(|r| r["ev"].as_str().unwrap())
            .collect();
        assert_eq!(ssh, ["ssh.master_check", "ssh.signin_started"]);
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["lvl"], "warn");
        // (Before the shared connection: `report`, the one command's first step.)
        assert_eq!(failed["stage"], "sign_in");
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
        // (Before the shared connection this failed at `report`, the one command's first step;
        // now it is the sign-in step that cannot reach the host.)
        assert_eq!(failed["stage"], "sign_in");
        // A network-level failure is `other` (retried with backoff, as before): ssh's own
        // refusals that need a person are the only words decided on (see `failure_kind`).
        assert_eq!(failed["kind"], "other");
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
                "#!/bin/sh\ncase \"$1\" in\n  boot) echo 'MARSAD-BOOT2 os=Linux arch={arch} helper_build={build} daemon=absent install=ok'; read l; echo 'MARSAD-OK stop=none install=ok daemon=started'; echo '{MARK}' >&2; exit 2;;\nesac\n",
                arch = std::env::consts::ARCH,
                build = our_build()
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
        assert_eq!(problem, Some(CommandFailureKind::HelperFailed));
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
                "boot.report",
                "boot.action",
                "boot.ok",
                "boot.failed",
                "link.attempt_end"
            ]
        );
        let failed = step(&trail, "boot.failed");
        assert_eq!(failed["stage"], "hello");
        assert_eq!(failed["kind"], "helper_failed");
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
        // The host's report says the helper directory is not writable (`reason=not_writable`), a
        // structural fact: the hub words the refusal itself, with the hint, and the link parks
        // as a configuration problem. The remote's words are not the reason.
        assert_eq!(problem, Some(CommandFailureKind::PermissionDenied));
        assert!(
            reason.contains("Check that your home directory allows running programs"),
            "{reason}"
        );
        assert!(!reason.contains("marsad bootstrap:"), "{reason}");
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
                .contains("Check that your home directory allows running programs"),
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
        // The reports came (the script's, then the installed helper's) and the attempt went on
        // as usual.
        assert_eq!(
            evs(&trail).iter().filter(|e| **e == "boot.report").count(),
            2
        );
        step(&trail, "boot.hello");
    }

    /// The report never comes (the shell startup files print and end the session): the lines
    /// and bytes that did arrive are in the records and in the reason the user is shown.
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
        // One command line for every shell: nothing is tried again another way.
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
        for pre in trail.iter().filter(|r| r["ev"] == "boot.preamble") {
            assert_eq!(
                (&pre["lines"], &pre["bytes"]),
                (&json!(3), &json!(junk.len()))
            );
            assert!(pre["shape"].as_str().unwrap().starts_with("junk-one"));
        }
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

    /// A box and a closed port: the box is connected and has a session, `gone` never connects.
    /// Returns the hub, its fleet, the session's id and the box's data directory.
    async fn a_box_with_a_session_and_a_gone_host(
        d: &TempDir,
        sshd: &Sshd,
        data: &Path,
    ) -> (
        MarsadClient,
        marsad_client::Subscription<FleetEvent>,
        wasit_runtime::wire::SessionId,
    ) {
        let c = hub(data, &[], &[]);
        let mut fleet = c.fleet(None);
        c.add_host(draft("Box", "box", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        // A closed port on this machine: ssh says "Connection refused" at once.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let kind = HostKind::Ssh {
            target: "leaktarget-3c1f@127.0.0.1".into(),
            port: Some(closed),
            identity_file: None,
            options: vec![],
        };
        c.add_host(draft("Gone", "gone", kind)).await.unwrap();
        until(&mut fleet, "box connected", |e| {
            link_is(e, "box", LinkState::is_connected)
        })
        .await;
        until(&mut fleet, "gone reconnecting", |e| {
            link_is(e, "gone", |s| matches!(s, LinkState::Reconnecting { .. }))
        })
        .await;
        let work = sshd.remote_home.join("w");
        std::fs::create_dir_all(&work).unwrap();
        let sc = Scenario::builder()
            .turn(Turn::builder().say("done on the box"))
            .build();
        let mut req = marsad_proto::agentconfig::LaunchRequest::new("claude");
        req.host = Some(HostId::new("box"));
        req.cwd = Some(work.to_string_lossy().into_owned());
        req.first_prompt = Some("hello box".into());
        req.overrides.env = scenario_env(d.path(), &sc);
        let id = c.launch(req).await.unwrap().id;
        let rt = c.runtime(Some(HostId::new("box")));
        let mut sub = rt.subscribe_session(&id, None);
        loop {
            if is_turn_completed(&next_event(&mut sub).await) {
                break;
            }
        }
        (c, fleet, id)
    }

    /// `diag.sources` and `diag.read` answer on the hub's own daemon and, routed by `host`, on an
    /// ssh host: the files the daemon and its helper wrote and the shim log of a session, whole
    /// lines only.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn diag_sources_and_reads_work_locally_and_through_the_hub_to_an_ssh_host() {
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let (c, _fleet, id) = a_box_with_a_session_and_a_gone_host(&d, &sshd, &data).await;
        for host in [None, Some(HostId::new("box"))] {
            let rt = c.runtime(host.clone());
            let s = rt.diag_sources().await.unwrap();
            assert!(!s.boot.is_empty());
            assert!(s.now_ms.abs_diff(now_ms()) < 5_000);
            let names: Vec<&str> = s.sources.iter().map(|x| x.name.as_str()).collect();
            assert!(names.contains(&"daemon"), "{host:?}: {names:?}");
            assert!(
                names.contains(&format!("shim:{id}").as_str()) || host.is_none(),
                "{host:?}: {names:?}"
            );
            let daemon = s.sources.iter().find(|x| x.name == "daemon").unwrap();
            let gen = daemon.gens.last().unwrap();
            assert!(gen.bytes > 0 && gen.last_t.is_some());
            // Whole lines, each a record of that daemon.
            let chunk = rt.diag_read("daemon", gen.gen, 0, 4096).await.unwrap();
            assert!(
                !chunk.data.is_empty() && chunk.data.ends_with('\n'),
                "{host:?}"
            );
            assert_eq!(chunk.next as usize, chunk.data.len());
            let first: Value = serde_json::from_str(chunk.data.lines().next().unwrap()).unwrap();
            assert_eq!(first["diag_file"], 1);
            // A name that is no source is refused on the host.
            for bad in ["../journal", "session.json", "shim:../../x", "store"] {
                assert!(
                    rt.diag_read(bad, 1, 0, 100).await.is_err(),
                    "{host:?}: {bad}"
                );
            }
        }
        drop(c);
        stop(&data).await;
    }

    /// One connected and one unreachable host: the manifest lists both (the unreachable one with
    /// its link state and last error), the hub's own log holds the failed attempts, and the
    /// connected host's logs, shim log and trail are in the file.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_export_with_a_connected_and_an_unreachable_host_lists_both() {
        use super::exporting::*;
        use marsad_proto::export::{ExportRequest, ExportSourceState};
        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let (c, _fleet, id) = a_box_with_a_session_and_a_gone_host(&d, &sshd, &data).await;
        let sid = id.to_string();
        let e = export(&c, &data, ExportRequest::default()).await;

        // The manifest: the hub, the box (connected: its facts), gone (its link and last error).
        assert_eq!(e.manifest()["mode"], "hub");
        let ids: Vec<&str> = e.manifest()["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["hub", "box", "gone"]);
        let bx = e.host("box");
        assert_eq!(
            (
                bx["kind"].as_str(),
                bx["role"].as_str(),
                bx["link"].as_str()
            ),
            (Some("ssh"), Some("host"), Some("connected"))
        );
        assert_eq!(bx["build"], our_build());
        assert_eq!(bx["os"], "linux");
        assert_eq!(bx["target"], our_target());
        assert!(bx["machine"].is_string());
        assert!(
            bx["clock_offset_ms"].as_i64().unwrap().abs() < 5_000,
            "{bx}"
        );
        assert!(bx["rtt_ms"].is_u64());
        assert_eq!(
            (bx["sessions"].as_u64(), bx["sessions_in_window"].as_u64()),
            (Some(1), Some(1))
        );
        let claude = bx["drivers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["driver"] == "claude")
            .unwrap();
        assert_eq!(claude["installed"], true, "{claude}");
        assert!(claude["version"].is_string(), "{claude}");
        assert!(bx["env"]["path"].as_array().is_some_and(|p| !p.is_empty()));
        assert!(bx["failures"].as_array().unwrap().is_empty(), "{bx}");
        let gone = e.host("gone");
        assert_eq!(gone["link"], "reconnecting");
        assert!(
            gone["last_error"]
                .as_str()
                .unwrap()
                .contains("Connection refused"),
            "{gone}"
        );
        assert!(gone.get("build").is_none() && gone.get("clock_offset_ms").is_none());
        // The ssh target is nowhere in the file; the user name is in no record (it can be in a
        // host's `PATH`, which the manifest shows: design decision 2).
        assert!(
            !e.text.contains("leaktarget"),
            "the ssh target is in the export"
        );
        let records: String = e.text.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(
            !records.contains(sshd.user.as_str()),
            "the user name is in a record"
        );

        // The unreachable host is a source that could not be read, with the link's words.
        let un = e.own("source_unavailable");
        assert_eq!(un.len(), 1, "{un:#?}");
        assert_eq!(un[0]["source"], "host:gone");
        assert_eq!(un[0]["link_state"], "reconnecting");
        assert!(un[0]["last_error"]
            .as_str()
            .unwrap()
            .contains("Connection refused"));
        let r = e.result("host:gone");
        assert_eq!(
            (r["outcome"].as_str(), r["reason"].as_str()),
            (Some("not_connected"), Some("reconnecting"))
        );
        assert!(e.section("host:gone", "daemon").is_empty());

        // The hub's own log has the failed attempts of both hosts, joined by (host, n).
        let hub = e.section("host:hub", "daemon");
        let ended: Vec<&&Value> = hub
            .iter()
            .filter(|r| r["ev"] == "link.attempt_end" && r["link"] == "hosts:gone")
            .collect();
        assert!(
            !ended.is_empty() && ended.iter().all(|r| r["outcome"] == "backoff"),
            "{ended:#?}"
        );
        let failed = of_ev(&hub, "boot.failed");
        assert!(failed.iter().any(|r| r["host"] == "gone"
            && r["stderr"]
                .as_str()
                .is_some_and(|t| t.contains("Connection refused"))));
        assert!(of_ev(&hub, "boot.hello").iter().any(|r| r["host"] == "box"));

        // The connected host: its daemon's and its bridge's logs, the session's shim log and trail.
        let daemon = e.section("host:box", "daemon");
        assert_eq!(of_ev(&daemon, "daemon.start").len(), 1);
        assert!(
            daemon
                .iter()
                .all(|r| r["src"] == "daemon" || r["src"] == "hub" || r.get("diag_file").is_some()),
            "{daemon:#?}"
        );
        assert!(!of_ev(&e.section("host:box", "aux"), "bridge.start").is_empty());
        assert!(!of_ev(&e.section("host:box", &format!("shim:{sid}")), "shim.start").is_empty());
        let trail = e.trail("host:box", &sid);
        let kinds: Vec<&str> = trail.iter().map(|t| t["ev"].as_str().unwrap()).collect();
        for ev in ["input.applied", "input.written", "turn.completed", "phase"] {
            assert!(kinds.contains(&ev), "{ev} in {kinds:?}");
        }
        // Nothing of the session's content.
        for never in ["hello box", "done on the box"] {
            assert!(!e.text.contains(never), "{never} is in the export");
        }

        // How the topic and the end line see it.
        let end = e.end();
        assert_eq!(end["complete"], false);
        assert_eq!(e.status.complete, Some(false));
        let state = |s: &str| {
            e.status
                .sources
                .iter()
                .find(|x| x.source == s)
                .unwrap()
                .state
        };
        assert_eq!(state("host:box"), ExportSourceState::Done);
        assert_eq!(state("host:hub"), ExportSourceState::Done);
        assert_eq!(state("host:gone"), ExportSourceState::NotConnected);
        assert_eq!(e.result("host:box")["outcome"], "done");
        drop(c);
        stop(&data).await;
    }

    /// The owner's requirement: a marked prompt and a marked secret through a session, and
    /// neither is in any log or in the export. A hub and one ssh host with the fake CLI; a unique
    /// marker through every content channel (first prompt, a queued prompt, an attachment's
    /// content and name, assistant text, thinking, a tool's input and result, a permission gate
    /// answered "deny" with text, a question's answer, the CLI's stderr, a CLI failure, a garbage
    /// stdout line, a session's new name, the project directory's name, a host secret variable,
    /// an MCP server's env value in a session layer, a sign-in prompt and its answer); then an
    /// export through `diagnostics.export.start`. Scanned afterwards: everything under both
    /// machines' `diag/`, both `marsad-agent.log`, every `shim.log*` and `shim.stderr`, and the
    /// export, each marker raw, JSON-escaped, base64 and UTF-16LE. The positive half is in the
    /// same test, so it cannot pass on nothing: the markers ARE in the durable data of the
    /// session (so the channels carried them) and the export holds the diagnostics a root cause
    /// is found from.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_content_in_any_log_or_export() {
        use super::exporting::*;
        use marsad_proto::agentconfig::{
            ItemOverride, ItemValue, LaunchReply, OverrideAction, OverrideLayer,
        };
        use marsad_proto::export::ExportRequest;
        use marsad_proto::hosts::HostVar;
        use marsad_proto::{
            AgentEvent, ConfigItemKind, GateDecision, GateRequest, McpTransport, QuestionAnswer,
        };
        const FIRST: &str = "LEAK\"first-prompt\"a1b2c3";
        const QUEUED: &str = "LEAK\"queued-prompt\"d4e5f6";
        const FINAL: &str = "LEAK\"final-prompt\"0a1b2c";
        const ATT_CONTENT: &str = "LEAK-attachment-content-4d5e6f7a";
        const ATT_NAME: &str = "LEAKATTNAME-7a8b9c.txt";
        const SAY: &str = "LEAK\"assistant-text\"3d4e5f";
        const THINK: &str = "LEAK\"thinking\"6a7b8c";
        const TOOL_INPUT: &str = "LEAK\"tool-input\"9d0e1f";
        const TOOL_RESULT: &str = "LEAK\"tool-result\"2a3b4c";
        const GATE_INPUT: &str = "LEAK\"gated-tool-input\"5d6e7f";
        const DENY: &str = "LEAK\"deny-reason\"8a9b0c";
        const QUESTION: &str = "LEAK\"question-text\"1d2e3f";
        const ANSWER_Q: &str = "LEAK\"question-answer\"4a5b6c";
        const STDERR: &str = "LEAK\"cli-stderr\"7d8e9f";
        const FAIL: &str = "LEAK\"cli-failure\"0a1b2d";
        const GARBAGE: &str = "LEAK\"garbage-line\"3d4e5a";
        const RENAME: &str = "LEAK\"session-name\"6b7c8d";
        const PROJECT: &str = "PROJECTDIRLEAK-9e0f1a";
        const HOST_SECRET: &str = "HOSTSECRET-leak-2b3c4d5e";
        const MCP_ENV: &str = "LEAK\"mcp-env-value\"7f8a9b";
        const SIGNIN_TEXT: &str = "PROMPTTEXT-leak-1c2d3e password:";
        const SIGNIN_ANSWER: &str = "ANSWER\"leak\"4f5a6b7c";
        const TRACE: &str = "u-leaktest00000001";
        let markers = [
            FIRST,
            QUEUED,
            FINAL,
            ATT_CONTENT,
            ATT_NAME,
            SAY,
            THINK,
            TOOL_INPUT,
            TOOL_RESULT,
            GATE_INPUT,
            DENY,
            QUESTION,
            ANSWER_Q,
            STDERR,
            FAIL,
            GARBAGE,
            RENAME,
            PROJECT,
            HOST_SECRET,
            MCP_ENV,
            SIGNIN_TEXT,
            SIGNIN_ANSWER,
        ];

        let d = TempDir::new();
        let sshd = Sshd::start(d.path());
        let data = d.path().join("l");
        let l = layout(&data);
        let c = hub(&data, &[], &[]);
        c.daemon_info(None).await.unwrap();
        let mut fleet = c.fleet(None);
        let box_id = HostId::new("box");
        c.add_host(draft("Box", "box", sshd.kind(&sshd.key)))
            .await
            .unwrap();
        // Connected, and the registry taught the host's machine (from the report, before the
        // link was up, or after: either order).
        let (up, known) = (std::cell::Cell::new(false), std::cell::Cell::new(false));
        until(&mut fleet, "box connected and its machine learned", |e| {
            up.set(up.get() || link_is(e, "box", LinkState::is_connected));
            known.set(known.get() || host_is(e, "box", |h| h.machine.is_some()));
            up.get() && known.get()
        })
        .await;

        // A host secret variable, through the host-environment method.
        let saved = c
            .set_host_env(HostEnvSettings {
                host: box_id.clone(),
                setup: String::new(),
                vars: vec![HostVar {
                    name: "LEAK_HOST_SECRET".into(),
                    value: Some(HOST_SECRET.into()),
                    secret: true,
                }],
                commands: Default::default(),
                revision: String::new(),
            })
            .await
            .unwrap();
        assert!(saved.not_applied.is_none(), "{saved:?}");

        // A sign-in prompt and its answer (the real askpass helper and methods).
        let helper = super::signin::askpass(&data, SIGNIN_TEXT);
        let p = super::signin::prompt(&mut fleet).await;
        assert_eq!(p.prompt, SIGNIN_TEXT);
        c.answer_auth(&HostId::new("hub"), p.id, SIGNIN_ANSWER)
            .await
            .unwrap();
        let signed_in = super::signin::finished(helper).await;
        assert_eq!(
            String::from_utf8_lossy(&signed_in.stdout).trim_end(),
            SIGNIN_ANSWER
        );

        // The session on the box: its project directory is named by a marker; its session layer
        // has an MCP server whose env holds one; the CLI plays a script that touches every
        // other channel.
        let work = sshd.remote_home.join(PROJECT);
        std::fs::create_dir_all(&work).unwrap();
        let sc = Scenario::builder()
            .turn(
                Turn::builder()
                    .think(THINK)
                    .tool("Read", json!({ "file_path": TOOL_INPUT }))
                    .result_if_allowed(TOOL_RESULT)
                    .done()
                    .stderr(STDERR)
                    .garbage(GARBAGE)
                    .tool("Bash", json!({ "command": GATE_INPUT }))
                    .needs_permission()
                    .result_if_denied("denied by the user")
                    .done()
                    .ask_question(QUESTION, &[("alpha", "the first"), ("beta", "the second")])
                    .say(SAY)
                    .wait_for_interrupt(),
            )
            .turn(Turn::when("final-prompt").fail(FAIL))
            .build();
        let mut req = marsad_proto::agentconfig::LaunchRequest::new("claude");
        req.host = Some(box_id.clone());
        req.cwd = Some(work.to_string_lossy().into_owned());
        req.first_prompt = Some(FIRST.into());
        req.overrides = OverrideLayer {
            items: vec![ItemOverride {
                kind: ConfigItemKind::McpServer,
                name: "leaky".into(),
                driver: None,
                action: OverrideAction::Set {
                    value: ItemValue::McpServer {
                        server: McpTransport::Stdio {
                            command: "cat".into(),
                            args: vec![],
                            env: [("LEAK_MCP_ENV".to_string(), MCP_ENV.to_string())].into(),
                        },
                    },
                },
            }],
            env: scenario_env(d.path(), &sc),
            ..OverrideLayer::default()
        };
        let trace = wasit_control::TraceId::new(TRACE).unwrap();
        let launched: LaunchReply = c
            .call_traced(None, methods::SESSIONS_LAUNCH, req, Some(trace.clone()))
            .await
            .unwrap();
        let id = launched.id.clone();
        let rt = c.runtime(Some(box_id.clone()));
        let mut sub = rt.subscribe_session(&id, None);

        // The first turn: a permission gate (denied, with words), a question (answered), then the
        // assistant's text; the turn then waits.
        let mut gates = 0;
        loop {
            let e = next_event(&mut sub).await;
            if let SessionEventKind::Agent {
                event: AgentEvent::GateRequested { gate },
            } = &e.kind
            {
                let request_id = gate.request_id().to_string();
                let command = match gate {
                    GateRequest::Question { questions, .. } => Command::AnswerQuestion {
                        request_id,
                        answer: QuestionAnswer {
                            answers: [(questions[0].id.clone(), vec![ANSWER_Q.to_string()])].into(),
                        },
                    },
                    _ => Command::AnswerGate {
                        request_id,
                        decision: GateDecision::choose_with("deny", Some(DENY)),
                    },
                };
                rt.command(&id, command).await.unwrap();
                gates += 1;
            }
            if contains_any(&serde_json::to_string(&e.kind).unwrap(), SAY) {
                break;
            }
        }
        assert_eq!(gates, 2, "a permission gate and a question");

        // A prompt while the turn runs, then an interrupt; a prompt with an attachment whose
        // turn fails; a new name.
        let prompting = rt
            .clone()
            .with_trace(wasit_control::TraceId::new("u-leaktest00000002").unwrap());
        prompting
            .command(&id, Command::prompt(QUEUED))
            .await
            .unwrap();
        rt.command(&id, Command::Interrupt).await.unwrap();
        let att = rt
            .upload(&id, "upleak0001", ATT_NAME, ATT_CONTENT.as_bytes())
            .await
            .unwrap();
        rt.command(
            &id,
            Command::Prompt {
                text: FINAL.into(),
                attachments: vec![att],
            },
        )
        .await
        .unwrap();
        loop {
            let e = next_event(&mut sub).await;
            if contains_any(&serde_json::to_string(&e.kind).unwrap(), FAIL) {
                break;
            }
        }
        rt.rename(&id, Some(RENAME.into())).await.unwrap();
        loop {
            if rt.snapshot(&id).await.unwrap().phase == marsad_proto::runtime::Phase::Idle {
                break;
            }
            let e = next_event(&mut sub).await;
            let _ = e;
        }
        drop(sub);

        // The export.
        let e = export(&c, &data, ExportRequest::default()).await;
        drop(fleet);
        drop(rt);
        drop(prompting);
        drop(c);
        stop(&data).await;

        // ── positive: the channels were driven, the export holds what a root cause needs ──
        let box_machines = sshd.remote_home.join(".marsad/machines");
        let box_dir: PathBuf = std::fs::read_dir(&box_machines)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .next()
            .expect("the box's machine directory");
        let hub_dir = l.data.clone();
        // Every marker is stored where it belongs: the session's journal on the box for what was
        // said, done and asked, its `session.json` for its name and folder, its attachments, the
        // hub's store for the secret and the layer. (The sign-in prompt and its answer live in
        // memory only; they were seen above.)
        let session_dir = box_dir.join("sessions").join(id.to_string());
        let read_all = |dir: &Path| {
            let mut fs = Vec::new();
            files(dir, &mut fs);
            fs.sort();
            fs.iter()
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let journal = std::fs::read_to_string(session_dir.join("journal.ndjson")).unwrap();
        let session_file = std::fs::read_to_string(session_dir.join("session.json")).unwrap();
        let attachments = read_all(&session_dir.join("attachments"));
        let store = read_all(&hub_dir.join("store"));
        let stored: [(&str, &str, &str); 20] = [
            ("the first prompt", FIRST, &journal),
            ("the queued prompt", QUEUED, &journal),
            ("the last prompt", FINAL, &journal),
            ("an attachment's name", ATT_NAME, &journal),
            ("an attachment's content", ATT_CONTENT, &attachments),
            ("assistant text", SAY, &journal),
            ("thinking", THINK, &journal),
            ("a tool's input", TOOL_INPUT, &journal),
            ("a tool's result", TOOL_RESULT, &journal),
            ("a gated tool's input", GATE_INPUT, &journal),
            ("a deny reason", DENY, &journal),
            ("a question", QUESTION, &journal),
            ("a question's answer", ANSWER_Q, &journal),
            ("the CLI's stderr", STDERR, &journal),
            ("a CLI failure", FAIL, &journal),
            ("a garbage line", GARBAGE, &journal),
            ("the session's name", RENAME, &session_file),
            ("the project folder", PROJECT, &session_file),
            ("a host secret", HOST_SECRET, &store),
            ("an MCP server's env", MCP_ENV, &store),
        ];
        for (what, marker, place) in stored {
            assert!(
                contains_any(place, marker),
                "{what} ({marker}) is not where it belongs: the channel was not driven"
            );
        }

        // The manifest: both machines, their facts.
        let m = e.manifest();
        assert_eq!(m["hub_machine"], l.machine.as_str());
        assert_eq!(
            e.host("box")["machine"],
            box_dir.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(e.host("hub")["machine"], l.machine.as_str());
        // The hub's and the host's daemons.
        let hub_log = e.section("host:hub", "daemon");
        let box_log = e.section("host:box", "daemon");
        assert_eq!(of_ev(&hub_log, "daemon.start").len(), 1);
        assert_eq!(of_ev(&box_log, "daemon.start").len(), 1);
        // The session's trail, on the host: the first prompt with the launch's trace, written and
        // acknowledged, a phase with its cause, the queued prompt, the attachment.
        let sid = id.to_string();
        let trail = e.trail("host:box", &sid);
        let kind = |k: &str| -> Vec<&&Value> { trail.iter().filter(|t| t["ev"] == k).collect() };
        let applied = kind("input.applied");
        assert!(
            applied
                .iter()
                .any(|t| t["trace"] == TRACE && t["kind"] == "prompt"),
            "{applied:#?}"
        );
        assert!(applied.iter().any(|t| t["trace"] == "u-leaktest00000002"));
        assert!(
            applied
                .iter()
                .any(|t| t["attachments"] == 1 && t["bytes"] == ATT_CONTENT.len()),
            "{applied:#?}"
        );
        assert!(!kind("input.written").is_empty());
        assert!(
            !kind("input.acknowledged").is_empty(),
            "{:?}",
            trail
                .iter()
                .map(|t| t["ev"].as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert!(
            kind("phase").iter().any(|t| t["cause"].is_object()),
            "{:#?}",
            kind("phase")
        );
        assert!(!kind("gate.raised").is_empty() && !kind("process.stderr").is_empty());
        // The hub recorded the launch and the request under the trace, and each sign-in step.
        assert!(of_ev(&hub_log, "launch")
            .iter()
            .any(|r| r["trace"] == TRACE && r["host"] == "box" && r["outcome"] == "ok"));
        assert!(of_ev(&hub_log, "rpc.traced")
            .iter()
            .any(|r| r["trace"] == TRACE));
        assert_eq!(of_ev(&hub_log, "auth.prompt").len(), 1);
        assert_eq!(of_ev(&hub_log, "auth.answered").len(), 1);
        // The connection to the ssh host: the steps of the connect and the link's states.
        for ev in ["boot.attempt", "boot.report", "boot.hello"] {
            assert!(
                of_ev(&hub_log, ev).iter().any(|r| r["host"] == "box"),
                "{ev}"
            );
        }
        assert!(of_ev(&hub_log, "link.attempt_end")
            .iter()
            .any(|r| r["link"] == "hosts:box" && r["outcome"] == "connected"));
        assert!(of_ev(&hub_log, "link.state")
            .iter()
            .any(|r| r["link"] == "hosts:box" && r["to"] == "connected"));
        // The box's shim log and its helper's records.
        assert!(!e.section("host:box", &format!("shim:{sid}")).is_empty());
        assert!(!e.section("host:box", "aux").is_empty());
        assert_eq!(e.end()["complete"], true);

        // ── negative: no marker in any file, in any form ──
        let mut scanned: Vec<(String, Vec<u8>)> =
            vec![("the export".into(), e.text.clone().into_bytes())];
        for dir in [&box_dir, &hub_dir] {
            let mut fs = Vec::new();
            files(&dir.join("diag"), &mut fs);
            for (path, bytes) in fs {
                // An export of the hub's `diag/exports/` is read as the text it holds.
                let bytes = if path.to_string_lossy().ends_with(".ndjson.gz") {
                    marsad_agent::hub::export::gzip::gunzip(&bytes).unwrap()
                } else {
                    bytes
                };
                scanned.push((path.display().to_string(), bytes));
            }
            if let Ok(b) = std::fs::read(dir.join("marsad-agent.log")) {
                scanned.push((dir.join("marsad-agent.log").display().to_string(), b));
            }
            let mut fs = Vec::new();
            files(&dir.join("sessions"), &mut fs);
            for (path, bytes) in fs {
                let n = path.file_name().unwrap().to_string_lossy().into_owned();
                if n.starts_with("shim.log") || n == "shim.stderr" {
                    scanned.push((path.display().to_string(), bytes));
                }
            }
        }
        // The scan sees what it must: both machines' daemon logs, the shim's, the export.
        let names: Vec<&str> = scanned.iter().map(|(n, _)| n.as_str()).collect();
        for want in ["daemon.ndjson", "aux.ndjson", "shim.log", "the export"] {
            assert!(
                names.iter().any(|n| n.contains(want)),
                "{want} among {names:?}"
            );
        }
        assert!(
            names
                .iter()
                .filter(|n| n.ends_with("daemon.ndjson"))
                .count()
                >= 2,
            "{names:?}"
        );
        let mut leaks = Vec::new();
        for (name, bytes) in &scanned {
            for m in markers {
                if let Some(form) = found_in(bytes, m) {
                    leaks.push(format!("{m} ({form}) in {name}"));
                }
            }
        }
        assert!(leaks.is_empty(), "{leaks:#?}");
    }
}

// ── a session feed that ends ─────────────────────────────────────────────────

/// A host whose daemon refuses the `host.sessions` subscription (an old daemon that does not know
/// the topic): the refusal is on record with the service's code and shown as the host's feed
/// error. The feed does not end: it waits for the host's link to connect again.
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
    fleet
        .host_upserted(
            marsad_proto::hosts::HostEntry {
                id: marsad_proto::HostId::new("old"),
                name: "old".into(),
                kind: marsad_proto::hosts::HostKind::Local,
                platform: None,
                created_at_ms: 0,
                interactive_auth: false,
                ask_before_connecting: false,
                local_files: false,
                shared_connection: false,
                signed_out: false,
                machine: None,
            },
            marsad_proto::fleet::FleetLink::Remote {
                state: marsad_proto::LinkState::Connecting {
                    stage: None,
                    since_ms: 0,
                },
            },
        )
        .await;
    let remote = fleet.feed_remote(marsad_proto::HostId::new("old"), link.clone());
    let mut events = fleet.subscribe(None).await.unwrap();
    let shown = tokio::time::timeout(T, async {
        loop {
            let (_, v) = futures::StreamExt::next(&mut events)
                .await
                .expect("the fleet is open")
                .expect("an event");
            if let marsad_proto::fleet::FleetEvent::FeedError {
                host,
                error: Some(text),
            } = serde_json::from_value(v).expect("a fleet event")
            {
                if host.as_str() == "old" {
                    return text;
                }
            }
        }
    })
    .await
    .expect("the refusal is shown as the host's feed error");
    assert!(shown.starts_with("topic_not_found"), "{shown}");
    assert!(!remote.is_finished(), "the feed waits for the link");
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
    remote.abort();
    link.stop().await;
}

// ── the diagnostics export ───────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// A valid app log with records of the desktop app (the shell writes the real one; the exporter
/// only reads it).
fn write_app_log(l: &Layout) {
    let t = now_ms();
    let mut text = json!({"diag_file":1,"gen":1,"b":"app0boot","src":"app","build":"test","pid":1,"opened_t":t})
        .to_string();
    for (seq, ev) in [(1, "app.start"), (2, "app.hub_link")] {
        text.push('\n');
        text.push_str(
            &json!({"b":"app0boot","seq":seq,"t":t,"m":seq,"src":"app","lvl":"info","ev":ev})
                .to_string(),
        );
    }
    text.push('\n');
    std::fs::create_dir_all(l.diag()).unwrap();
    std::fs::write(l.diag().join("app.ndjson"), text).unwrap();
}

/// The hub alone: one export is one file; what it holds, what it must not hold, what a second
/// start does, how many are kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_is_one_private_file_with_the_manifest_the_logs_and_the_trail() {
    use exporting::*;
    use marsad_client::ClientError;
    use marsad_proto::export::{ExportRequest, ExportSourceState, EXPORTS_KEPT};
    use marsad_proto::MarsadError;
    const PROMPT: &str = "EXPORT\"prompt\"4c19e0";
    const SAY: &str = "EXPORT\"say\"b27a63";
    const STORE_MARKER: &str = "SENTINEL-store-layer-77c2";
    const LOG_SENTINEL: &str = "SENTINEL-daemon-stderr-a81d";
    const STDERR_SENTINEL: &str = "SENTINEL-shim-stderr-5e03";
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let work = d.path().join("export-project");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&l.data).unwrap();
    std::fs::write(l.log(), format!("{LOG_SENTINEL}\n")).unwrap();

    let sc = Scenario::builder().turn(Turn::builder().say(SAY)).build();
    let c = client(&data);
    // A stored document: the hub's store is not a diagnostics source.
    c.set_layer(
        marsad_proto::agentconfig::LayerScope::Global,
        marsad_proto::agentconfig::OverrideLayer {
            env: vec![("EXPORT_STORE_VAR".into(), STORE_MARKER.into())],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some(PROMPT.into());
    req.overrides.env = scenario_env(d.path(), &sc);
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        if is_turn_completed(&next_event(&mut sub).await) {
            break;
        }
    }
    drop(sub);
    // Files the export must never read: the shim's raw stderr, the daemon's stderr (above).
    let session_dir = l.data.join("sessions").join(id.to_string());
    std::fs::write(
        session_dir.join("shim.stderr"),
        format!("{STDERR_SENTINEL}\n"),
    )
    .unwrap();
    write_app_log(&l);

    // A window in the future or beyond a month is refused, not clamped.
    for bad in [now_ms() + 3_600_000, now_ms() - 40 * 86_400_000] {
        let e = start(
            &c,
            ExportRequest {
                since_ms: Some(bad),
                include_trails: true,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(e, ClientError::Marsad(MarsadError::InvalidRequest { .. })),
            "{e:?}"
        );
    }

    // Two starts at once: one export runs, the other is refused naming it.
    let mut sub = c.subscribe_raw(None, marsad_proto::topics::DIAGNOSTICS_EXPORTS, None);
    let (a, b) = tokio::join!(
        start(&c, ExportRequest::default()),
        start(&c, ExportRequest::default())
    );
    let (started, refused) = match (a, b) {
        (Ok(s), Err(e)) | (Err(e), Ok(s)) => (s, e),
        other => panic!("exactly one of two starts must run: {other:?}"),
    };
    let ClientError::Marsad(MarsadError::Conflict { message }) = &refused else {
        panic!("{refused:?}");
    };
    assert!(message.contains(&started.id), "{message}");
    let first = finish(&mut sub, &data, &started.id).await;

    // The file: private, a real gzip file, the manifest first and the end line last.
    #[cfg(unix)]
    {
        assert_eq!(mode(&first.path), 0o600);
        assert_eq!(mode(first.path.parent().unwrap()), 0o700);
        let t = std::process::Command::new("gzip")
            .arg("-t")
            .arg(&first.path)
            .status()
            .expect("gzip");
        assert!(t.success(), "gzip -t");
    }
    assert!(first
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("marsad-diagnostics-hub-"));
    let m = first.manifest();
    assert_eq!(
        (m["format"].as_u64(), m["mode"].as_str()),
        (Some(1), Some("hub"))
    );
    assert_eq!(m["hub_host"], "hub");
    assert_eq!(m["include_trails"], true);
    assert_eq!(m["id"], first.status.id.as_str());
    assert!(m["since"].as_u64().unwrap() < m["until"].as_u64().unwrap());
    assert_eq!(m["hosts"].as_array().unwrap().len(), 1);
    let hub = first.host("hub");
    assert_eq!(
        (
            hub["kind"].as_str(),
            hub["role"].as_str(),
            hub["link"].as_str()
        ),
        (Some("local"), Some("hub"), Some("local"))
    );
    assert!(hub["build"].is_string() && hub["machine"].is_string());
    assert!(
        hub["clock_offset_ms"].as_i64().unwrap().abs() < 2000,
        "{hub}"
    );
    assert!(hub["rtt_ms"].is_u64());
    assert_eq!(hub["sessions"], 1);
    assert_eq!(hub["sessions_in_window"], 1);
    let drivers: Vec<&str> = hub["drivers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["driver"].as_str().unwrap())
        .collect();
    assert_eq!(drivers, ["claude", "codex", "opencode", "kilo"]);
    assert_eq!(hub["env"]["fact"], "env");
    assert!(hub["env"]["path"].as_array().is_some_and(|p| !p.is_empty()));
    assert!(hub["failures"].as_array().unwrap().is_empty(), "{hub}");

    // The sections: the app's log, the hub's own logs, the session's trail and shim log.
    let app = first.section("app", "app");
    assert_eq!(of_ev(&app, "app.start").len(), 1);
    let daemon = first.section("host:hub", "daemon");
    for ev in ["daemon.start", "daemon.ready"] {
        assert_eq!(of_ev(&daemon, ev).len(), 1, "{ev}");
    }
    assert!(daemon
        .iter()
        .all(|r| r["src"] == "hub" || r["src"] == "daemon" || r.get("diag_file").is_some()));
    let sid = id.to_string();
    let trail = first.trail("host:hub", &sid);
    let kinds: Vec<&str> = trail.iter().map(|e| e["ev"].as_str().unwrap()).collect();
    for ev in ["input.applied", "input.written", "turn.completed", "phase"] {
        assert!(kinds.contains(&ev), "{ev} in {kinds:?}");
    }
    let shim = first.section("host:hub", &format!("shim:{sid}"));
    assert!(!of_ev(&shim, "shim.start").is_empty());
    let end = first.end();
    assert_eq!(end["complete"], true);
    assert_eq!(end["skipped"], 0);
    assert_eq!(first.result("app")["outcome"], "done");
    assert_eq!(first.result("host:hub")["outcome"], "done");

    // What it must never hold: the journal's content, the daemon's and the shim's raw stderr, the
    // project's name, the store's documents. Its sections are the logs, never anything else.
    for never in [
        PROMPT,
        SAY,
        LOG_SENTINEL,
        STDERR_SENTINEL,
        "export-project",
        STORE_MARKER,
    ] {
        assert!(
            !contains_any(&first.text, never),
            "{never} is in the export"
        );
    }
    for s in first.own("section") {
        let f = s["file"].as_str().unwrap();
        assert!(
            ["app", "daemon", "aux"].contains(&f) || f.starts_with("shim:"),
            "a section of {f}"
        );
    }

    // The topic showed it come about: the sources were all waiting, then each ended.
    assert!(first.seen.iter().any(|s| s.sources.len() == 2
        && s.sources
            .iter()
            .all(|x| x.state == ExportSourceState::Waiting)));
    let last = &first.status;
    assert_eq!(last.complete, Some(true));
    assert!(last
        .sources
        .iter()
        .all(|s| s.state == ExportSourceState::Done));

    // A third export is a new one; only the newest are kept.
    let mut ids = vec![first.status.id.clone()];
    for _ in 0..EXPORTS_KEPT + 1 {
        ids.push(export(&c, &data, ExportRequest::default()).await.status.id);
    }
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    let kept: Vec<String> = std::fs::read_dir(first.path.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept.len(), EXPORTS_KEPT, "{kept:?}");
    assert!(kept.iter().all(|n| n.ends_with(".ndjson.gz")), "{kept:?}");
    assert!(
        !kept.iter().any(|n| n.contains(&ids[0])),
        "the oldest is gone"
    );
    assert!(kept.iter().any(|n| n.contains(ids.last().unwrap())));

    // The hub recorded each export: its start, how each source ended, its end.
    drop(c);
    stop(&data).await;
    let hub = records(&l.diag().join("daemon.ndjson"));
    let starts: Vec<&Value> = of(&hub, "export.start");
    assert_eq!(starts.len(), ids.len(), "{starts:#?}");
    assert!(starts
        .iter()
        .all(|r| r["mode"] == "hub" && r["hosts"] == 1 && r["include_trails"] == true));
    let first_id = first.status.id.as_str();
    let done: Vec<&Value> = of(&hub, "export.done");
    assert_eq!(done.len(), ids.len());
    assert!(done
        .iter()
        .all(|r| r["complete"] == true && r["skipped"] == 0 && r.get("error").is_none()));
    // One record per source and export (`app` and the hub's own computer), in the order they ended.
    let sources: Vec<(&str, &str)> = of(&hub, "export.source")
        .iter()
        .take(2)
        .map(|r| {
            (
                r["source"].as_str().unwrap(),
                r["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(of(&hub, "export.source").len(), 2 * ids.len());
    assert!(
        sources.contains(&("app", "done")) && sources.contains(&("host", "done")),
        "{sources:?}"
    );
    let hub_source = of(&hub, "export.source")
        .into_iter()
        .find(|r| r["source"] == "host")
        .unwrap();
    assert_eq!(hub_source["host"], "hub");
    assert!(
        hub_source["bytes"].as_u64().unwrap() > 0 && hub_source["sections"].as_u64().unwrap() >= 3
    );
    assert!(starts.iter().any(|r| r["id"] == first_id));
}

/// The records of `recs` of event `ev`.
fn of_ev<'a>(recs: &[&'a Value], ev: &str) -> Vec<&'a Value> {
    recs.iter().copied().filter(|r| r["ev"] == ev).collect()
}

/// A cap too small for any section: every section is listed as left out (with its size where it
/// is known), nothing is silently truncated, and the file is still a whole export.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_over_its_cap_lists_what_it_left_out() {
    use exporting::*;
    use marsad_proto::export::{ExportRequest, ExportSourceState};
    let d = TempDir::new();
    let data = d.path().join("d");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let mut cfg = client_config(&data, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.serve_args.extend(serve_args());
    cfg.serve_args
        .extend(["--export-cap-bytes".into(), "3000".into()]);
    let c = marsad_client::MarsadClient::connect(cfg);
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some("hello".into());
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        if is_turn_completed(&next_event(&mut sub).await) {
            break;
        }
    }
    let e = export(&c, &data, ExportRequest::default()).await;
    assert_eq!(e.manifest()["cap_bytes"], 3000);
    assert!(
        e.own("section").is_empty() && e.own("trail").is_empty(),
        "{:#?}",
        e.lines
    );
    let left: Vec<(String, String)> = e
        .own("skipped")
        .iter()
        .map(|s| {
            assert_eq!(s["why"], "size_cap");
            (
                s["source"].as_str().unwrap().into(),
                s["file"].as_str().unwrap().into(),
            )
        })
        .collect();
    // The hub's own logs, the session's trail and its shim log: all named.
    for want in [
        "daemon".to_owned(),
        format!("trail:{id}"),
        format!("shim:{id}"),
    ] {
        assert!(
            left.contains(&("host:hub".to_owned(), want.clone())),
            "{want} in {left:?}"
        );
    }
    assert!(e
        .own("skipped")
        .iter()
        .any(|s| s["file"] == "daemon" && s["bytes"].as_u64().unwrap() > 0));
    let end = e.end();
    assert_eq!(end["complete"], false);
    assert_eq!(end["skipped"].as_u64().unwrap() as usize, left.len());
    assert_eq!(end["sections"], 0);
    let r = e.result("host:hub");
    assert_eq!(
        (r["outcome"].as_str(), r["reason"].as_str()),
        (Some("partial"), Some("size_cap"))
    );
    assert_eq!(e.status.complete, Some(false));
    let hub = e
        .status
        .sources
        .iter()
        .find(|s| s.source == "host:hub")
        .unwrap();
    assert_eq!(hub.state, ExportSourceState::Partial);
    assert!(hub.reason.as_deref().unwrap().contains("size_cap"));
    drop(sub);
    drop(rt);
    drop(c);
    stop(&data).await;
}

/// `marsad-agent diagnostics export`: through the hub when one serves, else this machine's files
/// only, marked as such; bad options are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_line_export_asks_the_hub_or_reads_the_local_files() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let out = d.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let cli = |args: &[&str]| {
        let mut cmd = std::process::Command::new(agent());
        cmd.arg("diagnostics")
            .arg("export")
            .args(data_args(&data))
            .args(args);
        cmd
    };
    let read = |p: &Path| -> Vec<Value> {
        let bytes = marsad_agent::hub::export::gzip::gunzip(&std::fs::read(p).unwrap()).unwrap();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };

    let c = client(&data);
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some("hello".into());
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        if is_turn_completed(&next_event(&mut sub).await) {
            break;
        }
    }

    // With the hub: the file the hub wrote, copied into the directory asked for.
    let o = tokio::task::spawn_blocking({
        let mut cmd = cli(&[
            "--since",
            "3d",
            "--no-trails",
            "--out",
            out.to_str().unwrap(),
        ]);
        move || cmd.output().unwrap()
    })
    .await
    .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let copied = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
    assert_eq!(copied.parent().unwrap(), out);
    let lines = read(&copied);
    assert_eq!(lines[0]["mode"], "hub");
    // `--no-trails`: the logs, not the sessions' trails.
    assert_eq!(lines[0]["include_trails"], false);
    assert!(lines.iter().all(|l| l["line"] != "trail"));
    assert!(lines
        .iter()
        .any(|l| l["line"] == "section" && l["file"] == "daemon"));
    let window = lines[0]["until"].as_u64().unwrap() - lines[0]["since"].as_u64().unwrap();
    assert!(
        (3 * 86_400_000..3 * 86_400_000 + 60_000).contains(&window),
        "{window}"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("host:hub: Done"));
    assert!(std::fs::read_dir(l.diag().join("exports"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name() == copied.file_name().unwrap()));

    // Without it: this machine's own files, without trails, and the manifest says so.
    drop(sub);
    drop(rt);
    drop(c);
    stop(&data).await;
    let file = out.join("local.ndjson.gz");
    let o = tokio::task::spawn_blocking({
        let mut cmd = cli(&["--out", file.to_str().unwrap()]);
        move || cmd.output().unwrap()
    })
    .await
    .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let lines = read(&file);
    assert_eq!(lines[0]["line"], "manifest");
    assert_eq!(lines[0]["mode"], "local_files");
    assert_eq!(lines[0]["note"], "no_daemon");
    assert_eq!(lines[0]["hosts"].as_array().unwrap().len(), 0);
    let section_of = |file: &str| {
        let i = lines
            .iter()
            .position(|l| l["line"] == "section" && l["source"] == "local" && l["file"] == file)
            .unwrap_or_else(|| panic!("no {file} section"));
        lines[i + 1..]
            .iter()
            .take_while(|l| l.get("line").is_none())
            .collect::<Vec<_>>()
    };
    assert!(section_of("daemon")
        .iter()
        .any(|r| r["ev"] == "daemon.shutdown"));
    assert!(section_of(&format!("shim:{id}"))
        .iter()
        .any(|r| r["ev"] == "shim.start"));
    assert!(lines.iter().all(|l| l["line"] != "trail"));
    let end = lines.last().unwrap();
    assert_eq!(
        (end["line"].as_str(), end["complete"].as_bool()),
        (Some("end"), Some(true))
    );

    // A bad window is refused before anything is read.
    let o = tokio::task::spawn_blocking({
        let mut cmd = cli(&["--since", "2w"]);
        move || cmd.output().unwrap()
    })
    .await
    .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("is not a duration"));
}

/// The leak scan itself: it finds a marker in every form it can take, and nothing in plain text.
#[test]
fn the_leak_scan_finds_a_marker_in_every_form_it_can_take() {
    use wasit_core::content::b64_encode;
    let m = "LEAK\"probe\"1a2b3c4d5e";
    assert_eq!(found_in(format!("x {m} y").as_bytes(), m), Some("raw"));
    let once = forms(m)[1].clone();
    assert_eq!(
        found_in(format!("{{\"t\":\"{once}\"}}").as_bytes(), m),
        Some("json-escaped")
    );
    let twice = forms(m)[2].clone();
    assert_eq!(found_in(twice.as_bytes(), m), Some("json-escaped twice"));
    // Inside a longer base64 text, at each of the three alignments.
    for k in 0..3 {
        let mut bytes = vec![b'q'; k];
        bytes.extend_from_slice(m.as_bytes());
        bytes.extend_from_slice(b"-and the rest of the text");
        let text = format!("data:{}=", b64_encode(&bytes));
        assert_eq!(
            found_in(text.as_bytes(), m),
            Some("base64"),
            "alignment {k}"
        );
    }
    // UTF-16LE, in a binary file.
    let mut wide = vec![0u8, 1, 2];
    wide.extend(m.encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(found_in(&wide, m), Some("utf-16le"));
    // Nothing to find.
    assert_eq!(found_in(b"LEAK probe 1a2b3c4d5e, \"probe\"", m), None);
}

/// A daemon that is not the hub has no export to ask for: the command line finds that out (the
/// hub-only method is not served there) and exports the machine's own files, saying why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_line_export_beside_a_daemon_that_is_not_a_hub_reads_its_files() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .args(data_args(&data))
        .args(serve_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
    let out = d.path().join("host.ndjson.gz");
    let o = tokio::task::spawn_blocking({
        let mut cmd = std::process::Command::new(agent());
        cmd.args(["diagnostics", "export"])
            .args(data_args(&data))
            .arg("--out")
            .arg(&out);
        move || cmd.output().unwrap()
    })
    .await
    .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(
        marsad_agent::hub::export::gzip::gunzip(&std::fs::read(&out).unwrap()).unwrap(),
    )
    .unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["mode"], "local_files");
    assert_eq!(lines[0]["note"], "not_a_hub");
    let i = lines
        .iter()
        .position(|l| l["line"] == "section" && l["source"] == "local" && l["file"] == "daemon")
        .expect("the daemon's own log");
    assert!(lines[i + 1..]
        .iter()
        .take_while(|l| l.get("line").is_none())
        .any(|r| r["ev"] == "daemon.start" && r["hub"] == false));
    stop(&data).await;
}
