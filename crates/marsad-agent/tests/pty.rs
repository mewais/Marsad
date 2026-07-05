//! WAVE 3b — pty escape-hatch terminal: pure selection logic (`shell_for`,
//! `default_shell`, `pty_exec_argv`). Always compiled — no external
//! dependency (no real pty, no OS process) — unlike the REAL pty spawn/IO
//! tests in `tests/pty_live.rs`, which are `#![cfg(not(coverage))]` (see that
//! file's header for why).

use marsad_agent::{default_shell, pty_exec_argv, shell_for};

// ---------------------------------------------------------------------------
// shell_for / default_shell
// ---------------------------------------------------------------------------

#[test]
fn shell_for_prefers_non_empty_shell_env() {
    assert_eq!(shell_for(Some("/usr/bin/zsh")), "/usr/bin/zsh");
}

#[test]
fn shell_for_falls_back_when_env_absent() {
    let fallback = shell_for(None);
    assert!(!fallback.is_empty());
    #[cfg(not(windows))]
    assert_eq!(fallback, "/bin/sh");
    #[cfg(windows)]
    assert_eq!(fallback, "cmd.exe");
}

#[test]
fn shell_for_falls_back_when_env_is_empty_string() {
    assert_eq!(shell_for(Some("")), shell_for(None));
}

#[test]
fn shell_for_falls_back_when_env_is_whitespace_only() {
    assert_eq!(shell_for(Some("   ")), shell_for(None));
}

/// Smoke test for the thin production wrapper (reads the REAL `$SHELL`) —
/// `shell_for` above already covers both of its branches exhaustively; this
/// just proves the wrapper itself returns something usable regardless of
/// what the test machine's `$SHELL` happens to be.
#[test]
fn default_shell_returns_a_nonempty_string() {
    assert!(!default_shell().is_empty());
}

// ---------------------------------------------------------------------------
// pty_exec_argv
// ---------------------------------------------------------------------------

#[test]
fn pty_exec_argv_local_none_uses_bare_shell() {
    assert_eq!(
        pty_exec_argv(None, "/bin/bash"),
        vec!["/bin/bash".to_string()]
    );
}

#[test]
fn pty_exec_argv_local_aliases_use_bare_shell() {
    assert_eq!(
        pty_exec_argv(Some("local"), "/bin/bash"),
        vec!["/bin/bash".to_string()]
    );
    assert_eq!(
        pty_exec_argv(Some("localhost"), "/bin/bash"),
        vec!["/bin/bash".to_string()]
    );
}

/// Bare `wsl:<distro>` carries no `-T`/no-pty flag to flip — `wsl.exe`
/// already launches an interactive default shell on its own — so the argv
/// passes through unchanged from `remote::exec_prefix_for`.
#[test]
fn pty_exec_argv_wsl_bare_is_unchanged() {
    assert_eq!(
        pty_exec_argv(Some("wsl:Ubuntu"), "/bin/bash"),
        vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            "Ubuntu".to_string()
        ]
    );
}

/// `wslssh:<distro>:<target>` nests a real `ssh -T …` — the one-shot,
/// no-pty flag — which must be flipped to `-tt` (force pty) so the remote
/// shell is fully interactive.
#[test]
fn pty_exec_argv_wslssh_flips_capital_t_to_lowercase_tt() {
    let argv = pty_exec_argv(Some("wslssh:Ubuntu:user@host"), "/bin/bash");
    assert_eq!(argv[0], "wsl.exe");
    assert_eq!(argv[1], "-d");
    assert_eq!(argv[2], "Ubuntu");
    assert_eq!(argv[3], "ssh");
    assert!(
        !argv.contains(&"-T".to_string()),
        "the one-shot no-pty flag must be gone: {argv:?}"
    );
    assert!(
        argv.contains(&"-tt".to_string()),
        "a force-pty flag must be present: {argv:?}"
    );
    assert_eq!(argv.last(), Some(&"user@host".to_string()));
}

/// Native ssh (no wsl/plink wrapper): same `-T` → `-tt` flip. Asserted by
/// property (not full-array equality) because `remote::exec_prefix_for`'s
/// native-ssh argv includes extra `ControlMaster` options only on
/// `cfg(unix)` — this keeps the test meaningful on either host platform.
#[test]
fn pty_exec_argv_native_ssh_flips_capital_t_to_lowercase_tt() {
    let argv = pty_exec_argv(Some("user@example.com"), "/bin/bash");
    assert_eq!(argv[0], "ssh");
    assert!(
        !argv.contains(&"-T".to_string()),
        "the one-shot no-pty flag must be gone: {argv:?}"
    );
    assert!(
        argv.contains(&"-tt".to_string()),
        "a force-pty flag must be present: {argv:?}"
    );
    assert_eq!(argv.last(), Some(&"user@example.com".to_string()));
}

/// plink carries no `-T`/no-pty flag to swap (PuTTY's own defaults differ
/// from ssh's), so `pty_exec_argv` inserts `-t` (force pty) explicitly,
/// right after the binary name — before `-share`/`-batch`.
#[test]
fn pty_exec_argv_plink_inserts_force_pty_flag_after_binary() {
    assert_eq!(
        pty_exec_argv(Some("plink:my-session"), "/bin/bash"),
        vec![
            "plink".to_string(),
            "-t".to_string(),
            "-share".to_string(),
            "-batch".to_string(),
            "my-session".to_string(),
        ]
    );
}
