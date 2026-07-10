//! exec_shell.rs — a PERSISTENT per-session shell for `!` commands.
//!
//! The one-shot `run_remote_script`/local-`sh -c` path re-runs the host's
//! `setup` (e.g. `source ~/module.sh` on an HPC login node) on EVERY `!`,
//! so even a trivial `ls` pays that cost. This spawns a single long-lived shell
//! per session that sources `setup` + exports `env` + `cd`s into the project
//! ONCE, then executes each `!` command over its stdin — so setup is amortized
//! and shell state (cwd, exported vars, history) persists across commands, like
//! a real session shell.
//!
//! Transport: a plain child process — `bash -l` locally, or the session's
//! existing remote prefix (`remote::exec_prefix_for`) + `bash -l -s` — with
//! piped stdin/stdout and stderr folded into stdout (`exec 2>&1` inside the
//! shell). Each command's output is delimited by a unique sentinel line so the
//! caller knows exactly where the command's output ends and can read its exit
//! code. Non-TTY, so there is no prompt echo or ANSI to strip (unlike the pty
//! Terminal tab).
//!
//! Lifetime: tied to the desktop/agent process — NOT durable across app
//! restarts (unlike claude's FIFO session). If the shell dies, the caller
//! respawns it (paying setup once again) or falls back to the one-shot path.
//!
//! Real-IO only (`#[cfg(not(coverage))]`, like `pty.rs`): the pure sentinel/
//! quoting bits could be split out, but the module as a whole is real process IO.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A live persistent shell for one session.
pub struct ExecShell {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    counter: u64,
}

/// POSIX single-quote a value for a safe `export K=V`.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `cd` target with a leading `~` expanded (mirrors the durable-session helper):
/// `cd '~/x'` would look for a literal `~` dir, so map `~`/`~/…` to `$HOME`.
fn cd_target(project: &str) -> String {
    if project == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = project.strip_prefix("~/") {
        format!("\"$HOME/{rest}\"")
    } else {
        sq(project)
    }
}

impl ExecShell {
    /// Spawn the persistent shell for a session and run its one-time init
    /// (stderr→stdout, setup, env exports, cd). Returns the ready shell, or an
    /// error if the child couldn't be spawned / initialized (caller falls back
    /// to the one-shot path).
    ///
    /// `host_target` `None` (or local/unknown) → a local `bash -l`; otherwise
    /// the session's remote prefix + `bash -l -s` (same connection style the
    /// one-shot `run_remote_script` uses, but kept open).
    pub fn spawn(
        host_target: Option<&str>,
        project: &str,
        setup: Option<&str>,
        env: &[(String, String)],
    ) -> std::io::Result<ExecShell> {
        let mut command = match super::remote::exec_prefix_for(host_target) {
            None => {
                // Local shell. bash gives us the POSIX features the protocol uses
                // ($?, printf, exec 2>&1). On a host with no bash this spawn (or
                // the first read) fails and the caller falls back to one-shot.
                let mut c = Command::new("bash");
                c.arg("-l");
                c
            }
            Some(prefix) => {
                let bin = super::remote::resolve_remote_bin(&prefix[0]);
                let mut c = Command::new(&bin);
                c.args(&prefix[1..]).arg("bash").arg("-l").arg("-s");
                c
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no stdout"))?;
        let mut stdout = BufReader::new(stdout);

        // One-time init. Fold stderr into stdout so command errors are captured,
        // then setup + env + cd. A trailing INIT sentinel lets us swallow any
        // setup banner (module-load chatter) so it doesn't leak into the first
        // command's output.
        let mut init = String::new();
        init.push_str("exec 2>&1\n");
        if let Some(s) = setup.filter(|s| !s.trim().is_empty()) {
            init.push_str(s);
            init.push('\n');
        }
        for (k, v) in env {
            init.push_str(&format!("export {}={}\n", k, sq(v)));
        }
        init.push_str(&format!("cd {} 2>/dev/null || true\n", cd_target(project)));
        init.push_str("printf '\\n__MARSAD_EXEC_INIT__\\n'\n");
        stdin.write_all(init.as_bytes())?;
        stdin.flush()?;

        // Drain the init banner up to the INIT sentinel.
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(std::io::Error::other("shell exited during init"));
            }
            if line.contains("__MARSAD_EXEC_INIT__") {
                break;
            }
        }

        Ok(ExecShell {
            child,
            stdin,
            stdout,
            counter: 0,
        })
    }

    /// Run one command and return its combined stdout+stderr (up to, but not
    /// including, the sentinel). Errors if the shell has died (caller respawns /
    /// falls back).
    pub fn run(&mut self, command: &str) -> std::io::Result<String> {
        self.counter += 1;
        let sentinel = format!("__MARSAD_EXEC_{}__", self.counter);
        // The command, then a sentinel carrying its exit code. The leading `\n`
        // guarantees the sentinel starts a fresh line even if the command left
        // the cursor mid-line; `find` below also handles the concatenated case.
        let payload = format!("{command}\nprintf '\\n%s%s\\n' '{sentinel}' \"$?\"\n");
        self.stdin.write_all(payload.as_bytes())?;
        self.stdin.flush()?;

        let mut out = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(std::io::Error::other("shell exited"));
            }
            if let Some(idx) = line.find(&sentinel) {
                out.push_str(&line[..idx]);
                break;
            }
            out.push_str(&line);
        }
        // Drop the single trailing newline the sentinel's leading `\n` added.
        if out.ends_with('\n') {
            out.pop();
            if out.ends_with('\r') {
                out.pop();
            }
        }
        Ok(out)
    }
}

impl Drop for ExecShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
