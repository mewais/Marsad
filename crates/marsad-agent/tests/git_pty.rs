//! `git.*` on a temp repository and `pty.*` (echo + scrollback replay), through the client.

mod common;

use base64::Engine;
use common::*;
use marsad_proto::git::{ChangeKind, DiffSide, GitTarget};
use marsad_proto::pty::{PtyEvent, PtyStartRequest};
use marsad_proto::HostId;

/// Exercise git status/diff/commit/discard against `repo` on `host`.
pub async fn git_roundtrip(
    c: &marsad_client::MarsadClient,
    host: Option<&HostId>,
    repo: &std::path::Path,
) {
    std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::write(repo.join("new.txt"), "n1\nn2\n").unwrap();
    std::fs::write(repo.join("b.txt"), "changed\n").unwrap();
    sh(repo, &["git", "add", "b.txt"]);
    let t = || GitTarget::cwd(repo.to_string_lossy());
    let st = c.git_status(host, t()).await.unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    let f = |p: &str| st.files.iter().find(|f| f.path == p).unwrap().clone();
    let a = f("a.txt");
    assert_eq!(
        (a.unstaged, a.unstaged_added, a.unstaged_removed),
        (Some(ChangeKind::Modified), 2, 1)
    );
    let n = f("new.txt");
    assert_eq!(
        (n.unstaged, n.unstaged_added),
        (Some(ChangeKind::Untracked), 2)
    );
    let b = f("b.txt");
    assert_eq!(
        (b.staged, b.staged_added, b.staged_removed),
        (Some(ChangeKind::Modified), 1, 1)
    );

    let d = c
        .git_diff(host, t(), "a.txt", DiffSide::Unstaged, None)
        .await
        .unwrap();
    assert!(
        d.text.contains("-two") && d.text.contains("+TWO") && !d.truncated,
        "{}",
        d.text
    );
    let d = c
        .git_diff(host, t(), "a.txt", DiffSide::Unstaged, Some(40))
        .await
        .unwrap();
    assert!(d.truncated && d.text.len() <= 40);
    let d = c
        .git_diff(host, t(), "new.txt", DiffSide::Unstaged, None)
        .await
        .unwrap();
    assert!(d.text.contains("+n1"), "{}", d.text);
    let d = c
        .git_diff(host, t(), "b.txt", DiffSide::Staged, None)
        .await
        .unwrap();
    assert!(d.text.contains("+changed"));
    let err = c
        .git_diff(host, t(), "../etc/passwd", DiffSide::Unstaged, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        marsad_client::ClientError::Marsad(marsad_proto::MarsadError::InvalidRequest { .. })
    ));

    // Commit only the selected files (the staged b.txt stays staged, uncommitted).
    let r = c
        .git_commit(
            host,
            t(),
            "add new, change a",
            vec!["a.txt".into(), "new.txt".into()],
        )
        .await
        .unwrap();
    assert_eq!(sh(repo, &["git", "rev-parse", "HEAD"]).trim(), r.commit);
    let st = c.git_status(host, t()).await.unwrap();
    let paths: Vec<&str> = st.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["b.txt"]);

    // Discard: a staged modification and an untracked file.
    std::fs::write(repo.join("junk.txt"), "x").unwrap();
    c.git_discard(host, t(), vec!["b.txt".into(), "junk.txt".into()])
        .await
        .unwrap();
    assert!(c.git_status(host, t()).await.unwrap().files.is_empty());
    assert_eq!(
        std::fs::read_to_string(repo.join("b.txt")).unwrap(),
        "keep\n"
    );
    assert!(!repo.join("junk.txt").exists());

    let err = c
        .git_status(
            host,
            GitTarget::cwd(repo.parent().unwrap().to_string_lossy()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            marsad_client::ClientError::Marsad(marsad_proto::MarsadError::NotARepo { .. })
        ),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_on_a_local_repo() {
    let d = TempDir::new();
    let repo = d.path().join("repo");
    git_repo(&repo);
    let c = client(&d.path().join("d"));
    git_roundtrip(&c, None, &repo).await;
    let pid = c.daemon_info(None).await.unwrap().pid;
    stop_pid(pid);
}

// Commands whose typed text does not contain what they print, so the echo of the input is
// never mistaken for the output.
#[cfg(unix)]
const SHELL: &str = "/bin/sh";
#[cfg(unix)]
const ECHO_42: &str = "echo marsad-$((40+2))";
#[cfg(unix)]
const ECHO_2: &str = "echo after-$((1+1))";
#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(windows)]
const ECHO_42: &str = "echo marsad-4^2";
#[cfg(windows)]
const ECHO_2: &str = "echo after-^2";

/// A typed line, ended the way the platform's terminal ends one (Enter).
fn line(cmd: &str) -> String {
    if cfg!(windows) {
        format!("{cmd}\r")
    } else {
        format!("{cmd}\n")
    }
}

/// The next output event; on a timeout, fail with what the terminal printed so far.
async fn next_or_dump(
    s: &mut marsad_client::Subscription<PtyEvent>,
    text: &str,
) -> (wasit_control::Cursor, PtyEvent) {
    match tokio::time::timeout(std::time::Duration::from_secs(30), s.next()).await {
        Ok(Some(Ok(x))) => x,
        other => panic!(
            "no terminal output ({}); printed so far: {text:?}",
            match other {
                Ok(Some(Err(e))) => e.to_string(),
                Ok(None) => "ended".into(),
                _ => "timed out".into(),
            }
        ),
    }
}

fn decode(e: &PtyEvent) -> (u64, String) {
    match e {
        PtyEvent::Output { offset, data } => (
            *offset,
            String::from_utf8_lossy(
                &base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap(),
            )
            .into_owned(),
        ),
        PtyEvent::Exited { .. } => panic!("exited"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_echo_and_scrollback_replay() {
    let d = TempDir::new();
    let c = client(&d.path().join("d"));
    let info = c
        .pty_start(
            None,
            PtyStartRequest {
                target: GitTarget::cwd(d.path().to_string_lossy()),
                cols: 80,
                rows: 24,
                shell: Some(SHELL.into()),
            },
        )
        .await
        .unwrap();
    let mut out = c.pty_output(None, &info.id, None);
    c.pty_input(None, &info.id, &line(ECHO_42)).await.unwrap();
    let mut text = String::new();
    let mut last = 0u64;
    while !text.contains("marsad-42") {
        let (cur, ev) = next_or_dump(&mut out, &text).await;
        let (off, chunk) = decode(&ev);
        assert_eq!(off, last, "contiguous output");
        last = cur.as_u64().unwrap();
        // Answer a cursor-position query like a terminal does (ConPTY asks before it draws).
        if chunk.contains("\x1b[6n") {
            c.pty_input(None, &info.id, "\x1b[1;1R").await.unwrap();
        }
        text.push_str(&chunk);
    }
    drop(out);

    // "Reopening the tab": a fresh subscription replays the scrollback from the start…
    let mut again = c.pty_output(None, &info.id, None);
    let (_, ev) = next(&mut again).await;
    let (off, replay) = decode(&ev);
    assert_eq!(off, 0);
    assert!(replay.contains("marsad-42"));
    // …and one resuming from a cursor gets only what came after it.
    c.pty_input(None, &info.id, &line(ECHO_2)).await.unwrap();
    let mut resumed = c.pty_output(None, &info.id, Some(last));
    let mut text = String::new();
    while !text.contains("after-2") {
        let (_, ev) = next_or_dump(&mut resumed, &text).await;
        let (off, chunk) = decode(&ev);
        assert!(off >= last);
        text.push_str(&chunk);
    }
    assert!(!text.contains("marsad-42"));
    // Resized after the cursor checks: ConPTY repaints the whole screen on a resize.
    c.pty_resize(None, &info.id, 100, 30).await.unwrap();
    let listed = c.pty_list(None).await.unwrap();
    assert_eq!((listed[0].cols, listed[0].rows), (100, 30));

    c.pty_input(None, &info.id, &line("exit 3")).await.unwrap();
    loop {
        match next(&mut again).await.1 {
            PtyEvent::Exited { code } => {
                assert_eq!(code, Some(3));
                break;
            }
            PtyEvent::Output { .. } => {}
        }
    }
    c.pty_close(None, &info.id).await.unwrap();
    assert!(c.pty_list(None).await.unwrap().is_empty());
    let pid = c.daemon_info(None).await.unwrap().pid;
    stop_pid(pid);
}
