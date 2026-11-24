//! `host.exec.*`: a one-off shell command run in a directory on a host, with its output streamed.
//!
//! `host.exec.start` returns at once with an [`ExecStarted::exec_id`]. The command's output and
//! its end are read from `host.exec:<exec_id>` ([`crate::topics::exec_output`]): output chunks,
//! then exactly one [`ExecEvent::Exited`], after which the stream ends. The command runs in the
//! host's POSIX `sh -c` (Windows: PowerShell) and its whole process group is ended by
//! `host.exec.cancel`, or by the host when it has run for an hour ([`ExecEndReason::TimeLimit`]).

use serde::{Deserialize, Serialize};

/// `host.exec.start`. `cwd` must be an existing directory on the host (`~` is expanded); `line`
/// is the command line, run by the host's shell (not split by the client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExecStartRequest {
    pub cwd: String,
    pub line: String,
}

/// Result of `host.exec.start`: names the execution for `host.exec:<id>` and `host.exec.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExecStarted {
    pub exec_id: String,
}

/// Names one execution (`host.exec.cancel`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExecRef {
    pub exec_id: String,
}

/// Which of the command's output streams a chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExecStream {
    Stdout,
    Stderr,
}

/// Why the host ended a command it did not see finish on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExecEndReason {
    /// It ran for the host's time limit (one hour).
    TimeLimit,
    /// `host.exec.cancel` ended it (or the daemon shut down).
    Cancelled,
}

/// One event of `host.exec:<exec_id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecEvent {
    /// A chunk of output, at most 8 KiB, decoded as UTF-8 (invalid bytes become U+FFFD). Chunks
    /// of one stream arrive in order. Output beyond the host's retained cap is not sent; the
    /// `exited` event says so.
    Output { stream: ExecStream, text: String },
    /// The command has ended and all its output has been sent. The last event of the stream.
    Exited {
        /// The exit code; `None` when it was ended by a signal or its status was unknown.
        code: Option<i32>,
        /// The signal that ended it (Unix), if any.
        signal: Option<i32>,
        /// Output was produced beyond the retained cap (1 MiB across both streams). The command
        /// still ran to its end; only the excess was dropped.
        output_truncated: bool,
        /// Bytes of output produced by the command, across both streams, including the dropped.
        output_bytes: u64,
        /// Set when the host ended the command rather than the command ending itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<ExecEndReason>,
    },
}
