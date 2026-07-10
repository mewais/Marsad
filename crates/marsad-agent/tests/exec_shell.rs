//! Real-IO tests for the persistent per-session `!` shell (local bash).
//!
//! `#![cfg(not(coverage))]` so it compiles to zero tests under `cargo llvm-cov`
//! (which sets `--cfg coverage` and excludes the `ExecShell` API), mirroring
//! `tests/pty.rs`. A local `bash` is present in this sandbox, so unlike the
//! remote path these are genuinely exercised.
//!
//! Output keeps the command's natural trailing newline (same as the one-shot
//! path it replaces), so assertions compare with `trim_end()`.
#![cfg(not(coverage))]

use marsad_agent::exec_shell::ExecShell;

#[test]
fn runs_a_command_and_captures_output() {
    let mut sh = ExecShell::spawn(None, "/tmp", None, &[]).expect("spawn local shell");
    let out = sh.run("echo hello-marsad").expect("run echo");
    assert_eq!(out.trim_end(), "hello-marsad");
}

#[test]
fn captures_stderr_too() {
    let mut sh = ExecShell::spawn(None, "/tmp", None, &[]).expect("spawn");
    let out = sh.run("echo oops 1>&2").expect("run");
    assert_eq!(out.trim_end(), "oops");
}

#[test]
fn state_persists_across_commands() {
    let mut sh = ExecShell::spawn(None, "/tmp", None, &[]).expect("spawn");
    // Exported var set in one command is visible in the next (persistent shell).
    assert_eq!(sh.run("export FOO=bar").expect("export").trim_end(), "");
    assert_eq!(sh.run("echo \"$FOO\"").expect("echo var").trim_end(), "bar");
    // cwd persists too.
    sh.run("cd /").expect("cd");
    assert_eq!(sh.run("pwd").expect("pwd").trim_end(), "/");
}

#[test]
fn setup_is_applied_once_and_visible() {
    // `setup` runs at spawn; its effects (here, an env var) persist for every
    // command WITHOUT being re-run — the whole point of the persistent shell.
    let mut sh =
        ExecShell::spawn(None, "/tmp", Some("export MARSAD_SETUP=on"), &[]).expect("spawn");
    assert_eq!(sh.run("echo \"$MARSAD_SETUP\"").expect("echo").trim_end(), "on");
}

#[test]
fn env_exports_are_applied() {
    let env = vec![("MARSAD_ENV".to_string(), "yes".to_string())];
    let mut sh = ExecShell::spawn(None, "/tmp", None, &env).expect("spawn");
    assert_eq!(sh.run("echo \"$MARSAD_ENV\"").expect("echo").trim_end(), "yes");
}
