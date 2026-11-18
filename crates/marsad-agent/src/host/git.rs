//! `git.*` with the `git` binary (tokio::process, every call under a deadline).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use marsad_proto::git::{ChangeKind, DiffSide, GitCommitReply, GitDiff, GitFileChange, GitStatus};
use marsad_proto::MarsadError;

use crate::util::{run, Output};

const DEADLINE: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
/// Largest diff returned.
pub const MAX_DIFF: usize = 1024 * 1024;
/// Untracked files larger than this are not line-counted.
const MAX_COUNT_FILE: u64 = 1024 * 1024;

async fn git(dir: &Path, args: &[&str]) -> Result<Output, MarsadError> {
    let mut argv = vec![
        "git".to_string(),
        "-c".into(),
        "core.quotepath=off".into(),
        "-c".into(),
        "color.ui=never".into(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    // Never prompt (credentials, editors): the daemon has no terminal.
    let env = vec![
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GIT_EDITOR".to_string(), "true".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
    ];
    run(&argv, Some(dir), &env, None, DEADLINE, MAX_OUTPUT).await
}

fn failed(what: &str, out: &Output) -> MarsadError {
    MarsadError::GitFailed {
        message: format!("git {what} failed (exit {:?})", out.code),
        stderr: out.stderr.trim().to_string(),
    }
}

/// The repository root containing `dir`.
pub async fn repo_root(dir: &Path) -> Result<PathBuf, MarsadError> {
    if !tokio::fs::metadata(dir)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Err(MarsadError::NotFound {
            what: format!("directory {}", dir.display()),
        });
    }
    let out = git(dir, &["rev-parse", "--show-toplevel"]).await?;
    if !out.success() {
        return Err(MarsadError::NotARepo {
            path: dir.display().to_string(),
        });
    }
    Ok(PathBuf::from(out.stdout_str().trim()))
}

fn kind(c: u8) -> Option<ChangeKind> {
    Some(match c {
        b'M' => ChangeKind::Modified,
        b'A' => ChangeKind::Added,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        b'T' => ChangeKind::TypeChanged,
        b'U' => ChangeKind::Unmerged,
        _ => return None,
    })
}

/// Parse `git status --porcelain=v2 -z --branch`.
pub fn parse_status(raw: &[u8]) -> (GitStatus, Vec<String>) {
    let mut st = GitStatus {
        repo_root: String::new(),
        branch: None,
        head: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        files: Vec::new(),
    };
    let mut untracked = Vec::new();
    let mut recs = raw
        .split(|b| *b == 0)
        .map(|r| String::from_utf8_lossy(r).into_owned());
    let blank = |path: String| GitFileChange {
        path,
        orig_path: None,
        staged: None,
        unstaged: None,
        staged_added: 0,
        staged_removed: 0,
        unstaged_added: 0,
        unstaged_removed: 0,
        binary: false,
    };
    while let Some(rec) = recs.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" if v != "(initial)" => st.head = Some(v.to_string()),
                "branch.head" if v != "(detached)" => st.branch = Some(v.to_string()),
                "branch.upstream" => st.upstream = Some(v.to_string()),
                "branch.ab" => {
                    for p in v.split_whitespace() {
                        if let Some(n) = p.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = p.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let fields: Vec<&str> = rec.splitn(11, ' ').collect();
        match fields.first().copied() {
            Some("1") if fields.len() >= 9 => {
                let xy = fields[1].as_bytes();
                let mut c = blank(fields[8..].join(" "));
                c.staged = kind(xy[0]);
                c.unstaged = kind(xy[1]);
                st.files.push(c);
            }
            Some("2") if fields.len() >= 10 => {
                let xy = fields[1].as_bytes();
                let mut c = blank(fields[9..].join(" "));
                c.staged = kind(xy[0]);
                c.unstaged = kind(xy[1]);
                c.orig_path = recs.next();
                st.files.push(c);
            }
            Some("u") if fields.len() >= 11 => {
                let mut c = blank(fields[10].to_string());
                c.staged = Some(ChangeKind::Unmerged);
                c.unstaged = Some(ChangeKind::Unmerged);
                st.files.push(c);
            }
            Some("?") => {
                let path = rec[2..].to_string();
                let mut c = blank(path.clone());
                c.unstaged = Some(ChangeKind::Untracked);
                st.files.push(c);
                untracked.push(path);
            }
            _ => {}
        }
    }
    (st, untracked)
}

/// Parse `git diff --numstat -z`: path → (added, removed, binary).
pub fn parse_numstat(raw: &[u8]) -> HashMap<String, (u32, u32, bool)> {
    let mut out = HashMap::new();
    let mut recs = raw
        .split(|b| *b == 0)
        .map(|r| String::from_utf8_lossy(r).into_owned());
    while let Some(rec) = recs.next() {
        let mut it = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let binary = a == "-" && d == "-";
        let (a, d) = (a.parse().unwrap_or(0), d.parse().unwrap_or(0));
        let path = if p.is_empty() {
            // A rename: the old and the new path follow as their own records.
            let _old = recs.next();
            match recs.next() {
                Some(new) => new,
                None => continue,
            }
        } else {
            p.to_string()
        };
        out.insert(path, (a, d, binary));
    }
    out
}

async fn count_lines(path: &Path) -> (u32, bool) {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() && m.len() <= MAX_COUNT_FILE => {}
        _ => return (0, false),
    }
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return (0, true);
            }
            let n = bytes.iter().filter(|b| **b == b'\n').count()
                + usize::from(!bytes.is_empty() && bytes.last() != Some(&b'\n'));
            (n as u32, false)
        }
        Err(_) => (0, false),
    }
}

