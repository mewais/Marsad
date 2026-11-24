//! `host.exec.*` against a real daemon and real shells: output chunks and exactly one `exited`,
//! the retained-output cap, cancel ending the whole process group, a refused working directory,
//! and the time limit (checked in-process with a short limit, since the daemon's is an hour).
//! Process ends are observed as events (an `exited` event; a fifo reaching end-of-file once every
//! process holding it is gone), never by waiting a while.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use futures::{Stream, StreamExt};
use marsad_agent::host::exec::{Execs, TIME_LIMIT};
use marsad_client::{ClientError, MarsadClient, Subscription};
use marsad_proto::exec::{
    ExecEndReason, ExecEvent, ExecRef, ExecStartRequest, ExecStarted, ExecStream,
};
use marsad_proto::{methods, topics, Ack, MarsadError};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use wasit_control::Cursor;

/// The most output a single chunk may carry.
const CHUNK_MAX: usize = 8 * 1024;
/// The retained output cap, across both streams.
const CAP: usize = 1024 * 1024;

async fn start(c: &MarsadClient, cwd: &Path, line: &str) -> Result<ExecStarted, ClientError> {
    let v = c
        .call_raw(
            None,
            methods::EXEC_START,
            serde_json::to_value(ExecStartRequest {
                cwd: cwd.display().to_string(),
                line: line.into(),
            })
            .unwrap(),
        )
        .await?;
    Ok(serde_json::from_value(v).unwrap())
}

