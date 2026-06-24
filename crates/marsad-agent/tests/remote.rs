//! Remote-host execution helpers: `exec_prefix_for` (host target → argv prefix)
//! and `build_remote_spawn_cmd` (the FIFO-wired claude invocation run on a remote).

use marsad_agent::{build_remote_spawn_cmd, exec_prefix_for};

// ── exec_prefix_for ─────────────────────────────────────────────────────────────

#[test]
fn none_and_local_aliases_are_local() {
    assert_eq!(exec_prefix_for(None), None);
    assert_eq!(exec_prefix_for(Some("   ")), None); // empty after trim
    assert_eq!(exec_prefix_for(Some("local")), None);
    assert_eq!(exec_prefix_for(Some("localhost")), None);
}

#[test]
fn wsl_target_builds_wsl_prefix() {
    assert_eq!(
        exec_prefix_for(Some("wsl:Ubuntu")),
        Some(vec!["wsl.exe".to_string(), "-d".to_string(), "Ubuntu".to_string()])
    );
}

#[test]
fn wsl_without_distro_is_local() {
    // `wsl:` with no distro is not a usable target → treated as local.
    assert_eq!(exec_prefix_for(Some("wsl:")), None);
}

#[test]
fn ssh_target_builds_full_option_set() {
    let p = exec_prefix_for(Some("user@host")).expect("ssh prefix");
    assert_eq!(p.first().unwrap(), "ssh");
    assert_eq!(p.last().unwrap(), "user@host");
    // The multiplexing + keepalive options from docs/remote-hosts.md are present.
    assert!(p.contains(&"BatchMode=yes".to_string()));
    assert!(p.contains(&"ControlMaster=auto".to_string()));
    assert!(p.contains(&"ControlPath=~/.ssh/marsad-cm-%r@%h:%p".to_string()));
    assert!(p.contains(&"ControlPersist=600".to_string()));
    assert!(p.contains(&"ServerAliveInterval=30".to_string()));
}

#[test]
fn bare_alias_is_treated_as_ssh() {
    let p = exec_prefix_for(Some("etx")).expect("ssh prefix");
    assert_eq!(p.first().unwrap(), "ssh");
    assert_eq!(p.last().unwrap(), "etx");
}

// ── build_remote_spawn_cmd ──────────────────────────────────────────────────────

#[test]
fn remote_spawn_cmd_wires_remote_fifos() {
    let argv = vec![
        "claude".to_string(),
        "--model".to_string(),
        "haiku".to_string(),
    ];
    let cmd = build_remote_spawn_cmd("~/.marsad", "marsad-s1", "/proj", &argv);
    assert_eq!(
        cmd,
        "cd '/proj' && claude '--model' 'haiku' \
         < ~/.marsad/sessions/marsad-s1/in.fifo \
         > ~/.marsad/sessions/marsad-s1/out.fifo \
         2>~/.marsad/sessions/marsad-s1/err.log"
    );
}

#[test]
fn remote_spawn_cmd_quotes_embedded_single_quotes() {
    // Exercises the `'\''` escape in remote_shell_quote.
    let argv = vec!["claude".to_string(), "it's".to_string()];
    let cmd = build_remote_spawn_cmd("~/.marsad", "n", "/p", &argv);
    assert!(cmd.contains(r"'it'\''s'"));
}

#[test]
fn remote_spawn_cmd_handles_no_args_and_empty_argv() {
    // argv with only "claude" → no claude args.
    let only = vec!["claude".to_string()];
    let cmd = build_remote_spawn_cmd("~/.marsad", "n", "/p", &only);
    assert!(cmd.contains("&& claude  <"));
    // Fully empty argv → get(1..) is None → unwrap_or(&[]).
    let empty: Vec<String> = vec![];
    let cmd2 = build_remote_spawn_cmd("~/.marsad", "n", "/p", &empty);
    assert!(cmd2.contains("&& claude  <"));
}
