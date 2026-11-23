//! The app log, for real: the sink writes a temp data directory's `diag/app.ndjson`, the
//! webview's events go through `diag_log`'s core, and a traced `rpc` reaches the wire.
//!
//! One global sink per process: the tests share it (installed once, through the shell's own
//! `diag::start`) and each reads back only the records that name its own ids. Nothing waits
//! for time to pass: a test reads the file after the sink's own flush.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use marsad_client::{ClientConfig, MarsadClient};
use marsad_desktop_lib::{bridge, diag};
use serde_json::{json, Value};
use wasit_control::demo::DemoService;
use wasit_control::{ConnConfig, ControlServer, Hello, TraceId};

const FLUSH: Duration = Duration::from_secs(20);
/// Free text a webview might send. None of it may be found in any file.
const MARK: &str = "FREE TEXT MARK /home/u/secret";

/// A scratch data directory, and the app log started in it (once for the process).
fn data() -> &'static PathBuf {
    static DATA: OnceLock<PathBuf> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mrsd-applog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Developer mode keeps the previous panic hook chained, so that a failing assertion
        // still prints its message (the test harness relies on the default hook for that).
        std::env::set_var(diag::DEV_ENV, "off");
        diag::start(dir.join("diag"));
        dir
    })
}

/// Every record written so far, and the raw file.
fn records() -> (Vec<Value>, String) {
    let dir = data();
    assert!(wasit_diag::flush(FLUSH), "the sink did not flush");
    let raw = std::fs::read_to_string(dir.join("diag").join(diag::APP_FILE)).unwrap();
    let all = raw
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("every line is JSON"))
        .collect();
    (all, raw)
}

fn evs<'a>(records: &'a [Value], ev: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["ev"] == ev).collect()
}

fn prompt(trace: &str) -> Value {
    json!({"ev":"ui.prompt","trace":trace,"host":"local","session":"s-1","kind":"prompt",
           "chars":12,"attachments":1,"bytes":2048})
}

