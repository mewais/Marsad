//! `marsad-agent hold-sessions` against a fake daemon on a real local socket. The fake serves
//! the `host.sessions` topic from a queue the test fills, so each step is one event the test
//! sends and one line of the holder's stdout (`live=<n>`) it waits for: nothing sleeps.
//!
//! Exit statuses: 0 no live session left, 1 feed error, 3 another holder holds the lock, 4 the
//! daemon is gone.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex};
use wasit_control::{
    bind_local_exclusive, CallContext, CloseReason, ConnConfig, ControlServer, Cursor, EventStream,
    Hello, RpcError, Service,
};

use common::{agent, TempDir};

/// The codes of `marsad_agent::hold` (its module documents them).
const NO_LIVE: i32 = 0;
const ERROR: i32 = 1;
const LOCK_HELD: i32 = 3;
const DAEMON_GONE: i32 = 4;

/// One step of the fake feed: a payload, or the end of the subscription with its reason.
enum Step {
    Event(Value),
    End(CloseReason),
}

/// The fake daemon: its `host.sessions` feed reads from `queue`, shared by every subscription.
struct FakeFeed {
    queue: Arc<Mutex<mpsc::UnboundedReceiver<Step>>>,
}

#[async_trait]
impl Service for FakeFeed {
    async fn call(&self, _ctx: CallContext, method: &str, _p: Value) -> Result<Value, RpcError> {
        Err(RpcError::method_not_found(method))
    }

    async fn subscribe(
        &self,
        _ctx: CallContext,
        topic: &str,
        _params: Value,
        _from: Option<Cursor>,
    ) -> Result<EventStream, RpcError> {
        if topic != marsad_proto::runtime::topics::HOST_SESSIONS {
            return Err(RpcError::topic_not_found(topic));
        }
        let queue = self.queue.clone();
        // `done`: the subscription ended with its reason; the stream ends after it.
        let s = stream::unfold((queue, 0u64, false), |(queue, n, done)| async move {
            if done {
                return None;
            }
            let step = queue.lock().await.recv().await?;
            match step {
                Step::Event(v) => {
                    Some((Ok((Cursor::new(n.to_string()), v)), (queue, n + 1, false)))
                }
                Step::End(reason) => Some((Err(reason), (queue, n, true))),
            }
        });
        Ok(s.boxed())
    }
}

/// A running fake daemon at `socket`, fed through `feed`.
struct Daemon {
    feed: mpsc::UnboundedSender<Step>,
    server: Arc<ControlServer>,
    serving: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn start(socket: &Path) -> Daemon {
        let listener = bind_local_exclusive(socket)
            .await
            .expect("the fake daemon binds its socket");
        let (feed, rx) = mpsc::unbounded_channel();
        let service = FakeFeed {
            queue: Arc::new(Mutex::new(rx)),
        };
        let server = Arc::new(ControlServer::new(
            Arc::new(service),
            Hello::new("test-build", "fake-daemon", "daemon"),
            ConnConfig::default(),
        ));
        let s = server.clone();
        let serving = tokio::spawn(async move {
            let _ = s.serve(listener).await;
        });
        Daemon {
            feed,
            server,
            serving,
        }
    }

    fn event(&self, v: Value) {
        self.feed.send(Step::Event(v)).expect("the feed is open");
    }

    /// Stops serving: every connection, the holder's included, is closed.
    async fn go_away(self) {
        self.server.shutdown(Duration::from_secs(2)).await;
        let _ = self.serving.await;
    }
}

/// A layout of its own: data root, runtime directory (which holds the socket and the lock).
struct Home {
    _dir: TempDir,
    data: PathBuf,
    run: PathBuf,
}

impl Home {
    fn new() -> Home {
        let dir = TempDir::new();
        let data = dir.path().join("data");
        let run = dir.path().join("run");
        // The runtime directory the way a daemon makes it (private, owner-checked).
        marsad_proto::layout::Layout::resolve(&data, Some(run.clone()))
            .expect("the runtime directory");
        Home {
            _dir: dir,
            data,
            run,
        }
    }

    fn socket(&self) -> PathBuf {
        self.run.join("marsad.sock")
    }

    /// Starts a holder against this layout, its stdout piped.
    fn holder(&self) -> Holder {
        let mut child = Command::new(agent())
            .args(["hold-sessions", "--data"])
            .arg(&self.data)
            .arg("--run-dir")
            .arg(&self.run)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the holder starts");
        let lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        Holder { child, lines }
    }
}

/// A running holder: its process and the lines it has printed so far.
struct Holder {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
}

/// Bounds a wait so a broken holder fails the test instead of hanging it. Not a timing
/// assumption: every wait in these tests ends on an event, and this only catches a hang.
const GUARD: Duration = Duration::from_secs(60);

impl Holder {
    /// Waits for the holder to print exactly `want`, and fails on any other line before it.
    async fn expect_line(&mut self, want: &str) {
        let got = tokio::time::timeout(GUARD, self.lines.next_line())
            .await
            .unwrap_or_else(|_| panic!("no line {want:?} from the holder"))
            .expect("stdout reads")
            .unwrap_or_else(|| panic!("the holder ended before printing {want:?}"));
        assert_eq!(got, want);
    }

