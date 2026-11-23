//! A terminal's lifetime, against a real daemon and real shells: a session has one terminal
//! that clients open (idempotently) and detach from; it ends only when its shell exits, when it
//! is closed, when its session is deleted or when the daemon stops. Shell processes are
//! identified by the PID the shell itself reports (`$$`) and found gone with `kill -0`, after the
//! host's own signal that it reaped them (the reply of `pty.close`, the `exited` event, the
//! daemon's exit): never by searching processes and never by waiting a while.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Stdio;

use base64::Engine;
use common::*;
use marsad_client::{ClientError, MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::git::GitTarget;
use marsad_proto::pty::{PtyEvent, PtyInfo, PtyStartRequest, PtyStarted};
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{MarsadError, SessionId};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use wasit_fake_cli::scenario::Scenario;

const SH: &str = "/bin/sh";

fn req(target: GitTarget) -> PtyStartRequest {
    PtyStartRequest {
        target,
        cols: 80,
        rows: 24,
        shell: Some(SH.into()),
    }
}

async fn open(c: &MarsadClient, target: GitTarget) -> PtyStarted {
    c.pty_start(None, req(target)).await.unwrap()
}

fn bytes(e: &PtyEvent) -> (u64, Vec<u8>) {
    match e {
        PtyEvent::Output { offset, data } => (
            *offset,
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
        ),
        PtyEvent::Exited { .. } => panic!("the terminal exited"),
    }
}

/// Every byte of output up to the offset `end`, from wherever the subscription starts.
async fn read_to(out: &mut Subscription<PtyEvent>, end: u64) -> Vec<u8> {
    let mut all = Vec::new();
    let mut at = None;
    while at.is_none_or(|a| a < end) {
        let (cur, ev) = next(out).await;
        let (off, chunk) = bytes(&ev);
        assert_eq!(
            at.unwrap_or(off),
            off,
            "output arrives contiguously, from the cursor"
        );
        all.extend(chunk);
        at = Some(cur.as_u64().unwrap());
    }
    assert_eq!(at, Some(end));
    all
}

/// Output until `done(text)`; returns the text and the cursor after it.
async fn read_until(
    out: &mut Subscription<PtyEvent>,
    mut done: impl FnMut(&str) -> bool,
) -> (String, u64) {
    let mut text = String::new();
    loop {
        let (cur, ev) = next(out).await;
        let (_, chunk) = bytes(&ev);
        text.push_str(&String::from_utf8_lossy(&chunk));
        if done(&text) {
            return (text, cur.as_u64().unwrap());
        }
    }
}

/// The value printed as `tag=<digits>` and ended by a newline (the typed command line, which
/// holds `tag=$$`, never matches).
fn tagged(text: &str, tag: &str) -> Option<u32> {
    let key = format!("{tag}=");
    text.match_indices(&key).find_map(|(i, _)| {
        let rest = &text[i + key.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let ended = rest[digits.len()..].starts_with(['\r', '\n']);
        (ended && !digits.is_empty()).then(|| digits.parse().unwrap())
    })
}

/// The PID of the shell of terminal `info`, as the shell says it. The shell also ignores
/// hang-ups, so what ends it is the host killing it, not the terminal going away.
async fn shell_pid(c: &MarsadClient, info: &PtyInfo) -> u32 {
    let mut out = c.pty_output(None, &info.id, Some(info.offset));
    c.pty_input(None, &info.id, "trap '' HUP; echo shell=$$\n")
        .await
        .unwrap();
    let mut pid = None;
    read_until(&mut out, |t| {
        pid = tagged(t, "shell");
        pid.is_some()
    })
    .await;
    pid.unwrap()
}

/// Run a foreground program in the terminal's shell that holds `fifo` open for writing, and
/// return the read end once the program has it: reading it reaches end-of-file exactly when the
/// program is gone, which is the signal that it is. Hang-ups do not end it (it ignores them).
async fn run_held(c: &MarsadClient, info: &PtyInfo, fifo: &Path) -> tokio::fs::File {
    sh(fifo.parent().unwrap(), &["mkfifo", fifo.to_str().unwrap()]);
    c.pty_input(None, &info.id, &format!("sleep 600 > {}\n", fifo.display()))
        .await
        .unwrap();
    tokio::time::timeout(T, tokio::fs::File::open(fifo))
        .await
        .expect("the program opened the fifo")
        .unwrap()
}

/// A subscription to the terminal, known to be established: the host has replayed what the
/// terminal already printed (a subscription is made in the background; nothing says it exists
/// until the host's first event arrives).
async fn attached(c: &MarsadClient, info: &PtyInfo) -> Subscription<PtyEvent> {
    let mut out = c.pty_output(None, &info.id, None);
    let (_, first) = next(&mut out).await;
    assert!(matches!(first, PtyEvent::Output { offset: 0, .. }));
    out
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// The next event of an ended stream: the stream is over.
async fn ends(out: &mut Subscription<PtyEvent>) {
    match tokio::time::timeout(T, out.next()).await {
        Ok(None) => {}
        other => panic!(
            "the stream should have ended: {:?}",
            other.map(|o| o.map(|r| r.is_ok()))
        ),
    }
}

fn marsad(e: ClientError) -> MarsadError {
    match e {
        ClientError::Marsad(m) => m,
        other => panic!("not a Marsad error: {other:?}"),
    }
}

fn is_phase(e: &SessionEvent, name: &str) -> bool {
    matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == name)
}

/// A session (the fake agent, idle) working in `work`.
async fn session(c: &MarsadClient, d: &TempDir, work: &Path) -> SessionId {
    std::fs::create_dir_all(work).unwrap();
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &Scenario::builder().build()),
        ..OverrideLayer::default()
    };
    let id = c.launch(req).await.unwrap().id;
    session_reaches(c, &id, "idle").await;
    id
}