#[test]
fn the_app_writes_its_start_and_nothing_free_form() {
    let (records, raw) = records();
    // A header, then who and what started; the file is the app's (`src` app, role app).
    let header = &records[0];
    assert_eq!(header["diag_file"], 1);
    assert_eq!(header["src"], "app");
    assert!(header["build"]
        .as_str()
        .unwrap()
        .starts_with("marsad-desktop-"));
    // Starting recorded the process and the app, in that order and nothing else (the tests
    // that follow share the file, so only its first records are the start's).
    assert_eq!(records[1]["ev"], "process.start", "{raw}");
    let start = &records[2];
    assert_eq!(start["ev"], "app.start", "{raw}");
    assert_eq!(evs(&records, "process.start").len(), 1);
    assert_eq!(evs(&records, "app.start").len(), 1);
    assert_eq!(start["src"], "app");
    assert_eq!(start["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(start["os"], std::env::consts::OS);
    // Owner-only on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = data().join("diag");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(diag::APP_FILE)), 0o600);
    }
}

#[test]
fn the_webviews_events_are_recorded_as_given_and_free_text_never_is() {
    data();
    let t = "u-aaaaaaaaaaaaaaaa";
    diag::log_ui(vec![
        prompt(t),
        json!({"ev":"ui.rpc","trace":t,"method":"session.command","ms":12,"outcome":"error","code":"timeout"}),
        json!({"ev":"ui.render_error","component":"Transcript"}),
        json!({"ev":"ui.developer_mode","on":true}),
    ])
    .unwrap();

    // Every string position of every event, one at a time, carries the marker.
    let events = [
        prompt(t),
        json!({"ev":"ui.upload","trace":t,"files":2,"bytes":4096,"ms":30,"outcome":"error","code":"too_large"}),
        json!({"ev":"ui.command","trace":t,"host":"local","session":"s-1","command":"interrupt"}),
        json!({"ev":"ui.launch","trace":t,"host":"gpu","driver":"claude","resume":"fresh","first_prompt":true,"attachments":0}),
        json!({"ev":"ui.rpc","trace":t,"method":"session.command","ms":12,"outcome":"ok","code":"x"}),
        json!({"ev":"ui.render_error","component":"Transcript"}),
        json!({"ev":"ui.developer_mode","on":true}),
        json!({"ev":"ui.export","stage":"failed","code":"io"}),
    ];
    let mut sent = 0;
    for base in &events {
        for key in base.as_object().unwrap().keys().filter(|k| *k != "ev") {
            let mut v = base.clone();
            v[key] = json!(MARK);
            // Accepted (a word that is not a word is kept as its length) or refused: either
            // way the call answers, and below no file has the marker.
            let _ = diag::log_ui(vec![v]);
            sent += 1;
        }
        // An unknown field is refused, not dropped silently.
        let mut v = base.clone();
        v["note"] = json!(MARK);
        assert!(diag::log_ui(vec![v]).is_err());
    }
    // Unknown events, among them the shell's own, are refused.
    for ev in ["app.start", "app.hub_link", "app.failed", MARK] {
        assert!(diag::log_ui(vec![json!({ "ev": ev })]).is_err(), "{ev}");
    }
    assert!(diag::log_ui(vec![json!(MARK)]).is_err());
    assert!(sent > 25);

    let (records, raw) = records();
    for needle in ["FREE TEXT", "MARK", "secret", "/home/u"] {
        assert!(
            !raw.contains(needle),
            "{needle} reached the app log:\n{raw}"
        );
    }
    // What was accepted is there, as given, stamped by the shell.
    let ours = |ev: &str| -> Vec<&Value> {
        evs(&records, ev)
            .into_iter()
            .filter(|r| r["trace"] == t)
            .collect()
    };
    let p = ours("ui.prompt");
    assert!(!p.is_empty());
    for (k, v) in prompt(t).as_object().unwrap() {
        assert_eq!(&p[0][k], v, "{k}");
    }
    assert_eq!(p[0]["src"], "app");
    assert_eq!(ours("ui.rpc")[0]["code"], "timeout");
    assert_eq!(
        evs(&records, "ui.render_error")[0]["component"],
        "Transcript"
    );
    assert_eq!(evs(&records, "ui.developer_mode")[0]["on"], true);
    // A word that is not a word is kept as its length only.
    assert!(records.iter().any(|r| r["host"] == "<other:29>"), "{raw}");
    // The refusals are on record (a code and a length, never what was sent).
    let failed = evs(&records, "app.failed");
    assert!(failed
        .iter()
        .any(|r| r["op"] == "diag_log" && r["error"]["code"] == "unknown_field"));
    assert!(failed
        .iter()
        .any(|r| r["op"] == "diag_log" && r["error"]["code"] == "unknown_event"));
    assert!(failed.iter().all(|r| r["error"].get("text").is_none()));
}

#[test]
fn a_batch_is_bounded() {
    data();
    let t = "u-bbbbbbbbbbbbbbbb";
    let batch = |n: usize| (0..n).map(|_| prompt(t)).collect::<Vec<_>>();
    diag::log_ui(batch(diag::MAX_BATCH)).unwrap();
    let e = diag::log_ui(batch(diag::MAX_BATCH + 1)).unwrap_err();
    assert_eq!(e.code, "invalid_params");
    let (records, _) = records();
    // The accepted batch is recorded (rate budgets may thin a burst, never invent records); the
    // refused one stored nothing.
    let n = evs(&records, "ui.prompt")
        .iter()
        .filter(|r| r["trace"] == t)
        .count();
    assert!(n <= diag::MAX_BATCH, "{n}");
    assert!(evs(&records, "app.failed")
        .iter()
        .any(|r| r["op"] == "diag_log" && r["error"]["len"].is_number()));
}

#[tokio::test]
async fn a_trace_given_to_rpc_reaches_the_server_that_answers() {
    let dir = data().clone();
    let sock = dir.join("c.sock");
    let server = ControlServer::new(
        Arc::new(DemoService::new("local")),
        Hello::new("test", "local", "daemon"),
        ConnConfig::default(),
    );
    let listener = wasit_control::bind_local_exclusive(&sock).await.unwrap();
    tokio::spawn(async move { server.serve(listener).await });

    let mut cfg = ClientConfig::new(&dir, Hello::new("test", "marsad-desktop", "gui"));
    cfg.socket = Some(sock);
    cfg.start_daemon = false;
    let client = MarsadClient::connect(cfg);

    let trace = "u-0123456789abcdef";
    let id = TraceId::new(trace).unwrap();
    let answer = bridge::forward(&client, "echo", json!({"n": 1}), None, Some(id))
        .await
        .unwrap();
    assert_eq!(answer, json!({"n": 1}));
    // The same call without a trace records nothing under any trace.
    bridge::forward(&client, "echo", json!({"n": 2}), None, None)
        .await
        .unwrap();

    // The server side of the link (in this process) wrote `rpc.traced` with our trace.
    let (records, _) = records();
    let traced: Vec<&Value> = evs(&records, "rpc.traced")
        .into_iter()
        .filter(|r| r["trace"] == trace)
        .collect();
    assert_eq!(traced.len(), 1, "{records:#?}");
    assert_eq!(traced[0]["method"], "echo");
    assert_eq!(traced[0]["outcome"], "ok");
    assert_eq!(evs(&records, "rpc.traced").len(), 1);
}
