//! Session discovery — enumerate existing Claude Code sessions on disk.
//!
//! Claude stores sessions at `~/.claude/projects/<ENCODED_CWD>/<UUID>.jsonl`.
//! `ENCODED_CWD` is the absolute project path with every character that is NOT
//! in `[A-Za-z0-9]` replaced by `-`.
//!
//! Each `<UUID>.jsonl` file contains NDJSON lines; the UUID (filename stem) is
//! the Claude session id usable with `--resume`.  The first line whose `"type"`
//! field equals `"user"` carries the session's first prompt in
//! `message.content[].text`.

use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A summary of one discoverable Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The Claude session id (UUID filename stem). Pass to `--resume`.
    pub id: String,
    /// The first user prompt, trimmed and truncated to ~100 chars.
    pub snippet: String,
    /// File mtime as Unix epoch **milliseconds**.
    pub modified: u64,
}

// ---------------------------------------------------------------------------
// encode_project_dir
// ---------------------------------------------------------------------------

/// Encode an absolute project directory path into the Claude projects-root
/// subdirectory name.
///
/// Every character that is **not** ASCII alphanumeric (`[A-Za-z0-9]`) is
/// replaced by `-`.
///
/// ```
/// use marsad_core::discovery::encode_project_dir;
/// assert_eq!(encode_project_dir("/work/mewais/Marsad"), "-work-mewais-Marsad");
/// assert_eq!(encode_project_dir("/tmp/marsad-cont-3lx1an_6/project"),
///            "-tmp-marsad-cont-3lx1an-6-project");
/// ```
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ---------------------------------------------------------------------------
// first_user_prompt
// ---------------------------------------------------------------------------

/// Minimal serde shapes for parsing a JSONL line just enough to find the first
/// user message content.
#[derive(Deserialize)]
struct JsonlLine {
    #[serde(rename = "type")]
    line_type: Option<String>,
    message: Option<JsonlMessage>,
}

#[derive(Deserialize)]
struct JsonlMessage {
    content: Option<Vec<JsonlContent>>,
}

#[derive(Deserialize)]
struct JsonlContent {
    #[serde(rename = "type")]
    content_type: Option<String>,
    text: Option<String>,
}

/// Parse NDJSON from a `.jsonl` file and return the first `"user"` message's
/// first text-block content, trimmed and truncated to ~100 chars.
///
/// Returns `None` if no qualifying line is found.
pub fn first_user_prompt(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<JsonlLine>(trimmed) else {
            continue;
        };
        if parsed.line_type.as_deref() != Some("user") {
            continue;
        }
        let Some(message) = parsed.message else {
            continue;
        };
        let Some(contents) = message.content else {
            continue;
        };
        for block in contents {
            if block.content_type.as_deref() == Some("text") {
                if let Some(text) = block.text {
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    return Some(truncate_snippet(text));
                }
            }
        }
    }
    None
}

/// Truncate `s` to at most 100 Unicode scalar values, appending `"…"` when
/// truncation occurs.
fn truncate_snippet(s: String) -> String {
    let mut chars = s.char_indices();
    // Walk to the 100th character boundary.
    if let Some((byte_idx, _)) = chars.nth(100) {
        // There are more than 100 chars — truncate and append ellipsis.
        let mut result = s[..byte_idx].to_string();
        result.push('…');
        result
    } else {
        // 100 chars or fewer — return as-is.
        s
    }
}

// ---------------------------------------------------------------------------
// list_sessions
// ---------------------------------------------------------------------------