async fn cancel(c: &MarsadClient, exec_id: &str) {
    let v = c
        .call_raw(
            None,
            methods::EXEC_CANCEL,
            serde_json::to_value(ExecRef {
                exec_id: exec_id.into(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::from_value::<Ack>(v).unwrap(), Ack {});
}

/// Everything an execution printed, and its `exited` event.
#[derive(Debug)]
struct Run {
    stdout: String,
    stderr: String,
    exited: ExecEvent,
}

/// Fold events into a [`Run`] until the stream ends. Asserts that each chunk is within the chunk
/// bound, that nothing follows `exited`, and that exactly one `exited` came.
fn fold(
    events: impl IntoIterator<Item = ExecEvent>,
    stdout: &mut String,
    stderr: &mut String,
) -> Option<ExecEvent> {
    let mut exited = None;
    for ev in events {
        assert!(exited.is_none(), "an event after exited: {ev:?}");
        match ev {
            ExecEvent::Output { stream, text } => {
                assert!(
                    !text.is_empty() && text.len() <= CHUNK_MAX,
                    "chunk of {} bytes",
                    text.len()
                );
                match stream {
                    ExecStream::Stdout => stdout.push_str(&text),
                    ExecStream::Stderr => stderr.push_str(&text),
                }
            }
            e @ ExecEvent::Exited { .. } => exited = Some(e),
        }
    }
    exited
}

/// Every event of a daemon execution's stream, until it ends.
async fn run_to_end(out: &mut Subscription<Value>) -> Run {
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut exited = None;
    loop {
        let item = tokio::time::timeout(T, out.next())
            .await
            .expect("the execution's stream went quiet");
        let Some(item) = item else { break };
        let (_, v) = item.expect("the execution's stream failed");
        let ev: ExecEvent = serde_json::from_value(v).unwrap();
        if let Some(e) = fold([ev], &mut stdout, &mut stderr) {
            exited = Some(e);
        }
    }
    Run {
        stdout,
        stderr,
        exited: exited.expect("the stream ended without `exited`"),
    }
}

/// Every event of an in-process execution's stream, until it ends.
async fn run_stream<S, E>(out: &mut S) -> Run
where
    S: Stream<Item = Result<(Cursor, Value), E>> + Unpin,
{
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut exited = None;
    loop {
        let item = tokio::time::timeout(T, out.next())
            .await
            .expect("the execution's stream went quiet");
        let Some(item) = item else { break };
        let Ok((_, v)) = item else {
            panic!("the execution's stream failed")
        };
        let ev: ExecEvent = serde_json::from_value(v).unwrap();
        if let Some(e) = fold([ev], &mut stdout, &mut stderr) {
            exited = Some(e);
        }
    }
    Run {
        stdout,
        stderr,
        exited: exited.expect("the stream ended without `exited`"),
    }
}

/// Stdout until `done` holds of it; the subscription is left after that point.
async fn stdout_until(out: &mut Subscription<Value>, mut done: impl FnMut(&str) -> bool) -> String {
    let mut stdout = String::new();
    let mut stderr = String::new();
    while !done(&stdout) {
        let item = tokio::time::timeout(T, out.next())
            .await
            .expect("the execution printed what was waited for");
        let (_, v) = item
            .expect("the execution's stream ended")
            .expect("the execution's stream failed");
        let ev: ExecEvent = serde_json::from_value(v).unwrap();
        assert!(
            fold([ev], &mut stdout, &mut stderr).is_none(),
            "the execution ended early"
        );
    }
    stdout
}

/// The number printed as `tag=<digits>` at the start of a line.
fn tagged(text: &str, tag: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix(tag)?.strip_prefix('=')?.parse().ok())
}

fn exited_fields(e: &ExecEvent) -> (Option<i32>, Option<i32>, bool, u64, Option<ExecEndReason>) {
    match e {
        ExecEvent::Exited {
            code,
            signal,
            output_truncated,
            output_bytes,
            reason,
        } => (*code, *signal, *output_truncated, *output_bytes, *reason),
        other => panic!("not exited: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_is_streamed_then_exactly_one_exited_and_replayed() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();

    let s = start(&c, &work, "printf 'out\\n'; printf 'err\\n' >&2; exit 3")
        .await
        .unwrap();
    let topic = topics::exec_output(&s.exec_id);
    let mut out = c.subscribe_raw(None, topic.clone(), None);
    let run = run_to_end(&mut out).await;
    assert_eq!(run.stdout, "out\n");
    assert_eq!(run.stderr, "err\n");
    assert_eq!(
        exited_fields(&run.exited),
        (Some(3), None, false, 8, None),
        "{run:?}"
    );

    // A subscriber after the end reads the same output and the same single `exited`.
    let mut later = c.subscribe_raw(None, topic, None);
    let again = run_to_end(&mut later).await;
    assert_eq!((again.stdout, again.stderr), (run.stdout, run.stderr));
    assert_eq!(again.exited, run.exited);
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_job_that_outlives_the_shell_has_its_output_sent_before_exited() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();

    // The shell exits at once; the job still holds its output pipe and writes after that.
    let s = start(&c, &work, "(sleep 1; echo late) & echo first")
        .await
        .unwrap();
    let mut out = c.subscribe_raw(None, topics::exec_output(&s.exec_id), None);
    let run = run_to_end(&mut out).await;
    assert_eq!(run.stdout, "first\nlate\n");
    assert_eq!(
        exited_fields(&run.exited),
        (Some(0), None, false, 11, None),
        "{run:?}"
    );
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_past_the_cap_is_dropped_but_the_command_runs_to_completion() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let marker = work.join("finished");

    let line = format!(
        "head -c 1500000 /dev/zero | tr '\\0' a; printf tail >&2; touch '{}'",
        marker.display()
    );
    let s = start(&c, &work, &line).await.unwrap();
    let mut out = c.subscribe_raw(None, topics::exec_output(&s.exec_id), None);
    let run = run_to_end(&mut out).await;

    // The cap is exact: the first 1 MiB of stdout is kept, and nothing from stderr, which came
    // after the cap was reached.
    assert_eq!(run.stdout.len(), CAP);
    assert!(run.stdout.bytes().all(|b| b == b'a'));
    assert_eq!(run.stderr, "");
    assert_eq!(
        exited_fields(&run.exited),
        (Some(0), None, true, 1_500_004, None),
        "{run:?}"
    );
    assert!(marker.exists(), "the command ran to its end");
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multibyte_characters_split_across_reads_arrive_whole() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();

    // 100000 lines of a 3-byte character and a newline: 400000 bytes, which reads of 8 KiB
    // split inside characters. None may come out as U+FFFD.
    let s = start(&c, &work, "yes '€' | head -n 100000").await.unwrap();
    let mut out = c.subscribe_raw(None, topics::exec_output(&s.exec_id), None);
    let run = run_to_end(&mut out).await;
    assert_eq!(run.stdout, "€\n".repeat(100_000));
    assert_eq!(
        exited_fields(&run.exited),
        (Some(0), None, false, 400_000, None),
        "{run:?}"
    );
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_ends_the_whole_process_group() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();

    // The command's shell starts a child shell, which holds `fifo` open (fd 5) and starts a
    // grandchild that inherits it. The fifo reaches end-of-file exactly when both are gone.
    let fifo = work.join("held.fifo");
    sh(&work, &["mkfifo", fifo.to_str().unwrap()]);
    let line = format!(
        "sh -c 'exec 5>{f}; sleep 600 & echo gc=$!; wait' & echo child=$!; echo ready; wait",
        f = fifo.display()
    );
    let s = start(&c, &work, &line).await.unwrap();
    let mut out = c.subscribe_raw(None, topics::exec_output(&s.exec_id), None);

    // Opening the fifo for reading is what lets the child's `exec 5>` through.
    let mut reader = tokio::time::timeout(T, tokio::fs::File::open(&fifo))
        .await
        .expect("the child opened the fifo")
        .unwrap();
    let text = stdout_until(&mut out, |t| {
        tagged(t, "gc").is_some() && tagged(t, "child").is_some()
    })
    .await;
    assert!(tagged(&text, "child").is_some() && tagged(&text, "gc").is_some());

    cancel(&c, &s.exec_id).await;

    // Both processes holding the fifo are gone: it reaches end-of-file.
    let mut rest = Vec::new();
    tokio::time::timeout(T, reader.read_to_end(&mut rest))
        .await
        .expect("the child and its grandchild ended")
        .unwrap();
    assert!(rest.is_empty());

    let run = run_to_end(&mut out).await;
    let (code, signal, truncated, bytes, reason) = exited_fields(&run.exited);
    assert_eq!((code, signal, truncated), (None, Some(9), false), "{run:?}");
    assert_eq!(reason, Some(ExecEndReason::Cancelled));
    assert!(
        bytes >= text.len() as u64,
        "the output read before the cancel is counted"
    );
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_working_directory_is_refused() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let daemon = own_daemon(&data).await;
    let c = client_of(&data);

    let err = start(&c, &d.path().join("no-such-dir"), "true")
        .await
        .unwrap_err();
    match err {
        ClientError::Marsad(MarsadError::NotFound { .. }) => {}
        other => panic!("expected not_found, got {other:?}"),
    }
    end_own_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_past_the_time_limit_is_ended_with_reason_time_limit() {
    // The limit is configurable so the test need not wait an hour; the daemon's default is
    // checked below.
    let d = TempDir::new();
    let execs = Execs::with_time_limit(Duration::from_millis(200));
    let s = execs
        .start(&d.path().display().to_string(), "echo before; sleep 600")
        .await
        .unwrap();
    let mut out = execs.subscribe(&s.exec_id, None).unwrap();
    let run = run_stream(&mut out).await;
    assert_eq!(run.stdout, "before\n");
    assert_eq!(
        exited_fields(&run.exited),
        (None, Some(9), false, 7, Some(ExecEndReason::TimeLimit)),
        "{run:?}"
    );
}

#[test]
fn the_time_limit_is_one_hour() {
    assert_eq!(TIME_LIMIT, Duration::from_secs(60 * 60));
}
