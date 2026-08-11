//! Remote-host execution helpers: `exec_prefix_for` (host target → argv prefix),
//! `build_remote_spawn_cmd` (the claude invocation run on a remote), and
//! durable session helpers (design 2B: FIFO + setsid + keepalive).

use marsad_agent::{
    assistant_has_file_modifying_tool_use, build_remote_spawn_cmd,
    exec_prefix_for, reconnect_backoff_next, remote_durable_launch_script, remote_kill_script,
    remote_liveness_script, remote_reader_argv, remote_session_dir, remote_writer_argv,
    RECONNECT_INITIAL_BACKOFF_SECS, RECONNECT_MAX_BACKOFF_SECS,
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
        Some(vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            "Ubuntu".to_string()
        ])
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

#[test]
fn wslssh_target_builds_wsl_ssh_prefix_with_controlmaster() {
    // `wslssh:<distro>:<target>` → ssh INSIDE the distro, with ControlMaster ALWAYS
    // on (the ssh is Linux-in-WSL), so MFA hosts reuse one interactive login.
    let p = exec_prefix_for(Some("wslssh:Ubuntu:agent33")).expect("wslssh prefix");
    assert_eq!(p[0], "wsl.exe");
    assert_eq!(p[1], "-d");
    assert_eq!(p[2], "Ubuntu");
    assert_eq!(p[3], "ssh");
    assert_eq!(p.last().unwrap(), "agent33");
    assert!(p.contains(&"ControlMaster=auto".to_string()), "{p:?}");
    assert!(
        p.contains(&"ControlPath=~/.ssh/marsad-cm-%r@%h:%p".to_string()),
        "{p:?}"
    );
    assert!(p.contains(&"BatchMode=yes".to_string()), "{p:?}");
}

#[test]
fn wslssh_target_preserves_target_with_user_and_at() {
    // The target after the second colon is kept verbatim (e.g. `user@host`).
    let p = exec_prefix_for(Some("wslssh:Debian:me@h.example.com")).expect("wslssh prefix");
    assert_eq!(p[2], "Debian");
    assert_eq!(p.last().unwrap(), "me@h.example.com");
}

#[test]
fn wslssh_missing_distro_or_target_is_local() {
    assert_eq!(exec_prefix_for(Some("wslssh:Ubuntu:")), None); // no target
    assert_eq!(exec_prefix_for(Some("wslssh::agent33")), None); // no distro
    assert_eq!(exec_prefix_for(Some("wslssh:")), None); // neither
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
    let argv = vec![
        "genie".to_string(),
        "--model".to_string(),
        "haiku".to_string(),
    ];
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
    assert!(
        home.contains("cd \"$HOME/\" && exec"),
        "tilde-slash → $HOME: {home}"
    );
    // bare "~" → cd "$HOME"
    let bare = build_remote_spawn_cmd("~", &argv, None, &[]);
    assert!(
        bare.contains("cd \"$HOME\" && exec"),
        "bare tilde → $HOME: {bare}"
    );
    // "~/proj" → cd "$HOME/proj"
    let sub = build_remote_spawn_cmd("~/proj", &argv, None, &[]);
    assert!(
        sub.contains("cd \"$HOME/proj\" && exec"),
        "tilde subdir: {sub}"
    );
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
    assert!(
        plain.contains("$HOME/.marsad/"),
        "contains base dir: {plain}"
    );
    assert!(plain.contains("session-1"), "contains sid: {plain}");

    // A sid with a single quote is escaped via remote_shell_quote.
    let weird = remote_session_dir("sid'x");
    assert!(
        weird.contains("$HOME/.marsad/"),
        "contains base dir: {weird}"
    );
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
    // claude.pid MUST be (re)written from inside start.sh as `$$` — the real pid
    // surviving the sh→bash→claude exec chain. Relying on the launcher's `$!`
    // (the setsid wrapper, which can fork+exit immediately) made `tail --pid` quit
    // instantly and liveness report DEAD on a live claude.
    assert!(
        script.contains("echo $$ > \"%s/claude.pid\""),
        "start.sh must capture the real claude pid via $$: {script}"
    );
    // Must include the run_cmd.
    assert!(script.contains("'claude'"), "claude binary: {script}");
    // run.sh MUST be launched via a LOGIN shell so the user's PATH (e.g.
    // ~/.local/bin/claude, or a wrapper's module env) is available — a non-login
    // `sh` would not find claude and the session would die on startup.
    assert!(
        script.contains("bash -l \"%s/run.sh\"") || script.contains("bash -l"),
        "run.sh must run via a login shell: {script}"
    );
    // Must reference the session id in the dir.
    assert!(script.contains("session-42"), "sid in script: {script}");
}

