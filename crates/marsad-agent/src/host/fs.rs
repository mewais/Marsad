//! `fs.*`: directory listing for a project picker (bounded, under a deadline).

use std::path::Path;
use std::time::Duration;

use marsad_proto::fs::{FsEntry, FsExists, FsListing};
use marsad_proto::MarsadError;

use crate::util::expand_tilde;

const MAX_ENTRIES: usize = 5000;
const DEADLINE: Duration = Duration::from_secs(15);

pub async fn list(path: &str, show_hidden: bool, dirs_only: bool) -> Result<FsListing, MarsadError> {
    let dir = expand_tilde(path)?;
    if !dir.is_absolute() {
        return Err(MarsadError::invalid(format!("{path:?} is not an absolute path")));
    }
    let work = async {
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => MarsadError::NotFound {
                    what: dir.display().to_string(),
                },
                _ => MarsadError::io(format!("{}: {e}", dir.display())),
            })?;
        let mut entries = Vec::new();
        let mut truncated = false;
        while let Some(ent) = rd
            .next_entry()
            .await
            .map_err(|e| MarsadError::io(format!("{}: {e}", dir.display())))?
        {
            let name = ent.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let p = ent.path();
            let is_symlink = ent.file_type().await.map(|t| t.is_symlink()).unwrap_or(false);
            // Follow symlinks for the directory test (a linked project is still a project).
            let is_dir = tokio::fs::metadata(&p).await.map(|m| m.is_dir()).unwrap_or(false);
            if dirs_only && !is_dir {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let is_git_repo = is_dir && tokio::fs::try_exists(p.join(".git")).await.unwrap_or(false);
            entries.push(FsEntry {
                name,
                path: p.display().to_string(),
                is_dir,
                is_symlink,
                is_git_repo,
            });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(FsListing {
            path: dir.display().to_string(),
            parent: dir.parent().map(|p| p.display().to_string()),
            entries,
            truncated,
        })
    };
    tokio::time::timeout(DEADLINE, work)
        .await
        .map_err(|_| MarsadError::Timeout {
            what: format!("listing {}", dir.display()),
        })?
}

pub async fn exists(path: &str) -> Result<FsExists, MarsadError> {
    let p = expand_tilde(path)?;
    let meta = tokio::time::timeout(DEADLINE, tokio::fs::metadata(&p))
        .await
        .map_err(|_| MarsadError::Timeout {
            what: format!("checking {}", p.display()),
        })?;
    Ok(FsExists {
        path: p.display().to_string(),
        exists: meta.is_ok(),
        is_dir: meta.map(|m| m.is_dir()).unwrap_or(false),
    })
}

pub fn home() -> Result<String, MarsadError> {
    crate::util::home_dir()
        .map(|h| h.display().to_string())
        .ok_or_else(|| MarsadError::NotFound {
            what: "home directory".into(),
        })
}

/// `true` for `/`-rooted (or drive-rooted) paths.
pub fn is_absolute(p: &str) -> bool {
    Path::new(p).is_absolute()
}
