//! `pty.*`: an interactive shell in a session's (or any) directory.

use serde::{Deserialize, Serialize};

use crate::git::GitTarget;

/// `pty.start`. `target` names the working directory like a git request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyStartRequest {
    pub target: GitTarget,
    pub cols: u16,
    pub rows: u16,
    /// Shell to run (`None` = the user's shell: `$SHELL`, else `/bin/sh`; `cmd.exe` on Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}

/// One terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyInfo {
    pub id: String,
    pub cwd: String,
    /// The session the terminal was opened for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Bytes of output produced so far (the cursor of the latest output).
    pub offset: u64,
    /// Exit code once the shell has exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited: Option<i32>,
}

/// Result of `pty.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyList {
    pub ptys: Vec<PtyInfo>,
}

/// Names one terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyRef {
    pub id: String,
}

/// `pty.input`: keystrokes (UTF-8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyInputRequest {
    pub id: String,
    pub data: String,
}

/// `pty.resize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyResizeRequest {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

/// Payload of `pty.output:<id>`. The cursor of each event is the byte offset just after it.
/// Resubscribing with the last cursor replays from the bounded scrollback; if the cursor is
/// older than the scrollback, the first `output` starts later than asked (`offset` says where).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PtyEvent {
    /// Raw terminal bytes, base64 (standard alphabet) starting at byte `offset`.
    Output { offset: u64, data: String },
    /// The shell exited (the last event).
    Exited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
}