#[test]
fn remote_durable_launch_script_setup_and_env_pass_through() {
    // The run_cmd (from build_remote_spawn_cmd) includes setup and env lines.
    // They are embedded verbatim in the launch script.
    let argv = vec![
        "genie".to_string(),
        "--model".to_string(),
        "haiku".to_string(),
    ];
    let env = vec![("FOO".to_string(), "bar".to_string())];
    let run_cmd = build_remote_spawn_cmd("/proj", &argv, Some("module load x"), &env);
    let script = remote_durable_launch_script("sid-env", &run_cmd);
    assert!(
        script.contains("module load x"),
        "setup in script: {script}"
    );
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
    // Must ALSO require the claude process alive (not just the keepalive) —
    // prevents reconnecting to a zombie session whose claude has exited.
    assert!(script.contains("claude.pid"), "claude.pid: {script}");
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
    // Graceful drain: SIGTERM, then WAIT for claude to exit (kill -0 poll) BEFORE
    // escalating to SIGKILL, so it can flush its transcript .jsonl. A bare 0.2s
    // sleep then kill -9 (the old behavior) dropped the last turn(s) on disk.
    assert!(script.contains("kill -0"), "waits for exit before SIGKILL: {script}");
    assert!(script.contains("kill -9"), "escalates to SIGKILL: {script}");
    assert!(!script.contains("sleep 0.2"), "no fixed 0.2s pre-SIGKILL sleep: {script}");
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
        assert!(
            !a.contains("$HOME"),
            "no $HOME (use resolved abs dir): {a:?}"
        );
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
        let argv = remote_reader_argv(&prefix, "12345", ABS_DIR, false);
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

#[test]
fn reader_argv_from_start_reads_whole_file_not_n0() {
    // opencode first-attach must read from line 1 so the one-shot ACP handshake
    // responses aren't missed by a late-attaching remote tail.
    for target in ["user@host", "wsl:Ubuntu", "plink:etx"] {
        let prefix = exec_prefix_for(Some(target)).expect("prefix");
        let argv = remote_reader_argv(&prefix, "12345", ABS_DIR, true);
        assert!(argv.iter().any(|a| a == "tail"), "tail: {argv:?}");
        // -n +1 (whole file), NOT -n0.
        assert!(argv.iter().any(|a| a == "-n"), "-n flag: {argv:?}");
        assert!(argv.iter().any(|a| a == "+1"), "+1 offset: {argv:?}");
        assert!(!argv.iter().any(|a| a == "-n0"), "must NOT use -n0: {argv:?}");
        assert!(argv.iter().any(|a| a == "--pid=12345"), "literal --pid: {argv:?}");
        assert!(argv.iter().any(|a| a == "-f"), "-f follow: {argv:?}");
        assert_no_shell_or_quotes(&argv);
    }
}

// ── assistant_has_file_modifying_tool_use (#4 Changes probe trigger) ───────────
//
// Pure-logic function: tested here so it gets 100% line+branch coverage and the
// debounced Changes probe is verified to fire on the right tools only.

fn make_assistant_event(tool_name: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": tool_name,
                    "id": "tu_123",
                    "input": {}
                }
            ]
        }
    })
}

/// Each file-modifying tool triggers the probe.
#[test]
fn file_modifying_tools_return_true() {
    for tool in ["Bash", "Edit", "Write", "NotebookEdit", "MultiEdit"] {
        let ev = make_assistant_event(tool);
        assert!(
            assistant_has_file_modifying_tool_use(&ev),
            "tool {tool} should be recognised as file-modifying"
        );
    }
}

