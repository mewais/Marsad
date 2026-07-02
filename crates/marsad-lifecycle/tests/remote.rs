//! Pure-logic coverage for `RemoteTmuxContainer`.
//!
//! Exercises the argv/script builders and remote POSIX path construction for
//! both an ssh-style prefix (`["ssh", "user@h", "--"]`) and a wsl-style prefix
//! (`["wsl.exe", "-d", "Ubuntu", "--"]`), plus the idempotency guard and edge
//! prefixes.  The real `Command::spawn()` is gated `#[cfg(not(coverage))]`, so
//! these tests reach 100 % of the pure surface without an ssh/tmux/wsl.

use marsad_lifecycle::{remote_shell_argv, RemoteTmuxContainer};

// ── helpers ───────────────────────────────────────────────────────────────────

fn ssh_prefix() -> Vec<String> {
    ["ssh", "user@h", "--"]
        .iter()
        .map(|s| s.to_string())
        .collect()
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
    assert_eq!(
        c.remote_session_dir("marsad-a"),
        "~/.marsad/sessions/marsad-a"
    );
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
    assert_eq!(c.remote_session_dir("x"), "/var/lib/marsad/sessions/x");
}

// ── exists argv ────────────────────────────────────────────────────────────────

#[test]
fn exists_script_is_has_session() {
    assert_eq!(
        RemoteTmuxContainer::exists_script("marsad-7"),
        "tmux has-session -t marsad-7"
    );
}

// ── kill argv ──────────────────────────────────────────────────────────────────

#[test]
fn kill_script_is_kill_session() {
    assert_eq!(
        RemoteTmuxContainer::kill_script("marsad-7"),
        "tmux kill-session -t marsad-7"
    );
}

// ── spawn script (ssh prefix) ──────────────────────────────────────────────────

#[test]
fn spawn_script_ssh_contains_all_stages() {
    let c = RemoteTmuxContainer::new(ssh_prefix(), "$HOME/.marsad");
    let s = c.spawn_script("marsad-a", "claude --resume id1");

    // mkdir of the remote session dir — UNQUOTED so $HOME expands remotely.
    assert!(
        s.contains("mkdir -p $HOME/.marsad/sessions/marsad-a"),
        "missing mkdir stage: {s}"
    );
    // mkfifo (guarded) for both FIFOs
    assert!(
        s.contains("[ -p $HOME/.marsad/sessions/marsad-a/in.fifo ] || mkfifo $HOME/.marsad/sessions/marsad-a/in.fifo"),
        "missing in.fifo mkfifo guard: {s}"
    );
    assert!(
        s.contains("[ -p $HOME/.marsad/sessions/marsad-a/out.fifo ] || mkfifo $HOME/.marsad/sessions/marsad-a/out.fifo"),
        "missing out.fifo mkfifo guard: {s}"
    );
    // The command is written VERBATIM into run.sh via a quoted heredoc...
    assert!(
        s.contains("cat > $HOME/.marsad/sessions/marsad-a/run.sh <<'__MARSAD_RUN__'\nclaude --resume id1\n__MARSAD_RUN__"),
        "missing run.sh heredoc with verbatim cmd: {s}"
    );
    // ...then tmux runs it via a LOGIN shell (`bash -l run.sh`) with the FIFOs wired,
    // guarded by has-session for idempotency.
    assert!(
        s.contains(r#"tmux has-session -t marsad-a 2>/dev/null || tmux new-session -d -s marsad-a "bash -l $HOME/.marsad/sessions/marsad-a/run.sh < $HOME/.marsad/sessions/marsad-a/in.fifo > $HOME/.marsad/sessions/marsad-a/out.fifo 2>$HOME/.marsad/sessions/marsad-a/err.log""#),
        "missing login-shell new-session with fifo wiring: {s}"
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
fn spawn_script_writes_cmd_verbatim_into_run_sh() {
    let c = RemoteTmuxContainer::new(wsl_prefix(), "~/.marsad");
    // The cmd is written VERBATIM into run.sh via a quoted heredoc — its own quotes
    // are NOT re-escaped here (the single outer quoting is done by remote_shell_argv).
    let s = c.spawn_script("n", "echo 'hi'");
    assert!(
        s.contains("<<'__MARSAD_RUN__'\necho 'hi'\n__MARSAD_RUN__"),
        "cmd must be written verbatim into the run.sh heredoc: {s}"
    );
    assert!(
        s.contains("bash -l ~/.marsad/sessions/n/run.sh"),
        "must run via login shell: {s}"
    );
}

// ── remote_shell_argv: ssh re-parses (single token), wsl preserves (sh -c) ──────

#[test]
fn remote_shell_argv_ssh_uses_login_shell_single_token() {
    // ssh joins trailing args with spaces + re-runs via the remote shell, so the whole
    // command must be ONE token. It's wrapped in `bash -lc '<script>'` so a LOGIN shell
    // sources the profile/modules (→ tmux on PATH).
    let argv = remote_shell_argv(&ssh_prefix(), "tmux has-session -t s");
    assert_eq!(argv[0], "ssh");
    assert_eq!(argv.last().unwrap(), "bash -lc 'tmux has-session -t s'");
    assert!(
        !argv.contains(&"sh".to_string()),
        "ssh must not use a bare sh -c: {argv:?}"
    );
}

#[test]
fn remote_shell_argv_plink_uses_login_shell_like_ssh() {
    // plink re-parses its trailing command like ssh → same login-shell single token.
    let prefix = vec![
        "plink".to_string(),
        "-share".to_string(),
        "user@h".to_string(),
    ];
    let argv = remote_shell_argv(&prefix, "tmux has-session -t s");
    assert_eq!(argv[0], "plink");
    assert_eq!(argv.last().unwrap(), "bash -lc 'tmux has-session -t s'");
}

#[test]
fn remote_shell_argv_wsl_uses_login_bash() {
    // wsl.exe preserves argv (no re-parse), but the script still needs a LOGIN shell
    // so ~/.local/bin (claude) is on PATH — `bash -lc <script>` as explicit args.
    let argv = remote_shell_argv(&wsl_prefix(), "command -v claude");
    assert_eq!(argv[argv.len() - 3], "bash");
    assert_eq!(argv[argv.len() - 2], "-lc");
    assert_eq!(argv.last().unwrap(), "command -v claude");
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
