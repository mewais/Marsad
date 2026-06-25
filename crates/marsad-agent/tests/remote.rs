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

#[test]
fn plink_target_builds_sharing_prefix() {
    // `plink:user@host` → PuTTY connection sharing for the automated connection:
    // `-share` (enable sharing/downstream) + `-batch` (no interactive prompts).
    assert_eq!(
        exec_prefix_for(Some("plink:user@host")),
        Some(vec![
            "plink".to_string(),
            "-share".to_string(),
            "-batch".to_string(),
            "user@host".to_string(),
        ])
    );
}

#[test]
fn plink_target_with_bare_alias() {
    // `plink:<bare-alias>` strips the prefix and keeps the inner saved-session name.
    assert_eq!(
        exec_prefix_for(Some("plink:etx")),
        Some(vec![
            "plink".to_string(),
            "-share".to_string(),
            "-batch".to_string(),
            "etx".to_string(),
        ])
    );
}

#[test]
fn plink_without_inner_is_local() {
    // `plink:` with no inner target is not usable → treated as local.
    assert_eq!(exec_prefix_for(Some("plink:")), None);
}

// ── build_remote_spawn_cmd ──────────────────────────────────────────────────────

#[test]
fn remote_spawn_cmd_is_cd_and_exec_claude() {
    let argv = vec![
        "claude".to_string(),
        "--model".to_string(),
        "haiku".to_string(),
    ];
    let cmd = build_remote_spawn_cmd("/proj", &argv, None, &[]);
    // No redirects / no shell wrapping — just cd + exec <bin> with quoted args.
    // The FIFO wiring is added by spawn_script (which writes this into run.sh).
    assert_eq!(cmd, "cd '/proj' && exec 'claude' '--model' 'haiku'");
}

#[test]
fn remote_spawn_cmd_with_bin_setup_and_env() {
    // Exact output for claude_bin=genie + setup + a single env pair.
    let argv = vec!["genie".to_string(), "--model".to_string(), "haiku".to_string()];
    let env = vec![("A".to_string(), "b".to_string())];
    let cmd = build_remote_spawn_cmd("/proj", &argv, Some("module load x"), &env);
    assert_eq!(
        cmd,
        "module load x\nexport 'A'='b'\ncd '/proj' && exec 'genie' '--model' 'haiku'"
    );
}

#[test]
fn remote_spawn_cmd_expands_tilde_project() {
    let argv = vec!["claude".to_string()];
    // "~/" → cd "$HOME/" (double-quoted so $HOME expands; `cd '~/'` would fail).
    let home = build_remote_spawn_cmd("~/", &argv, None, &[]);
    assert!(home.contains("cd \"$HOME/\" && exec"), "tilde-slash → $HOME: {home}");
    // bare "~" → cd "$HOME"
    let bare = build_remote_spawn_cmd("~", &argv, None, &[]);
    assert!(bare.contains("cd \"$HOME\" && exec"), "bare tilde → $HOME: {bare}");
    // "~/proj" → cd "$HOME/proj"
    let sub = build_remote_spawn_cmd("~/proj", &argv, None, &[]);
    assert!(sub.contains("cd \"$HOME/proj\" && exec"), "tilde subdir: {sub}");
}

#[test]
fn remote_spawn_cmd_empty_setup_is_skipped() {
    // An empty setup string adds no leading line.
    let argv = vec!["claude".to_string()];
    let cmd = build_remote_spawn_cmd("/p", &argv, Some(""), &[]);
    assert_eq!(cmd, "cd '/p' && exec 'claude' ");
}

#[test]
fn remote_spawn_cmd_quotes_embedded_single_quotes() {
    // Exercises the `'\''` escape in remote_shell_quote.
    let argv = vec!["claude".to_string(), "it's".to_string()];
    let cmd = build_remote_spawn_cmd("/p", &argv, None, &[]);
    assert!(cmd.contains(r"'it'\''s'"));
}

#[test]
fn remote_spawn_cmd_handles_no_args_and_empty_argv() {
    // argv with only "claude" → no claude args.
    let only = vec!["claude".to_string()];
    let cmd = build_remote_spawn_cmd("/p", &only, None, &[]);
    assert!(cmd.contains("&& exec 'claude' "));
    // Fully empty argv → get(1..) is None → unwrap_or(&[]); bin defaults to claude.
    let empty: Vec<String> = vec![];
    let cmd2 = build_remote_spawn_cmd("/p", &empty, None, &[]);
    assert!(cmd2.contains("&& exec 'claude' "));
}
