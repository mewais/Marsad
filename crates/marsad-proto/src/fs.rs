//! `fs.*`: browsing directories for a project picker.

use serde::{Deserialize, Serialize};

/// `fs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsListRequest {
    /// Absolute path, or `~` / `~/…`.
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
    /// Only directories (the usual project-picker view).
    #[serde(default)]
    pub dirs_only: bool,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// A directory containing `.git`.
    pub is_git_repo: bool,
}

/// Result of `fs.list` (sorted: directories first, then by name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsListing {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    /// More entries existed than the bound (5000) allows.
    pub truncated: bool,
}

/// Result of `fs.home`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsHome {
    pub path: String,
}

/// `fs.exists`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsPathRequest {
    pub path: String,
}

/// Result of `fs.exists`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FsExists {
    /// The path after `~` expansion.
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
}
