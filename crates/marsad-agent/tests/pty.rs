//! WAVE 3b — pty escape-hatch terminal: pure selection logic (`shell_for`,
//! `default_shell`, `pty_exec_argv`) plus the `PtyScrollback` ring-buffer
//! helper. Always compiled — no external dependency (no real pty, no OS
//! process) — unlike the REAL pty spawn/IO tests in `tests/pty_live.rs`,
//! which are `#![cfg(not(coverage))]` (see that file's header for why).

use marsad_agent::{default_shell, pty_exec_argv, shell_for, PtyScrollback};

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

// ---------------------------------------------------------------------------
// PtyScrollback — ring buffer: bounds, order, replay
// ---------------------------------------------------------------------------

/// A fresh buffer is empty and `lines()` returns an empty vec.
#[test]
fn scrollback_starts_empty() {
    let sb: PtyScrollback<10> = PtyScrollback::new();
    assert_eq!(sb.len(), 0);
    assert!(sb.is_empty());
    assert!(sb.lines().is_empty());
}

/// Pushing up to the capacity does not evict anything.
#[test]
fn scrollback_fills_to_cap_without_eviction() {
    let mut sb: PtyScrollback<4> = PtyScrollback::new();
    for i in 0..4usize {
        sb.push(format!("line{i}"));
    }
    assert_eq!(sb.len(), 4);
    let lines = sb.lines();
    assert_eq!(lines, vec!["line0", "line1", "line2", "line3"]);
}

/// Pushing beyond the capacity evicts the OLDEST entries and retains the
/// NEWEST, preserving insertion order in the remaining slice.
#[test]
fn scrollback_evicts_oldest_when_over_cap() {
    let mut sb: PtyScrollback<3> = PtyScrollback::new();
    // Push 5 entries into a cap-3 buffer.
    for i in 0..5usize {
        sb.push(format!("line{i}"));
    }
    // Only the 3 newest (line2, line3, line4) should remain, in order.
    assert_eq!(sb.len(), 3);
    let lines = sb.lines();
    assert_eq!(lines, vec!["line2", "line3", "line4"]);
}

/// The lines returned by `lines()` are in oldest→newest insertion order —
/// the correct replay order for xterm.js reconstruction.
#[test]
fn scrollback_lines_are_oldest_to_newest() {
    let mut sb: PtyScrollback<5> = PtyScrollback::new();
    let input: Vec<String> = (0..5).map(|i| format!("msg{i}")).collect();
    for line in &input {
        sb.push(line.clone());
    }
    assert_eq!(sb.lines(), input);
}

/// A CAP-0 buffer never stores anything (no-op push, always empty lines).
#[test]
fn scrollback_zero_cap_is_always_empty() {
    let mut sb: PtyScrollback<0> = PtyScrollback::new();
    sb.push("should be dropped".to_string());
    assert_eq!(sb.len(), 0);
    assert!(sb.lines().is_empty());
}

/// `lines()` returns a snapshot that is independent of subsequent pushes —
/// i.e. calling `push` after `lines()` does not mutate the snapshot.
#[test]
fn scrollback_lines_snapshot_is_independent() {
    let mut sb: PtyScrollback<4> = PtyScrollback::new();
    sb.push("a".to_string());
    sb.push("b".to_string());
    let snapshot = sb.lines();
    sb.push("c".to_string()); // mutate AFTER taking the snapshot
    // The snapshot must still reflect the state at the time it was taken.
    assert_eq!(snapshot, vec!["a", "b"]);
    // The live buffer has the new entry.
    assert_eq!(sb.lines(), vec!["a", "b", "c"]);
}

/// Wrap-around stress: push N entries into a cap-K buffer and verify that
/// after wrapping many times, the K newest entries are retained in order.
#[test]
fn scrollback_wrap_around_stress() {
    const CAP: usize = 7;
    let n = 100usize;
    let mut sb: PtyScrollback<CAP> = PtyScrollback::new();
    for i in 0..n {
        sb.push(format!("entry{i}"));
    }
    assert_eq!(sb.len(), CAP);
    // The K newest are entries (n-CAP)..(n), in order.
    let expected: Vec<String> = (n - CAP..n).map(|i| format!("entry{i}")).collect();
    assert_eq!(sb.lines(), expected);
}