/// Enumerate Claude Code sessions for `cwd` under `projects_root`.
///
/// Reads `<projects_root>/<encode_project_dir(cwd)>/*.jsonl`, builds a
/// [`SessionSummary`] for each file, and returns the list sorted **newest
/// first** (by mtime).
///
/// If the encoded directory does not exist, returns an empty `Vec` (not an
/// error).
pub fn list_sessions(projects_root: &Path, cwd: &str) -> std::io::Result<Vec<SessionSummary>> {
    let encoded = encode_project_dir(cwd);
    let dir = projects_root.join(&encoded);

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries: Vec<SessionSummary> = Vec::new();

    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();

        // Skip everything that is not a `.jsonl` file (or has a non-UTF-8 stem).
        // Using filter_map logic in-line keeps the control flow branchless for llvm-cov.
        let id = match path
            .extension()
            .and_then(|e| e.to_str())
            .filter(|&e| e == "jsonl")
            .and_then(|_| path.file_stem())
            .and_then(|s| s.to_str())
        {
            Some(s) => s.to_string(),
            None => continue,
        };

        // Mtime as epoch milliseconds.
        let metadata = std::fs::metadata(&path)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Read the file and extract the first user prompt.
        let contents = std::fs::read_to_string(&path)?;
        let snippet = first_user_prompt(&contents).unwrap_or_default();

        summaries.push(SessionSummary {
            id,
            snippet,
            modified,
        });
    }

    // Sort newest first.
    summaries.sort_by_key(|s| std::cmp::Reverse(s.modified));

    Ok(summaries)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::SystemTime;

    // -------------------------------------------------------------------------
    // encode_project_dir
    // -------------------------------------------------------------------------

    #[test]
    fn encode_plain_alphanumeric_unchanged() {
        assert_eq!(encode_project_dir("abc123"), "abc123");
    }

    #[test]
    fn encode_leading_slash() {
        assert_eq!(
            encode_project_dir("/work/mewais/Marsad"),
            "-work-mewais-Marsad"
        );
    }

    #[test]
    fn encode_underscores_become_dashes() {
        assert_eq!(
            encode_project_dir("/tmp/marsad-cont-3lx1an_6/project"),
            "-tmp-marsad-cont-3lx1an-6-project"
        );
    }

    #[test]
    fn encode_dots_become_dashes() {
        assert_eq!(
            encode_project_dir("/home/user/my.project"),
            "-home-user-my-project"
        );
    }

    #[test]
    fn encode_spaces_become_dashes() {
        assert_eq!(
            encode_project_dir("/home/user/my project"),
            "-home-user-my-project"
        );
    }

    #[test]
    fn encode_mixed_case_preserved() {
        assert_eq!(encode_project_dir("/Work/MyProject"), "-Work-MyProject");
    }

    #[test]
    fn encode_empty_string() {
        assert_eq!(encode_project_dir(""), "");
    }

    // -------------------------------------------------------------------------
    // first_user_prompt
    // -------------------------------------------------------------------------

    #[test]
    fn first_user_prompt_none_on_empty_input() {
        assert_eq!(first_user_prompt(""), None);
    }

    #[test]
    fn first_user_prompt_none_when_no_user_line() {
        let jsonl = r#"{"type":"queue-operation","data":{}}"#;
        assert_eq!(first_user_prompt(jsonl), None);
    }

    #[test]
    fn first_user_prompt_returns_text_from_user_line() {
        let jsonl = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":[{"text":"Remember this exact token: MARSAD-CONT-77. Reply only: OK","type":"text"}]}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(
            result,
            Some("Remember this exact token: MARSAD-CONT-77. Reply only: OK".to_string())
        );
    }

    #[test]
    fn first_user_prompt_skips_non_user_lines_first() {
        let jsonl = concat!(
            r#"{"type":"queue-operation","data":{}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"text":"hi","type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"text":"first prompt","type":"text"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("first prompt".to_string()));
    }

    #[test]
    fn first_user_prompt_returns_first_of_multiple_user_lines() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"text":"first","type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"text":"second","type":"text"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("first".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_content_has_no_text_block() {
        // content present but no "type":"text" block
        let jsonl =
            r#"{"type":"user","message":{"role":"user","content":[{"type":"image","url":"x"}]}}"#;
        assert_eq!(first_user_prompt(jsonl), None);
    }

    #[test]
    fn first_user_prompt_skips_non_text_content_then_finds_text() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"image","url":"x"},{"type":"text","text":"hello"}]}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn first_user_prompt_truncates_long_text() {
        // 101 'a' characters → truncated to 100 + '…'
        let long_text = "a".repeat(101);
        let jsonl = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{long_text}"}}]}}}}"#
        );
        let result = first_user_prompt(&jsonl).unwrap();
        assert!(result.ends_with('…'), "should end with ellipsis: {result}");
        // The snippet should be 100 'a's + '…'
        assert_eq!(result, format!("{}…", "a".repeat(100)));
    }

    #[test]
    fn first_user_prompt_exactly_100_chars_no_truncation() {
        let text = "b".repeat(100);
        let jsonl = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        );
        let result = first_user_prompt(&jsonl).unwrap();
        assert_eq!(result, text);
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn first_user_prompt_skips_empty_text_block() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"   "}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"real prompt"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("real prompt".to_string()));
    }

    #[test]
    fn first_user_prompt_skips_invalid_json_lines() {
        let jsonl = concat!(
            "not json\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"valid"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("valid".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_message_is_null() {
        let jsonl = r#"{"type":"user","message":null}"#;
        // The serde type uses Option<JsonlMessage>, so null deserializes to None.
        // The function should gracefully return None and fall through.
        let result = first_user_prompt(jsonl);
        assert_eq!(result, None);
    }

    #[test]
    fn first_user_prompt_skips_blank_lines() {
        // A JSONL file may contain blank lines between records.
        let jsonl = concat!(
            "   \n",
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"after blanks"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("after blanks".to_string()));
    }

    #[test]
    fn first_user_prompt_none_when_message_content_is_null() {
        // message present but content field is null
        let jsonl = r#"{"type":"user","message":{"role":"user","content":null}}"#;
        let result = first_user_prompt(jsonl);
        assert_eq!(result, None);
    }

    #[test]
    fn first_user_prompt_text_block_with_no_text_field() {
        // content block has type "text" but no text field, followed by a valid line
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"second"}]}}"#,
        );
        let result = first_user_prompt(jsonl);
        assert_eq!(result, Some("second".to_string()));
    }

    // -------------------------------------------------------------------------
    // list_sessions
    // -------------------------------------------------------------------------

    fn write_jsonl(dir: &std::path::Path, filename: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn set_mtime(path: &std::path::Path, offset_secs: i64) {
        // Set mtime relative to now using filetime crate — but we don't have that dep.
        // Instead we use std: touch the file with a fake timestamp via std::fs::File
        // metadata manipulation — not available in std. We'll just write and immediately
        // sleep is not an option. Use std::fs::set_modified... which is nightly only.
        //
        // Work-around: write the file at slightly different real times is unreliable.
        // Instead we manipulate the mtime via the `utime` syscall through a raw libc call.
        // Since we're in test code only, we use std::process::Command touch -t.
        use std::process::Command;
        // Compute a timestamp offset from now.
        let base = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let ts = base + offset_secs;
        // Format as [[CC]YY]MMDDhhmm[.SS]
        let dt = ts;
        // Use `touch -d @<epoch>` (GNU coreutils) for simplicity.
        Command::new("touch")
            .arg("-d")
            .arg(format!("@{dt}"))
            .arg(path)
            .status()
            .expect("touch failed");
    }

    #[test]
    fn list_sessions_empty_when_dir_missing() {
        let tmp = tempdir();
        // Use a projects root that has no matching subdir.
        let result = list_sessions(tmp.path(), "/nonexistent/project").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_sessions_empty_when_encoded_dir_missing() {
        let tmp = tempdir();
        // Create a different subdir to make sure the root exists.
        std::fs::create_dir_all(tmp.path().join("some-other-dir")).unwrap();
        let result = list_sessions(tmp.path(), "/my/project").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_sessions_single_file_with_prompt() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/my/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello world"}]}}"#;
        write_jsonl(&proj_dir, "abc-uuid-123.jsonl", jsonl);

        let result = list_sessions(tmp.path(), "/my/project").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "abc-uuid-123");
        assert_eq!(result[0].snippet, "hello world");
        assert!(result[0].modified > 0);
    }

    #[test]
    fn list_sessions_file_with_no_user_prompt_has_empty_snippet() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/proj");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl = r#"{"type":"queue-operation","data":{}}"#;
        write_jsonl(&proj_dir, "no-prompt.jsonl", jsonl);

        let result = list_sessions(tmp.path(), "/proj").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].snippet, "");
    }

    #[test]
    fn list_sessions_skips_non_jsonl_files() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/proj2");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        write_jsonl(&proj_dir, "session.jsonl", r#"{"type":"queue-operation"}"#);
        // Write a non-.jsonl file.
        let txt_path = proj_dir.join("readme.txt");
        std::fs::write(&txt_path, "not a session").unwrap();

        let result = list_sessions(tmp.path(), "/proj2").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "session");
    }

    #[test]
    fn list_sessions_sorted_newest_first() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/myproject");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let jsonl_older = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"older prompt"}]}}"#;
        let jsonl_newer = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"newer prompt"}]}}"#;

        let older_path = write_jsonl(&proj_dir, "older-session.jsonl", jsonl_older);
        let newer_path = write_jsonl(&proj_dir, "newer-session.jsonl", jsonl_newer);

        // Set older file mtime to 60 seconds in the past.
        set_mtime(&older_path, -60);
        // Set newer file mtime to now (default after write is fine, but make explicit).
        set_mtime(&newer_path, 0);

        let result = list_sessions(tmp.path(), "/myproject").unwrap();
        assert_eq!(result.len(), 2);
        // Newest first.
        assert_eq!(result[0].snippet, "newer prompt", "newest must be first");
        assert_eq!(result[1].snippet, "older prompt");
        // Newest-first ordering: [0].modified must be >= [1].modified
        let newer_mod = result[0].modified;
        let older_mod = result[1].modified;
        assert!(newer_mod >= older_mod);
    }

    #[test]
    fn list_sessions_multiple_files_with_prompts() {
        let tmp = tempdir();
        let encoded = encode_project_dir("/multi");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        let sessions = [
            ("session-a.jsonl", "task alpha", -120i64),
            ("session-b.jsonl", "task beta", -60),
            ("session-c.jsonl", "task gamma", 0),
        ];

        for (filename, prompt, offset) in &sessions {
            let jsonl = format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{prompt}"}}]}}}}"#
            );
            let path = write_jsonl(&proj_dir, filename, &jsonl);
            set_mtime(&path, *offset);
        }

        let result = list_sessions(tmp.path(), "/multi").unwrap();
        assert_eq!(result.len(), 3);
        // Newest first: gamma (0), beta (-60), alpha (-120)
        assert_eq!(result[0].snippet, "task gamma");
        assert_eq!(result[1].snippet, "task beta");
        assert_eq!(result[2].snippet, "task alpha");
    }

    #[test]
    fn list_sessions_io_error_on_file_instead_of_dir() {
        // If the encoded path exists as a FILE (not a dir), read_dir returns an error.
        let tmp = tempdir();
        let encoded = encode_project_dir("/io/error/project");
        // Write a regular file at the encoded path.
        let fake_dir_path = tmp.path().join(&encoded);
        std::fs::write(&fake_dir_path, "not a directory").unwrap();

        // list_sessions should propagate the I/O error.
        let result = list_sessions(tmp.path(), "/io/error/project");
        assert!(
            result.is_err(),
            "expected I/O error when encoded path is a file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn list_sessions_io_error_on_unreadable_jsonl_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir();
        let encoded = encode_project_dir("/perm/test/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        // Write a valid .jsonl file, then remove read permission.
        let path = write_jsonl(
            &proj_dir,
            "secret.jsonl",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"secret"}]}}"#,
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = list_sessions(tmp.path(), "/perm/test/project");
        // Restore permissions so the TempDir cleanup can remove the file.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            result.is_err(),
            "expected I/O error for unreadable .jsonl file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn list_sessions_io_error_on_dangling_symlink_jsonl() {
        // A dangling symlink ending in .jsonl will appear in the directory listing
        // but `std::fs::metadata()` (which follows symlinks) will fail with ENOENT.
        use std::os::unix::fs::symlink;
        let tmp = tempdir();
        let encoded = encode_project_dir("/symlink/test/project");
        let proj_dir = tmp.path().join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();

        // Create a dangling symlink: points to a non-existent target.
        let link_path = proj_dir.join("dangling.jsonl");
        symlink("/nonexistent/target/path", &link_path).unwrap();

        let result = list_sessions(tmp.path(), "/symlink/test/project");
        assert!(
            result.is_err(),
            "expected I/O error for dangling symlink .jsonl"
        );
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("marsad-discovery-test-{n}"));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}