async fn session_reaches(c: &MarsadClient, id: &SessionId, phase: &str) {
    let mut sub = c.runtime(None).subscribe_session(id, None);
    loop {
        match tokio::time::timeout(T, sub.next()).await {
            Ok(Some(Ok((_, e)))) if is_phase(&e, phase) => return,
            Ok(Some(Ok(_))) => {}
            other => panic!("waiting for the session to be {phase}: {:?}", other.is_ok()),
        }
    }
}

/// The exit status the stream reports, after any output; the stream then ends.
async fn exits(out: &mut Subscription<PtyEvent>) -> (u64, Option<i32>) {
    loop {
        let (cur, ev) = next(out).await;
        if let PtyEvent::Exited { code } = ev {
            ends(out).await;
            return (cur.as_u64().unwrap(), code);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_sessions_terminal_twice_is_one_shell() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let work = d.path().join("w");
    let id = session(&c, &d, &work).await;
    let target = || GitTarget::session(id.clone());

    // Four opens at once (the tab mounted four times before the first answer): one shell.
    let opens = futures::future::join_all((0..4).map(|_| c.pty_start(None, req(target())))).await;
    let opens: Vec<PtyStarted> = opens.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(opens.iter().filter(|o| o.created).count(), 1, "{opens:?}");
    let first = opens.iter().find(|o| o.created).unwrap().pty.clone();
    assert!(opens.iter().all(|o| o.pty.id == first.id));
    assert_eq!(first.session.as_deref(), Some(id.as_str()));
    assert_eq!(first.cwd, work.to_string_lossy());
    assert!(!first.exited);

    // A later open, from "another visit", is the same terminal and the same shell.
    let again = open(&c, target()).await;
    assert!(!again.created);
    assert_eq!(again.pty.id, first.id);
    let listed = c.pty_list(None).await.unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    let pid = shell_pid(&c, &first).await;
    assert_eq!(shell_pid(&c, &again.pty).await, pid, "one shell process");
    assert!(alive(pid));

    // A directory is not a session: its terminals are their own.
    let a = open(&c, GitTarget::cwd(work.to_string_lossy())).await;
    let b = open(&c, GitTarget::cwd(work.to_string_lossy())).await;
    assert!(a.created && b.created);
    assert!(a.pty.id != b.pty.id && a.pty.session.is_none());
    stop_hub(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_survives_detaching_and_reattaching_by_offset() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let id = session(&c, &d, &d.path().join("w")).await;
    let target = || GitTarget::session(id.clone());
    let info = open(&c, target()).await.pty;

    // The tab is open: one client follows the terminal.
    let mut tab = c.pty_output(None, &info.id, None);
    c.pty_input(None, &info.id, "echo first-$((40+2))\n")
        .await
        .unwrap();
    // What the tab has shown when it is left: everything up to this offset.
    let (_, cursor) = read_until(&mut tab, |t| t.contains("first-42")).await;

    // The user leaves the tab: the subscription goes, the terminal stays. A second client sees
    // what is printed meanwhile, which is how this test knows the host has it, without waiting.
    drop(tab);
    let mut watcher = c.pty_output(None, &info.id, Some(cursor));
    c.pty_input(None, &info.id, "echo second-$((50+5))\n")
        .await
        .unwrap();
    read_until(&mut watcher, |t| t.contains("second-55")).await;
    let before = cursor;

    // Coming back: the same terminal; its whole scrollback replays from the start, and a cursor
    // resumes exactly after the bytes the tab had.
    let back = open(&c, target()).await;
    assert!(!back.created && back.pty.id == info.id);
    let end = back.pty.offset;
    let mut whole = c.pty_output(None, &info.id, None);
    let all = read_to(&mut whole, end).await;
    let text = String::from_utf8_lossy(&all);
    assert!(
        text.contains("first-42") && text.contains("second-55"),
        "{text}"
    );
    let mut resumed = c.pty_output(None, &info.id, Some(before));
    let tail = read_to(&mut resumed, end).await;
    assert_eq!(
        tail,
        all[before as usize..],
        "resumes right after the cursor"
    );
    stop_hub(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_exits_stays_readable_until_replaced_or_closed() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let id = session(&c, &d, &d.path().join("w")).await;
    let target = || GitTarget::session(id.clone());
    let info = open(&c, target()).await.pty;
    let pid = shell_pid(&c, &info).await;

    let mut out = c.pty_output(None, &info.id, None);
    c.pty_input(None, &info.id, "echo bye-$((6*7))\nexit 3\n")
        .await
        .unwrap();
    let mut printed = 0u64;
    let mut text = String::new();
    let exit_cursor = loop {
        let (cur, ev) = next(&mut out).await;
        match ev {
            PtyEvent::Output { data, .. } => {
                text.push_str(&String::from_utf8_lossy(
                    &base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                ));
                printed = cur.as_u64().unwrap();
            }
            PtyEvent::Exited { code } => {
                assert_eq!(code, Some(3));
                break cur.as_u64().unwrap();
            }
        }
    };
    assert!(text.contains("bye-42"), "{text}");
    assert_eq!(exit_cursor, printed + 1, "`exited` follows the last output");
    ends(&mut out).await;
    // The exit is reported after the shell has been reaped.
    assert!(!alive(pid));

    // The entry stays, saying so; nothing more goes in.
    let listed = c.pty_list(None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].exited && listed[0].exit_code == Some(3));
    assert_eq!(listed[0].offset, printed);
    assert!(matches!(
        marsad(c.pty_input(None, &info.id, "x").await.unwrap_err()),
        MarsadError::InvalidRequest { .. }
    ));
    assert!(matches!(
        marsad(c.pty_resize(None, &info.id, 90, 30).await.unwrap_err()),
        MarsadError::InvalidRequest { .. }
    ));

    // A late subscriber reads everything, then the exit, then the stream ends.
    let mut late = c.pty_output(None, &info.id, None);
    let (off, first) = bytes(&next(&mut late).await.1);
    assert_eq!(off, 0);
    assert!(String::from_utf8_lossy(&first).contains("bye-42"));
    assert_eq!(exits(&mut late).await, (exit_cursor, Some(3)));
    // One that had seen all of it, the exit included, has nothing left and is told so.
    let mut done = c.pty_output(None, &info.id, Some(exit_cursor));
    ends(&mut done).await;
    // A cursor from nowhere is refused, not waited on forever.
    let mut bogus = c.pty_output(None, &info.id, Some(exit_cursor + 1));
    assert!(matches!(
        tokio::time::timeout(T, bogus.next()).await,
        Ok(Some(Err(_)))
    ));

    // Opening the session's terminal again replaces the exited one: one slot.
    let fresh = open(&c, target()).await;
    assert!(fresh.created && fresh.pty.id != info.id && !fresh.pty.exited);
    let listed = c.pty_list(None).await.unwrap();
    assert_eq!(
        listed.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        [fresh.pty.id.as_str()]
    );
    assert!(matches!(
        marsad(c.pty_close(None, &info.id).await.unwrap_err()),
        MarsadError::NotFound { .. }
    ));
    // An exited entry is also gone when it is closed.
    let mut out = c.pty_output(None, &fresh.pty.id, None);
    c.pty_input(None, &fresh.pty.id, "exit 0\n").await.unwrap();
    assert_eq!(exits(&mut out).await.1, Some(0));
    c.pty_close(None, &fresh.pty.id).await.unwrap();
    assert!(c.pty_list(None).await.unwrap().is_empty());
    stop_hub(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_ends_the_shell_and_what_it_started() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let id = session(&c, &d, &d.path().join("w")).await;
    let info = open(&c, GitTarget::session(id.clone())).await.pty;
    let pid = shell_pid(&c, &info).await;
    assert!(alive(pid));

    let mut held = run_held(&c, &info, &d.path().join("held")).await;

    let mut out = attached(&c, &info).await;
    c.pty_close(None, &info.id).await.unwrap();
    // The reply came after the shell was reaped.
    assert!(!alive(pid));
    assert!(exits(&mut out).await.1.is_some());
    let mut rest = Vec::new();
    tokio::time::timeout(T, held.read_to_end(&mut rest))
        .await
        .expect("the program the shell started is gone too")
        .unwrap();
    assert!(c.pty_list(None).await.unwrap().is_empty());
    assert!(matches!(
        marsad(c.pty_close(None, &info.id).await.unwrap_err()),
        MarsadError::NotFound { .. }
    ));
    // Open again: a new shell.
    let fresh = open(&c, GitTarget::session(id.clone())).await;
    assert!(fresh.created && fresh.pty.id != info.id);
    assert_ne!(shell_pid(&c, &fresh.pty).await, pid);
    stop_hub(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_session_keeps_its_terminal_and_a_deleted_one_does_not() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let id = session(&c, &d, &d.path().join("w")).await;
    let rt = c.runtime(None);
    let info = open(&c, GitTarget::session(id.clone())).await.pty;
    let pid = shell_pid(&c, &info).await;

    // The agent's process ends, the session stays: the shell is the user's, and still theirs.
    rt.stop(&id).await.unwrap();
    session_reaches(&c, &id, "exited").await;
    let kept = open(&c, GitTarget::session(id.clone())).await;
    assert!(!kept.created && kept.pty.id == info.id);
    assert_eq!(shell_pid(&c, &kept.pty).await, pid);
    assert!(alive(pid));

    // Deleting the session ends its terminal; the host says so on the terminal's own stream.
    let mut out = attached(&c, &info).await;
    rt.delete(&id).await.unwrap();
    let (_, code) = exits(&mut out).await;
    assert!(code.is_some());
    assert!(!alive(pid));
    assert!(c.pty_list(None).await.unwrap().is_empty());
    // And a terminal cannot be opened for a session that is gone.
    assert!(matches!(
        marsad(
            c.pty_start(None, req(GitTarget::session(id.clone())))
                .await
                .unwrap_err()
        ),
        MarsadError::NotFound { .. }
    ));
    assert!(c.pty_list(None).await.unwrap().is_empty());
    stop_hub(&c).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminals_not_tied_to_a_session_are_capped() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let cwd = || GitTarget::cwd(d.path().to_string_lossy());
    let mut made = Vec::new();
    for _ in 0..marsad_agent::host::pty::MAX_DIRECTORY_TERMINALS {
        made.push(open(&c, cwd()).await.pty);
    }
    let refused = marsad(c.pty_start(None, req(cwd())).await.unwrap_err());
    assert!(
        matches!(refused, MarsadError::LimitReached { .. }),
        "{refused:?}"
    );
    assert_eq!(
        c.pty_list(None).await.unwrap().len(),
        marsad_agent::host::pty::MAX_DIRECTORY_TERMINALS
    );
    // Closing one makes room.
    c.pty_close(None, &made[0].id).await.unwrap();
    assert!(open(&c, cwd()).await.created);
    stop_hub(&c).await;
}

/// A daemon this test starts itself (so its exit can be awaited), serving `data`.
async fn own_daemon(data: &Path) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--hub")
        .arg("--data")
        .arg(data)
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
    .expect("the daemon said it is ready")
    .unwrap();
    assert!(line.contains("ready"), "{line}");
    child
}

/// Start a daemon, open a session's terminal and two directory ones, then end the daemon the
/// way `end` says and wait for it to exit: no shell is left.
async fn no_shell_outlives_the_daemon(end: impl AsyncFnOnce(&Path, &tokio::process::Child)) {
    let d = TempDir::new();
    let data = d.path().join("d");
    let mut daemon = own_daemon(&data).await;
    let c = client(&data);
    let id = session(&c, &d, &d.path().join("w")).await;
    let mut shells = Vec::new();
    let mut held = Vec::new();
    for (n, target) in [
        GitTarget::session(id.clone()),
        GitTarget::cwd(d.path().to_string_lossy()),
        GitTarget::cwd(d.path().to_string_lossy()),
    ]
    .into_iter()
    .enumerate()
    {
        let info = open(&c, target).await.pty;
        shells.push(shell_pid(&c, &info).await);
        held.push(run_held(&c, &info, &d.path().join(format!("held{n}"))).await);
    }
    assert!(shells.iter().all(|p| alive(*p)));
    end(&data, &daemon).await;
    let status = tokio::time::timeout(T, daemon.wait())
        .await
        .expect("the daemon exited")
        .unwrap();
    assert!(status.success(), "{status}");
    for pid in shells {
        assert!(!alive(pid), "shell {pid} outlived the daemon");
    }
    for mut f in held {
        let mut rest = Vec::new();
        tokio::time::timeout(T, f.read_to_end(&mut rest))
            .await
            .expect("what the shell ran is gone too")
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminated_daemon_leaves_no_shell() {
    no_shell_outlives_the_daemon(async |_, daemon| stop_pid(daemon.id().unwrap())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_stopped_by_request_leaves_no_shell() {
    no_shell_outlives_the_daemon(async |data, _| {
        stop(data).await;
    })
    .await;
}
