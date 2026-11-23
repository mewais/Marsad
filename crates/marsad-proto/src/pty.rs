//! `pty.*`: an interactive shell in a session's (or any) directory.
//!
//! A session has at most one terminal, identified by the session: `pty.start` for a session
//! that already has a live one returns it ([`PtyStarted::created`] is `false`) instead of
//! starting another. A terminal ends when its shell exits, when it is closed (`pty.close`),
//! when its session is deleted or when the daemon stops; an ended session keeps its terminal.
//! After the shell has exited the entry stays (scrollback and exit status readable, replayable
//! on `pty.output:<id>`) until it is closed or replaced by a new terminal for the session.

use serde::{Deserialize, Serialize};

use crate::git::GitTarget;

/// `pty.start`. `target` names the working directory like a git request. For a session target
/// this is "open the session's terminal": a live one is returned as it is (`cols`, `rows` and
/// `shell` then change nothing; send `pty.resize`), an exited one is replaced by a new shell,
/// and a missing one is started. A directory target always starts a new terminal (the host
/// allows a bounded number of them; the refusal is the `limit_reached` error).
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

/// Result of `pty.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PtyStarted {
    pub pty: PtyInfo,
    /// `true` when this call started the shell, `false` when it returned the session's live
    /// terminal that already existed.
    pub created: bool,
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
    /// The shell has exited. The entry stays readable until it is closed or replaced.
    pub exited: bool,
    /// The shell's exit status once it has exited, when the host could read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
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

/// Payload of `pty.output:<id>`. The cursor of each output event is the byte offset just after
/// it; `exited` has the cursor after the last output byte plus one. Resubscribing with the last
/// cursor replays from the bounded scrollback; if the cursor is older than the scrollback, the
/// first `output` starts later than asked (`offset` says where). A subscriber that attaches
/// after the shell exited reads the scrollback and then `exited`, and the stream ends; one whose
/// cursor is already past `exited` gets an empty stream that ends at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PtyEvent {
    /// Raw terminal bytes, base64 (standard alphabet) starting at byte `offset`.
    Output { offset: u64, data: String },
    /// The shell exited (the last event of the stream).
    Exited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
}
