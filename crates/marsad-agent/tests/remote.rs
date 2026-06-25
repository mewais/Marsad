//! Remote-host execution helpers: `exec_prefix_for` (host target → argv prefix),
//! `build_remote_spawn_cmd` (the claude invocation run on a remote), and
//! durable session helpers (design 2B: FIFO + setsid + keepalive).

use marsad_agent::{
    build_remote_spawn_cmd, exec_prefix_for, remote_durable_launch_script, remote_kill_script,
    remote_liveness_script, remote_reader_argv, remote_session_dir, remote_writer_argv,
};

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

// ── Durable remote session helpers (design 2B) ─────────────────────────────────

#[test]
fn remote_session_dir_uses_home_and_quotes_sid() {
    // Session dir should be $HOME/.marsad/<sid>.
    // SIDs with special chars are single-quoted.
    let plain = remote_session_dir("session-1");
    assert!(plain.contains("$HOME/.marsad/"), "contains base dir: {plain}");
    assert!(plain.contains("session-1"), "contains sid: {plain}");

    // A sid with a single quote is escaped via remote_shell_quote.
    let weird = remote_session_dir("sid'x");
    assert!(weird.contains("$HOME/.marsad/"), "contains base dir: {weird}");
    // The single quote is escaped: '\''
    assert!(weird.contains(r"'\''"), "single-quote escaped: {weird}");
}

#[test]
fn remote_durable_launch_script_contains_mkfifo_and_nohup_setsid() {
    let argv = vec![
        "claude".to_string(),
        "--model".to_string(),
        "haiku".to_string(),
    ];
    let run_cmd = build_remote_spawn_cmd("/proj", &argv, None, &[]);
    let script = remote_durable_launch_script("session-42", &run_cmd);

    // Must create the session dir.
    assert!(script.contains("mkdir -p"), "mkdir: {script}");
    // Must create the FIFO.
    assert!(script.contains("mkfifo"), "mkfifo: {script}");
    // Must start keepalive with nohup + setsid.
    assert!(script.contains("nohup"), "nohup: {script}");
    assert!(script.contains("setsid"), "setsid: {script}");
    // Must write pids.
    assert!(script.contains("ka.pid"), "ka.pid: {script}");
    assert!(script.contains("claude.pid"), "claude.pid: {script}");
    // Must include the run_cmd.
    assert!(script.contains("'claude'"), "claude binary: {script}");
    // Must reference the session id in the dir.
    assert!(script.contains("session-42"), "sid in script: {script}");
}

#[test]
fn remote_durable_launch_script_setup_and_env_pass_through() {
    // The run_cmd (from build_remote_spawn_cmd) includes setup and env lines.
    // They are embedded verbatim in the launch script.
    let argv = vec!["genie".to_string(), "--model".to_string(), "haiku".to_string()];
    let env = vec![("FOO".to_string(), "bar".to_string())];
    let run_cmd = build_remote_spawn_cmd("/proj", &argv, Some("module load x"), &env);
    let script = remote_durable_launch_script("sid-env", &run_cmd);
    assert!(script.contains("module load x"), "setup in script: {script}");
    assert!(script.contains("'FOO'"), "env key in script: {script}");
    assert!(script.contains("'bar'"), "env val in script: {script}");
}

#[test]
fn remote_liveness_script_structure() {
    let script = remote_liveness_script("session-99");
    // Must reference the session dir.
    assert!(script.contains("session-99"), "sid: {script}");
    // Must check for a named FIFO.
    assert!(script.contains("-p "), "FIFO check: {script}");
    // Must read the ka.pid file.
    assert!(script.contains("ka.pid"), "ka.pid: {script}");
    // Must print LIVE or DEAD.
    assert!(script.contains("LIVE"), "LIVE output: {script}");
    assert!(script.contains("DEAD"), "DEAD output: {script}");
}

#[test]
fn remote_kill_script_structure() {
    let script = remote_kill_script("session-77");
    // Must reference the session dir.
    assert!(script.contains("session-77"), "sid: {script}");
    // Must read both PID files.
    assert!(script.contains("ka.pid"), "ka.pid: {script}");
    assert!(script.contains("claude.pid"), "claude.pid: {script}");
    // Must kill and remove the dir.
    assert!(script.contains("kill"), "kill: {script}");
    assert!(script.contains("rm -rf"), "rm: {script}");
}

// ── remote_writer_argv / remote_reader_argv ────────────────────────────────────
//
// Both build SHELL-FREE, QUOTE-FREE, $()-FREE argv from a resolved absolute dir
// (+ pid for the reader) — separate tokens so Rust→wsl.exe/ssh.exe can't mangle
// them (the bug that killed durable sessions with "reader EOF after 0 lines").

const ABS_DIR: &str = "/home/u/.marsad/sid-x";

fn assert_no_shell_or_quotes(argv: &[String]) {
    // No `sh -c`, no embedded double-quotes, no command substitution anywhere.
    assert!(
        !argv.iter().any(|a| a == "-c"),
        "must not use a shell wrapper: {argv:?}"
    );
    for a in argv {
        assert!(!a.contains('"'), "no double-quotes allowed: {a:?}");
        assert!(!a.contains("$("), "no command substitution allowed: {a:?}");
        assert!(!a.contains("$HOME"), "no $HOME (use resolved abs dir): {a:?}");
    }
}

#[test]
fn writer_argv_is_shellfree_tee() {
    for target in ["user@host", "wsl:Ubuntu", "plink:etx"] {
        let prefix = exec_prefix_for(Some(target)).expect("prefix");
        let argv = remote_writer_argv(&prefix, ABS_DIR);
        assert!(argv.iter().any(|a| a == "tee"), "tee in argv: {argv:?}");
        assert!(argv.iter().any(|a| a == "-a"), "-a in argv: {argv:?}");
        assert!(
            argv.iter().any(|a| a == &format!("{ABS_DIR}/in.fifo")),
            "literal in.fifo path: {argv:?}"
        );
        assert_no_shell_or_quotes(&argv);
    }
}

#[test]
fn reader_argv_is_shellfree_tail_with_pid() {
    for target in ["user@host", "wsl:Ubuntu", "plink:etx"] {
        let prefix = exec_prefix_for(Some(target)).expect("prefix");
        let argv = remote_reader_argv(&prefix, "12345", ABS_DIR);
        assert!(argv.iter().any(|a| a == "tail"), "tail: {argv:?}");
        assert!(argv.iter().any(|a| a == "-n0"), "-n0: {argv:?}");
        assert!(
            argv.iter().any(|a| a == "--pid=12345"),
            "literal --pid: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == &format!("{ABS_DIR}/out.log")),
            "literal out.log path: {argv:?}"
        );
        assert_no_shell_or_quotes(&argv);
    }
}
