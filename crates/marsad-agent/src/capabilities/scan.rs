//! Temp-dir-parameterized session scanners (filesystem IO rooted at a caller-supplied path).
//!
//! Extracted from `capabilities.rs` verbatim; `use super::*` inherits the
//! parent module's imports and cross-section helpers.

use super::*;

// ===========================================================================
// Part 2 — temp-dir-parameterized scanners (real filesystem IO, rooted at a
// caller-supplied &Path so unit tests can point them at a temp tree)
// ===========================================================================

/// Recursively collect every file under `dir`, paired with its mtime. Callers
/// filter by name pattern afterward (`parse_codex_rollout_meta` already
/// rejects anything that isn't `rollout-*.jsonl`). An absent/unreadable `dir`
/// silently yields no files.
fn collect_jsonl_files(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
            continue;
        }
        out.extend(
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|mtime| (path, mtime)),
        );
    }
}

fn system_time_to_ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Scan `<root>/<Y>/<M>/<D>/rollout-*.jsonl` newest-first, capped at `cap`
/// files (codex-research.md §d). An absent/empty root yields an empty list —
/// never an error.
pub fn scan_codex_sessions(root: &Path, cap: usize) -> Vec<DiscoveredSession> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    files
        .into_iter()
        .take(cap)
        .filter_map(|(path, mtime)| {
            path.file_name()
                .and_then(|f| f.to_str())
                .map(str::to_string)
                .and_then(|filename| {
                    let body = std::fs::read_to_string(&path).unwrap_or_default();
                    let first_line = body.lines().next().unwrap_or("");
                    parse_codex_rollout_meta(&filename, first_line).map(|(id, project)| {
                        DiscoveredSession {
                            id,
                            project,
                            snippet: extract_codex_user_snippet(&body),
                            modified_ms: system_time_to_ms(mtime),
                        }
                    })
                })
        })
        .collect()
}

/// Encode a project's absolute path into claude's on-disk directory name:
/// every `/` becomes `-` (claude stores sessions at
/// `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`). Pure; no IO.
pub fn encode_claude_project_dir(project_abs: &str) -> String {
    project_abs.replace('/', "-")
}

/// Extract the first user-message text from a claude session `.jsonl` body.
/// claude's transcript lines are `{"type":"user","message":{"role":"user",
/// "content":...}}` where content is either a plain string or an array of
/// content blocks. Returns an empty string when nothing is cheaply
/// extractable (never a parse error).
fn claude_snippet_from_body(body: &str) -> String {
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let content = match v.get("message").and_then(|m| m.get("content")) {
            Some(c) => c,
            None => continue,
        };
        // Plain-string content.
        if let Some(s) = content.as_str() {
            return s.trim().to_string();
        }
        // Array-of-blocks content: first block with a `text` field.
        if let Some(arr) = content.as_array() {
            for block in arr {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    return t.trim().to_string();
                }
            }
        }
    }
    String::new()
}

/// List claude resumable sessions for one `project` under `projects_root`
/// (`~/.claude/projects`). Sessions live at
/// `<projects_root>/<encoded-cwd>/<uuid>.jsonl` — the encoded dir is the
/// project abs path with `/`→`-`. Returns each `*.jsonl` newest-first, capped
/// at `cap`, as `DiscoveredSession { id = filename stem (uuid), project,
/// snippet = first user message text, modified }`. An absent/empty project
/// directory yields an empty list — never an error.
pub fn scan_claude_sessions(
    projects_root: &Path,
    project: &str,
    cap: usize,
) -> Vec<DiscoveredSession> {
    let dir = projects_root.join(encode_claude_project_dir(project));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("jsonl")
        })
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, mtime))
        })
        .collect();
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    files
        .into_iter()
        .take(cap)
        .filter_map(|(path, mtime)| {
            let id = path.file_stem().and_then(|s| s.to_str())?.to_string();
            if id.is_empty() {
                return None;
            }
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            Some(DiscoveredSession {
                id,
                project: Some(project.to_string()),
                snippet: claude_snippet_from_body(&body),
                modified_ms: system_time_to_ms(mtime),
            })
        })
        .collect()
}