/// Read-only tools must NOT trigger the probe.
#[test]
fn read_only_tools_return_false() {
    for tool in ["Read", "Grep", "Glob", "WebSearch", "WebFetch", "Task"] {
        let ev = make_assistant_event(tool);
        assert!(
            !assistant_has_file_modifying_tool_use(&ev),
            "tool {tool} should NOT be recognised as file-modifying"
        );
    }
}

/// A non-assistant event (e.g. `result`) must never match.
#[test]
fn non_assistant_event_returns_false() {
    let ev = serde_json::json!({"type": "result", "subtype": "success"});
    assert!(!assistant_has_file_modifying_tool_use(&ev));
}

/// An assistant event with NO content blocks must not match.
#[test]
fn assistant_with_no_content_returns_false() {
    let ev = serde_json::json!({"type": "assistant", "message": {"content": []}});
    assert!(!assistant_has_file_modifying_tool_use(&ev));
}

/// An assistant event with only text blocks (no tool_use) must not match.
#[test]
fn assistant_with_only_text_blocks_returns_false() {
    let ev = serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "here's my plan"}]
        }
    });
    assert!(!assistant_has_file_modifying_tool_use(&ev));
}

/// Mixed content: text block + file-modifying tool_use → true.
#[test]
fn assistant_mixed_content_text_and_write_returns_true() {
    let ev = serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": "writing now"},
                {"type": "tool_use", "name": "Write", "id": "tu_1", "input": {}}
            ]
        }
    });
    assert!(assistant_has_file_modifying_tool_use(&ev));
}

/// Missing `message` field → false (not a panic).
#[test]
fn assistant_missing_message_returns_false() {
    let ev = serde_json::json!({"type": "assistant"});
    assert!(!assistant_has_file_modifying_tool_use(&ev));
}

/// `tool_use` block with no `name` field → false (not a panic).
#[test]
fn tool_use_missing_name_returns_false() {
    let ev = serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "tool_use", "id": "tu_1", "input": {}}]
        }
    });
    assert!(!assistant_has_file_modifying_tool_use(&ev));
}

// ── reconnect_backoff_next ─────────────────────────────────────────────────────
//
// Pure-logic exponential backoff: doubles the current delay, capped at
// RECONNECT_MAX_BACKOFF_SECS.  Tested so it gets 100% coverage and the
// reconnect supervisor is verified to never busy-loop or exceed the cap.

/// Doubling stays below the cap: 3 → 6 → 12 → 24 → 30 (capped).
#[test]
fn backoff_doubles_and_caps() {
    assert_eq!(reconnect_backoff_next(RECONNECT_INITIAL_BACKOFF_SECS), 6);
    assert_eq!(reconnect_backoff_next(6), 12);
    assert_eq!(reconnect_backoff_next(12), 24);
    // 24*2 = 48 > 30, so it should be capped at RECONNECT_MAX_BACKOFF_SECS.
    assert_eq!(
        reconnect_backoff_next(24),
        RECONNECT_MAX_BACKOFF_SECS,
        "backoff must be capped at RECONNECT_MAX_BACKOFF_SECS"
    );
    // Already at cap → stays at cap.
    assert_eq!(
        reconnect_backoff_next(RECONNECT_MAX_BACKOFF_SECS),
        RECONNECT_MAX_BACKOFF_SECS
    );
}

/// Zero input → 0 (no infinite loop from a zero initial value).
#[test]
fn backoff_from_zero_stays_zero() {
    assert_eq!(reconnect_backoff_next(0), 0);
}

/// Large input saturates to RECONNECT_MAX_BACKOFF_SECS without overflow.
#[test]
fn backoff_saturates_without_overflow() {
    assert_eq!(
        reconnect_backoff_next(u64::MAX / 2 + 1),
        RECONNECT_MAX_BACKOFF_SECS
    );
    assert_eq!(reconnect_backoff_next(u64::MAX), RECONNECT_MAX_BACKOFF_SECS);
}

// ── build_remote_codex_eval_loop ──────────────────────────────────────────────
//
// Pure-logic builder: tested here at 100% coverage.  The eval-loop is the
// long-lived "process" for a remote codex durable session — it reads one
// `codex_turn_cmd` line at a time from in.fifo and `eval`s it, producing JSONL
// on stdout (which start.sh captures via >> out.log).