pub async fn status(dir: &Path) -> Result<GitStatus, MarsadError> {
    let root = repo_root(dir).await?;
    let out = git(
        &root,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )
    .await?;
    if !out.success() {
        return Err(failed("status", &out));
    }
    let (mut st, untracked) = parse_status(&out.stdout);
    st.repo_root = root.display().to_string();
    let (staged, unstaged) = tokio::join!(
        git(
            &root,
            &["diff", "--cached", "--numstat", "-z", "--no-renames"]
        ),
        git(&root, &["diff", "--numstat", "-z", "--no-renames"]),
    );
    let staged = staged?;
    let unstaged = unstaged?;
    let staged = if staged.success() {
        parse_numstat(&staged.stdout)
    } else {
        HashMap::new()
    };
    let unstaged = if unstaged.success() {
        parse_numstat(&unstaged.stdout)
    } else {
        return Err(failed("diff --numstat", &unstaged));
    };
    for f in &mut st.files {
        if let Some((a, d, b)) = staged.get(&f.path) {
            f.staged_added = *a;
            f.staged_removed = *d;
            f.binary |= *b;
        }
        if let Some((a, d, b)) = unstaged.get(&f.path) {
            f.unstaged_added = *a;
            f.unstaged_removed = *d;
            f.binary |= *b;
        }
    }
    for path in untracked {
        let (n, binary) = count_lines(&root.join(&path)).await;
        if let Some(f) = st.files.iter_mut().find(|f| f.path == path) {
            f.unstaged_added = n;
            f.binary = binary;
        }
    }
    Ok(st)
}

fn check_rel(path: &str) -> Result<(), MarsadError> {
    let p = Path::new(path);
    if path.is_empty()
        || p.is_absolute()
        // `/x` and `C:x` are not absolute on Windows, but they are not relative either.
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(MarsadError::invalid(format!(
            "{path:?} must be a path relative to the repository root"
        )));
    }
    Ok(())
}

/// Cut `text` to at most `max` bytes at a line boundary.
pub fn truncate_lines(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').map(|i| i + 1).unwrap_or(end);
    (text[..cut].to_string(), true)
}

pub async fn diff(
    dir: &Path,
    path: &str,
    side: DiffSide,
    max: usize,
) -> Result<GitDiff, MarsadError> {
    check_rel(path)?;
    let root = repo_root(dir).await?;
    let base = ["diff", "--no-color", "--no-ext-diff", "--patch"];
    let out = match side {
        DiffSide::Staged => {
            let mut a: Vec<&str> = base.to_vec();
            a.extend(["--cached", "--", path]);
            git(&root, &a).await?
        }
        DiffSide::Unstaged => {
            let tracked = git(&root, &["ls-files", "--error-unmatch", "--", path]).await?;
            if tracked.success() {
                let mut a: Vec<&str> = base.to_vec();
                a.extend(["--", path]);
                git(&root, &a).await?
            } else {
                let mut a: Vec<&str> = base.to_vec();
                a.extend(["--no-index", "--", "/dev/null", path]);
                // `--no-index` exits 1 when the files differ.
                let o = git(&root, &a).await?;
                Output {
                    code: if o.code == Some(1) { Some(0) } else { o.code },
                    ..o
                }
            }
        }
    };
    if !out.success() {
        return Err(failed("diff", &out));
    }
    let full = out.stdout_str();
    let binary = full
        .lines()
        .any(|l| l.starts_with("Binary files ") && l.ends_with(" differ"));
    let (text, truncated) = truncate_lines(&full, max);
    Ok(GitDiff {
        path: path.to_string(),
        side,
        text,
        truncated,
        binary,
    })
}

