//! Pure-logic coverage for `RemoteTmuxContainer`.
//!
//! Exercises the argv/script builders and remote POSIX path construction for
//! both an ssh-style prefix (`["ssh", "user@h", "--"]`) and a wsl-style prefix
//! (`["wsl.exe", "-d", "Ubuntu", "--"]`), plus the idempotency guard and edge
//! prefixes.  The real `Command::spawn()` is gated `#[cfg(not(coverage))]`, so
//! these tests reach 100 % of the pure surface without an ssh/tmux/wsl.

use marsad_lifecycle::RemoteTmuxContainer;

// ── helpers ───────────────────────────────────────────────────────────────────

fn ssh_prefix() -> Vec<String> {
    ["ssh", "user@h", "--"].iter().map(|s| s.to_string()).collect()
}

fn wsl_prefix() -> Vec<String> {
    ["wsl.exe", "-d", "Ubuntu", "--"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// ── remote session-dir / FIFO path construction ────────────────────────────────

#[test]
fn remote_session_dir_joins_with_posix_slashes() {
    let c = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    assert_eq!(c.remote_session_dir("marsad-a"), "~/.marsad/sessions/marsad-a");
}

#[test]
fn remote_session_dir_independent_of_prefix() {
    // The prefix (ssh vs wsl) must not affect the remote path layout.
    let ssh = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    let wsl = RemoteTmuxContainer::new(wsl_prefix(), "~/.marsad");
    assert_eq!(
        ssh.remote_session_dir("s"),
        wsl.remote_session_dir("s"),
        "remote layout must be prefix-agnostic"
    );
}

#[test]
fn fifo_paths_live_under_the_session_dir() {
    let c = RemoteTmuxContainer::new(wsl_prefix(), "/home/u/.marsad");
    assert_eq!(
        c.in_fifo_path("sid"),
        "/home/u/.marsad/sessions/sid/in.fifo"
    );
    assert_eq!(
        c.out_fifo_path("sid"),
        "/home/u/.marsad/sessions/sid/out.fifo"
    );
}

#[test]
fn runtime_dir_with_trailing_segment_is_preserved_verbatim() {
    // No std::path normalization: an absolute remote path stays as-is.
    let c = RemoteTmuxContainer::new(ssh_prefix(), "/var/lib/marsad");
    assert_eq!(
        c.remote_session_dir("x"),
        "/var/lib/marsad/sessions/x"
    );
}

// ── exists argv ────────────────────────────────────────────────────────────────

#[test]
fn exists_argv_is_has_session() {
    assert_eq!(
        RemoteTmuxContainer::exists_argv("marsad-7"),
        vec![
            "tmux".to_string(),
            "has-session".to_string(),
            "-t".to_string(),
            "marsad-7".to_string(),
        ]
    );
}

// ── kill argv ──────────────────────────────────────────────────────────────────

#[test]
fn kill_argv_is_kill_session() {
    assert_eq!(
        RemoteTmuxContainer::kill_argv("marsad-7"),
        vec![
            "tmux".to_string(),
            "kill-session".to_string(),
            "-t".to_string(),
            "marsad-7".to_string(),
        ]
    );
}

// ── spawn script (ssh prefix) ──────────────────────────────────────────────────

#[test]
fn spawn_script_ssh_contains_all_stages() {
    let c = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    let s = c.spawn_script("marsad-a", "claude --resume id1");

    // mkdir of the remote session dir
    assert!(
        s.contains("mkdir -p '~/.marsad/sessions/marsad-a'"),
        "missing mkdir stage: {s}"
    );
    // mkfifo (guarded) for both FIFOs
    assert!(
        s.contains("[ -p '~/.marsad/sessions/marsad-a/in.fifo' ] || mkfifo '~/.marsad/sessions/marsad-a/in.fifo'"),
        "missing in.fifo mkfifo guard: {s}"
    );
    assert!(
        s.contains("[ -p '~/.marsad/sessions/marsad-a/out.fifo' ] || mkfifo '~/.marsad/sessions/marsad-a/out.fifo'"),
        "missing out.fifo mkfifo guard: {s}"
    );
    // idempotency guard before new-session
    assert!(
        s.contains("tmux has-session -t 'marsad-a' 2>/dev/null || tmux new-session -d -s 'marsad-a' 'claude --resume id1'"),
        "missing idempotency guard + new-session: {s}"
    );
}

// ── spawn script (wsl prefix) — script is prefix-agnostic ──────────────────────

#[test]
fn spawn_script_identical_across_prefixes() {
    let ssh = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    let wsl = RemoteTmuxContainer::new(wsl_prefix(), "~/.marsad");
    assert_eq!(
        ssh.spawn_script("n", "cmd"),
        wsl.spawn_script("n", "cmd"),
        "the remote script must not depend on the exec prefix"
    );
}

// ── single-quote escaping of name and cmd ──────────────────────────────────────

#[test]
fn spawn_script_escapes_single_quotes_in_cmd() {
    let c = RemoteTmuxContainer::new(wsl_prefix(), "~/.marsad");
    // A cmd containing a single quote must be escaped via the '\'' idiom so the
    // remote shell parses it as one argument.
    let s = c.spawn_script("n", "echo 'hi'");
    assert!(
        s.contains(r#"'echo '\''hi'\'''"#),
        "single quotes in cmd must be escaped: {s}"
    );
}

#[test]
fn spawn_script_escapes_single_quotes_in_name() {
    let c = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    let s = c.spawn_script("a'b", "cmd");
    // The name appears (escaped) in both the dir path and the tmux args.
    assert!(
        s.contains(r#"-s 'a'\''b'"#),
        "single quotes in name must be escaped: {s}"
    );
}

// ── edge prefixes / construction ───────────────────────────────────────────────

#[test]
fn empty_prefix_still_builds_pure_logic() {
    // An empty exec_prefix is a degenerate (would fail at spawn time), but the
    // pure path builders must not depend on it.
    let c = RemoteTmuxContainer::new(Vec::new(), "~/.marsad");
    assert_eq!(c.remote_session_dir("z"), "~/.marsad/sessions/z");
    assert!(c.spawn_script("z", "cmd").contains("mkdir -p"));
}

#[test]
fn new_accepts_string_and_str_runtime_dir() {
    // Into<String> ergonomics: both &str and String compile.
    let _a = RemoteTmuxContainer::new(ssh_prefix(), "~/.marsad");
    let _b = RemoteTmuxContainer::new(ssh_prefix(), String::from("~/.marsad"));
}
