//! `git.*`: working-tree changes of a session's (or any) directory, diffs, commit, discard.

use serde::{Deserialize, Serialize};
use wasit_runtime::wire::SessionId;

/// Which directory a git request is about: a session's working directory (resolved by the
/// daemon hosting the session) or an explicit path. Exactly one must be set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl GitTarget {
    pub fn session(id: SessionId) -> Self {
        GitTarget {
            session: Some(id),
            cwd: None,
        }
    }
    pub fn cwd(path: impl Into<String>) -> Self {
        GitTarget {
            session: None,
            cwd: Some(path.into()),
        }
    }
}

/// `git.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitStatusRequest {
    pub target: GitTarget,
}

/// How one side (index or worktree) of a path changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

/// One changed path. Paths are relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitFileChange {
    pub path: String,
    /// The source path of a rename / copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,
    /// Change staged in the index (`None` = nothing staged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<ChangeKind>,
    /// Change in the worktree not yet staged (`None` = none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unstaged: Option<ChangeKind>,
    pub staged_added: u32,
    pub staged_removed: u32,
    /// For untracked files: the file's line count (files over 1 MiB count as 0).
    pub unstaged_added: u32,
    pub unstaged_removed: u32,
    pub binary: bool,
}

/// Result of `git.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitStatus {
    pub repo_root: String,
    /// Current branch (`None` = detached HEAD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// HEAD commit (`None` = no commit yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFileChange>,
}

/// Which side of a change a diff shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Index vs HEAD.
    Staged,
    /// Worktree vs index (an untracked file: the whole file as added).
    #[default]
    Unstaged,
}

/// `git.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitDiffRequest {
    pub target: GitTarget,
    /// Path relative to the repository root (as in [`GitFileChange::path`]).
    pub path: String,
    #[serde(default)]
    pub side: DiffSide,
    /// Bound on the returned text (default and maximum 1 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u32>,
}

/// Result of `git.diff`: unified diff text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitDiff {
    pub path: String,
    pub side: DiffSide,
    pub text: String,
    /// The diff was longer than the bound and was cut at a line boundary.
    pub truncated: bool,
    pub binary: bool,
}

/// `git.commit`: stage exactly `files` (paths relative to the repository root) and commit
/// only them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitCommitRequest {
    pub target: GitTarget,
    pub message: String,
    pub files: Vec<String>,
}

/// Result of `git.commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitCommitReply {
    pub commit: String,
}

/// `git.discard`: drop all changes (staged and unstaged) of `files`; untracked files are
/// deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitDiscardRequest {
    pub target: GitTarget,
    pub files: Vec<String>,
}