pub async fn commit(
    dir: &Path,
    message: &str,
    files: &[String],
) -> Result<GitCommitReply, MarsadError> {
    if message.trim().is_empty() {
        return Err(MarsadError::invalid("empty commit message"));
    }
    if files.is_empty() {
        return Err(MarsadError::invalid("no files selected"));
    }
    for f in files {
        check_rel(f)?;
    }
    let root = repo_root(dir).await?;
    let mut add: Vec<&str> = vec!["add", "-A", "--"];
    add.extend(files.iter().map(String::as_str));
    let out = git(&root, &add).await?;
    if !out.success() {
        return Err(failed("add", &out));
    }
    // A pathspec commit records exactly these paths, whatever else is staged.
    let mut c: Vec<&str> = vec!["commit", "--no-verify", "-m", message, "--"];
    c.extend(files.iter().map(String::as_str));
    let out = git(&root, &c).await?;
    if !out.success() {
        return Err(failed("commit", &out));
    }
    let head = git(&root, &["rev-parse", "HEAD"]).await?;
    if !head.success() {
        return Err(failed("rev-parse", &head));
    }
    Ok(GitCommitReply {
        commit: head.stdout_str().trim().to_string(),
    })
}

pub async fn discard(dir: &Path, files: &[String]) -> Result<(), MarsadError> {
    if files.is_empty() {
        return Err(MarsadError::invalid("no files selected"));
    }
    for f in files {
        check_rel(f)?;
    }
    let st = status(dir).await?;
    let root = PathBuf::from(&st.repo_root);
    let mut restore: Vec<&str> = Vec::new();
    let mut unstage_new: Vec<&str> = Vec::new();
    let mut delete: Vec<&str> = Vec::new();
    for f in files {
        let Some(c) = st.files.iter().find(|c| &c.path == f) else {
            return Err(MarsadError::NotFound {
                what: format!("no change to {f}"),
            });
        };
        match (c.staged, c.unstaged) {
            (_, Some(ChangeKind::Untracked)) => delete.push(f),
            (Some(ChangeKind::Added), _) => {
                unstage_new.push(f);
                delete.push(f);
            }
            _ => restore.push(f),
        }
        if let Some(orig) = &c.orig_path {
            // Undo a rename: restoring the old path brings it back.
            restore.push(orig);
            delete.push(f);
        }
    }
    if !unstage_new.is_empty() {
        let mut a = vec!["rm", "--cached", "-q", "-f", "--"];
        a.extend(unstage_new);
        let out = git(&root, &a).await?;
        if !out.success() {
            return Err(failed("rm --cached", &out));
        }
    }
    if !restore.is_empty() {
        let mut a = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        a.extend(restore);
        let out = git(&root, &a).await?;
        if !out.success() {
            return Err(failed("restore", &out));
        }
    }
    for f in delete {
        let p = root.join(f);
        match tokio::fs::remove_file(&p).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(MarsadError::io(format!("{}: {e}", p.display()))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_and_numstat() {
        let raw = b"# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
1 A. N... 000000 100644 100644 000 bbb added.rs\0\
? notes/x.md\0";
        let (st, untracked) = parse_status(raw);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head.as_deref(), Some("1234abcd"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.files.len(), 4);
        assert_eq!(st.files[0].path, "src/a b.rs");
        assert_eq!(st.files[0].unstaged, Some(ChangeKind::Modified));
        assert_eq!(st.files[0].staged, None);
        assert_eq!(st.files[1].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(st.files[1].staged, Some(ChangeKind::Renamed));
        assert_eq!(untracked, ["notes/x.md"]);

        let ns = parse_numstat(b"3\t1\tsrc/a b.rs\0-\t-\timg.png\0");
        assert_eq!(ns["src/a b.rs"], (3, 1, false));
        assert_eq!(ns["img.png"], (0, 0, true));
        let (st, _) = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!((st.head, st.branch), (None, None));
    }

    #[test]
    fn truncation_keeps_whole_lines() {
        assert_eq!(
            truncate_lines("ab\ncd\nef\n", 7),
            ("ab\ncd\n".to_string(), true)
        );
        assert_eq!(truncate_lines("ab\n", 7), ("ab\n".to_string(), false));
        assert!(check_rel("../x").is_err());
        assert!(check_rel("/x").is_err());
        #[cfg(windows)]
        {
            assert!(check_rel("C:\\x").is_err());
            assert!(check_rel("C:x").is_err());
            assert!(check_rel("a\\..\\..\\x").is_err());
        }
        assert!(check_rel("a/b").is_ok());
    }
}