    /// Waits for the holder to end and returns its exit code.
    async fn exit_code(&mut self) -> i32 {
        let status = tokio::time::timeout(GUARD, self.child.wait())
            .await
            .expect("the holder ends")
            .expect("the holder can be waited for");
        status.code().expect("the holder exits, it is not killed")
    }

    /// The holder printed nothing more (checked once it has ended).
    async fn rest_of_output(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(Ok(Some(l))) = tokio::time::timeout(GUARD, self.lines.next_line()).await {
            out.push(l);
        }
        out
    }
}

fn snapshot(id: &str, state: &str) -> Value {
    json!({"id": id, "phase": {"state": state}, "cwd": "/work/x", "pid": 42})
}

fn reset(sessions: &[(&str, &str)]) -> Step {
    let sessions: Vec<Value> = sessions.iter().map(|(i, s)| snapshot(i, s)).collect();
    Step::Event(json!({"kind": "reset", "sessions": sessions}))
}

fn upsert(id: &str, state: &str) -> Step {
    Step::Event(json!({"kind": "upsert", "session": snapshot(id, state)}))
}

fn removed(id: &str) -> Step {
    Step::Event(json!({"kind": "removed", "id": id}))
}

fn send(daemon: &Daemon, step: Step) {
    match step {
        Step::Event(v) => daemon.event(v),
        end => daemon.feed.send(end).expect("the feed is open"),
    }
}

#[tokio::test]
async fn holds_while_a_session_is_live_and_exits_with_the_last() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    send(&daemon, reset(&[("a", "idle")]));
    holder.expect_line("live=1").await;
    // An ended session does not keep the holder; the live one does.
    send(&daemon, upsert("b", "exited"));
    holder.expect_line("live=1").await;
    send(&daemon, upsert("c", "needs_you"));
    holder.expect_line("live=2").await;
    send(&daemon, removed("a"));
    holder.expect_line("live=1").await;
    // A session whose shim is gone counts for nothing either.
    send(&daemon, upsert("c", "lost"));
    holder.expect_line("live=0").await;
    assert_eq!(holder.exit_code().await, NO_LIVE);
    assert!(holder.rest_of_output().await.is_empty());
    daemon.go_away().await;
}

#[tokio::test]
async fn exits_at_once_when_no_session_is_live() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    send(&daemon, reset(&[("a", "exited"), ("b", "failed")]));
    holder.expect_line("live=0").await;
    assert_eq!(holder.exit_code().await, NO_LIVE);
    daemon.go_away().await;
}

#[tokio::test]
async fn a_second_holder_of_the_same_distribution_exits_at_once() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut first = home.holder();
    send(&daemon, reset(&[("a", "working")]));
    first.expect_line("live=1").await;
    // The second one takes no part in the feed: it exits before it connects, without output.
    let mut second = home.holder();
    assert_eq!(second.exit_code().await, LOCK_HELD);
    assert!(second.rest_of_output().await.is_empty());
    // The first one is unaffected and still holds.
    send(&daemon, removed("a"));
    first.expect_line("live=0").await;
    assert_eq!(first.exit_code().await, NO_LIVE);
    daemon.go_away().await;
}

#[tokio::test]
async fn exits_when_the_daemon_goes_away_while_sessions_are_held() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    send(&daemon, reset(&[("a", "idle")]));
    holder.expect_line("live=1").await;
    daemon.go_away().await;
    assert_eq!(holder.exit_code().await, DAEMON_GONE);
}

#[tokio::test]
async fn exits_at_once_when_no_daemon_is_running() {
    let home = Home::new();
    let mut holder = home.holder();
    assert_eq!(holder.exit_code().await, DAEMON_GONE);
    assert!(holder.rest_of_output().await.is_empty());
}

#[tokio::test]
async fn a_lagged_feed_is_followed_by_a_fresh_snapshot() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    send(&daemon, reset(&[("a", "idle")]));
    holder.expect_line("live=1").await;
    // The feed lagged: the holder resubscribes, and the new snapshot is what it believes now.
    daemon
        .feed
        .send(Step::End(CloseReason::Lagged))
        .expect("the feed is open");
    send(&daemon, reset(&[("b", "working")]));
    holder.expect_line("live=1").await;
    send(&daemon, removed("b"));
    holder.expect_line("live=0").await;
    assert_eq!(holder.exit_code().await, NO_LIVE);
    daemon.go_away().await;
}

#[tokio::test]
async fn a_feed_error_exits_with_failure() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    send(&daemon, reset(&[("a", "idle")]));
    holder.expect_line("live=1").await;
    daemon
        .feed
        .send(Step::End(CloseReason::Error {
            error: RpcError::new("boom", "the feed broke"),
        }))
        .expect("the feed is open");
    assert_eq!(holder.exit_code().await, ERROR);
    daemon.go_away().await;
}

#[tokio::test]
async fn an_event_it_cannot_read_exits_with_failure() {
    let home = Home::new();
    let daemon = Daemon::start(&home.socket()).await;
    let mut holder = home.holder();
    daemon.event(json!({"kind": "not_a_host_event"}));
    assert_eq!(holder.exit_code().await, ERROR);
    assert!(holder.rest_of_output().await.is_empty());
    daemon.go_away().await;
}
