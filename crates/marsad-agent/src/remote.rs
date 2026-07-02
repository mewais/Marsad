//! SSH/WSL/plink remote-host execution machinery: resolving a `host_target`
//! into an argv prefix, running scripts over that prefix, and building the
//! shell scripts + argv vectors for the durable remote session model (design
//! 2B: FIFO + setsid + keepalive). See `docs/remote-hosts.md`.

// ── remote-host execution helpers ───────────────────────────────────────────────

/// Remote home-relative runtime dir where a remote host keeps Marsad session
/// state (the FIFOs). The remote POSIX shell expands `~`. Only the real-IO path
/// builds remote containers, so this is absent from the coverage build.
#[cfg(not(coverage))]
pub(crate) const REMOTE_RUNTIME_DIR: &str = "$HOME/.marsad";

/// POSIX single-quote for remote shell commands (the remote is always a POSIX
/// `sh`, regardless of the local platform — so this is not `cfg(unix)`-gated).
fn remote_shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve the `plink` executable to a concrete path.
///
/// `exec_prefix_for` returns the sentinel `"plink"` for PuTTY hosts, but PuTTY's
/// installer does not always add itself to `PATH` (and some users only have an
/// old `plink` bundled with another tool, e.g. Xming). So at spawn time we look
/// for `plink.exe` on `PATH` first, then in the standard PuTTY install dirs
/// (preferring a modern PuTTY over an Xming-bundled fallback). Any other binary
/// (`ssh`, `wsl.exe`) is returned unchanged — those are reliably on `PATH`.
/// Gated out of the coverage build (filesystem/env probing, real-IO only).
#[cfg(not(coverage))]
pub(crate) fn resolve_remote_bin(argv0: &str) -> String {
    if argv0 != "plink" {
        return argv0.to_string();
    }
    #[cfg(windows)]
    {
        if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(';').filter(|d| !d.is_empty()) {
                let cand = std::path::Path::new(dir).join("plink.exe");
                if cand.exists() {
                    return cand.to_string_lossy().into_owned();
                }
            }
        }
        for cand in [
            r"C:\Program Files\PuTTY\plink.exe",
            r"C:\Program Files (x86)\PuTTY\plink.exe",
            r"C:\Program Files (x86)\Xming\plink.exe",
        ] {
            if std::path::Path::new(cand).exists() {
                return cand.to_string();
            }
        }
    }
    "plink".to_string()
}

/// Build the remote shell script that prints a session's transcript `.jsonl`.
///
/// Claude names its `projects/<dir>` after the session's REAL working directory
/// (every non-alphanumeric char → `-`), so we must encode the cwd Claude
/// actually used — not the raw `project` string, which may be `~` or relative.
/// The script therefore resolves the cwd ON THE REMOTE (`cd <target> && pwd`)
/// and encodes it there with `sed`, matching `encode_project_dir` exactly. With
/// a known claude uuid it cats that file; for `--continue` (no uuid) it cats the
/// most-recently-modified `.jsonl`. Run via `run_remote_script` (`bash -l -s`),
/// so `$HOME`/`~` resolve via the login shell. Real-IO build only.
#[cfg(not(coverage))]
pub(crate) fn remote_transcript_script(project: &str, uuid: Option<&str>) -> String {
    let cd = cd_target(project);
    // Honor CLAUDE_CONFIG_DIR (wrappers like `genie` relocate Claude's config +
    // transcripts off ~/.claude), falling back to ~/.claude. The login shell
    // (`bash -l -s`) has the wrapper's env, so $CLAUDE_CONFIG_DIR is set there.
    let resolve = format!(
        "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}; \
         cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0; \
         enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g'); \
         dir=\"$base/projects/$enc\"; "
    );
    // Cap at the last ~1 MiB: full Claude `.jsonl` transcripts can be tens of MB
    // (embedded tool outputs), too big to ship as one IPC line and render. `tail
    // -c` keeps the most recent turns; the leading partial line is skipped by
    // `parse_transcript` (invalid JSON). Small transcripts are returned whole.
    match uuid {
        Some(u) if !u.is_empty() => {
            format!("{resolve}tail -c 1048576 \"$dir/{u}.jsonl\" 2>/dev/null")
        }
        _ => format!(
            "{resolve}f=$(ls -t \"$dir\"/*.jsonl 2>/dev/null | head -1); \
             [ -n \"$f\" ] && tail -c 1048576 \"$f\""
        ),
    }
}

/// Build the remote shell script that aggregates project-wide per-model token
/// usage across ALL `*.jsonl` files in the claude projects directory for a
/// session's project.  Uses python3 to sum `message.usage` fields grouped by
/// `message.model` across every `assistant`-typed line in every `.jsonl` file,
/// skipping the `<synthetic>` pseudo-model.  Prints one compact JSON object:
/// `{"<model>":{"input":N,"output":N,"cache_read":N,"cache_creation":N},...}`.
///
/// Run via `run_remote_script` on the `is_result` trigger (same branch as the
/// ChangesUpdate probe).  Guard: exit 0 silently if python3 is not available.
/// Real-IO build only.
#[cfg(not(coverage))]
pub(crate) fn remote_usage_script(project: &str) -> String {
    let cd = cd_target(project);
    format!(
        r#"command -v python3 >/dev/null || exit 0
base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}
cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0
enc=$(printf '%s' "$cwd" | sed 's/[^a-zA-Z0-9]/-/g')
dir="$base/projects/$enc"
python3 - "$dir" << 'MARSAD_USAGE_EOF'
import sys, os, json
d = sys.argv[1]
agg = {{}}
if not os.path.isdir(d):
    print('{{}}')
    sys.exit(0)
for fn in os.listdir(d):
    if not fn.endswith('.jsonl'):
        continue
    try:
        with open(os.path.join(d, fn), encoding='utf-8', errors='replace') as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                if obj.get('type') != 'assistant':
                    continue
                msg = obj.get('message', {{}})
                model = msg.get('model', '')
                if not model or model == '<synthetic>':
                    continue
                u = msg.get('usage', {{}})
                inp = u.get('input_tokens', 0) or 0
                out = u.get('output_tokens', 0) or 0
                cr  = u.get('cache_read_input_tokens', 0) or 0
                cc  = u.get('cache_creation_input_tokens', 0) or 0
                if inp == 0 and out == 0 and cr == 0 and cc == 0:
                    continue
                if model not in agg:
                    agg[model] = {{'input': 0, 'output': 0, 'cache_read': 0, 'cache_creation': 0}}
                agg[model]['input'] += inp
                agg[model]['output'] += out
                agg[model]['cache_read'] += cr
                agg[model]['cache_creation'] += cc
    except Exception:
        pass
print(json.dumps(agg, separators=(',', ':')))
MARSAD_USAGE_EOF
"#,
        cd = cd,
    )
}

/// Build the remote shell script that probes `git status` + `diff --numstat` for
/// a session's project dir on the remote host.
///
/// Prints one TSV line per changed file: `status\tadded\tremoved\tpath`.
/// The porcelain lines supply status+path; numstat supplies added/removed for
/// tracked changes (untracked → 0/0). Run via `run_remote_script`. Real-IO only.
#[cfg(not(coverage))]
pub(crate) fn remote_changes_script(project: &str) -> String {
    let cd = cd_target(project);
    format!(
        r#"cd {cd} 2>/dev/null || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0
git status --porcelain=v1 | while IFS= read -r line; do
  st=$(printf '%s' "$line" | cut -c1-2 | tr -d ' '); f=$(printf '%s' "$line" | cut -c4-)
  printf '%s\t0\t0\t%s\n' "$st" "$f"
done
git diff --numstat
git diff --numstat --cached
"#,
        cd = cd
    )
}

/// Parse the stdout of `remote_changes_script` into a JSON `ChangesUpdate`
/// `changes` array. Lines from `git status --porcelain` contribute the status
/// and path; lines from `git diff --numstat` contribute the added/removed counts.
/// Returns the JSON array value (may be empty). Real-IO only.
#[cfg(not(coverage))]
pub(crate) fn parse_remote_changes(stdout: &str) -> serde_json::Value {
    use std::collections::HashMap;

    // Separate porcelain lines (tab-separated: `status\t0\t0\tpath`) from
    // numstat lines (`added\tremoved\tpath`). We distinguish them by the second
    // column: porcelain lines always have "0" there; numstat lines have a number
    // or "-" (binary). Lines that parse as neither are silently ignored.
    let mut statuses: Vec<(String, String)> = Vec::new(); // (status, path)
    let mut numstats: HashMap<String, (u64, u64)> = HashMap::new(); // path → (added, removed)

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        match parts.as_slice() {
            [st, "0", "0", path] => {
                // Porcelain line from the `while` loop.
                if !st.is_empty() && !path.is_empty() {
                    statuses.push((st.to_string(), path.to_string()));
                }
            }
            [added_s, removed_s, path] => {
                // numstat line: `added\tremoved\tpath`.
                let added = added_s.parse::<u64>().unwrap_or(0);
                let removed = removed_s.parse::<u64>().unwrap_or(0);
                if !path.is_empty() {
                    // Later numstat lines (staged diff) may overwrite earlier ones;
                    // we take the max to surface both unstaged + staged counts.
                    let entry = numstats.entry(path.to_string()).or_insert((0, 0));
                    entry.0 = entry.0.max(added);
                    entry.1 = entry.1.max(removed);
                }
            }
            _ => {}
        }
    }

    let items: Vec<serde_json::Value> = statuses
        .into_iter()
        .map(|(status, path)| {
            let (added, removed) = numstats.get(&path).copied().unwrap_or((0, 0));
            serde_json::json!({
                "path": path,
                "status": status,
                "added": added,
                "removed": removed,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

/// Run a shell `script` on a remote host and capture its stdout.
///
/// The script is fed via the child's STDIN to `bash -l -s` (login shell reading
/// from stdin), NOT embedded in argv — `wsl.exe` and `ssh` both re-parse their
/// command tail, which mangles a script full of quotes/`$()`/backslashes (the
/// cause of the empty remote listing). Piping via stdin sidesteps all argv
/// quoting. `CREATE_NO_WINDOW` stops `ssh.exe` from flashing a console on
/// Windows. stdin is closed after writing so the remote `bash -s` sees EOF and
/// runs (otherwise it hangs waiting for more input). `None` for a local/unknown
/// host or a spawn failure. Real-IO build only.
#[cfg(not(coverage))]
pub fn run_remote_script(host_target: &str, script: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let prefix = exec_prefix_for(Some(host_target))?;
    let bin = resolve_remote_bin(&prefix[0]);
    let mut command = Command::new(&bin);
    command
        .args(&prefix[1..])
        .arg("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    {
        // Write the script then drop stdin → EOF so `bash -s` executes and exits.
        let mut stdin = child.stdin.take()?;
        stdin.write_all(script.as_bytes()).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`run_remote_script`] but kills the child if it runs longer than
/// `timeout`, returning `None`. Used for STARTUP liveness probes: a host that
/// needs interactive MFA (Duo) and has no live ControlMaster yet will sit at the
/// prompt forever, and the old unbounded probe — run sequentially on the serve
/// thread before the command loop starts — froze the ENTIRE agent (no launches,
/// blank UI) whenever one persisted session lived on such a host. Bounding the
/// probe lets startup treat a slow/blocked host as DEAD and move on. Intended for
/// SMALL outputs (it reads stdout only after the child exits). Real-IO build only.
#[cfg(not(coverage))]
pub fn run_remote_script_bounded(
    host_target: &str,
    script: &str,
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let prefix = exec_prefix_for(Some(host_target))?;
    let bin = resolve_remote_bin(&prefix[0]);
    let mut command = Command::new(&bin);
    command
        .args(&prefix[1..])
        .arg("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    {
        let mut stdin = child.stdin.take()?;
        stdin.write_all(script.as_bytes()).ok()?;
    } // stdin dropped → EOF so `bash -s` runs and exits.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    let _ = so.read_to_string(&mut out);
                }
                return Some(out);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

/// Resolve a durable remote session's ABSOLUTE dir + the detached claude pid,
/// read over the host's connection via the reliable stdin-pipe `run_remote_script`
/// (login shell, so `$HOME` resolves). Returns `(pid, abs_dir)`, or `None` if the
/// session dir is absent or `claude.pid` is empty (claude isn't running). These
/// literals are then baked into the writer/reader argv so those commands carry NO
/// `$HOME`/`$()`/quotes — the form that survives Rust→wsl.exe/ssh.exe. Real-IO only.
#[cfg(not(coverage))]
pub(crate) fn remote_resolve_pid_dir(host_target: &str, sid: &str) -> Option<(String, String)> {
    let dir = remote_session_dir(sid);
    let script =
        format!("d={dir}; cd \"$d\" 2>/dev/null && pwd; cat \"$d/claude.pid\" 2>/dev/null");
    let out = run_remote_script(host_target, &script)?;
    let mut lines = out.lines();
    let abs_dir = lines.next().unwrap_or("").trim().to_string();
    let pid = lines.next().unwrap_or("").trim().to_string();
    if abs_dir.is_empty() || pid.is_empty() {
        return None;
    }
    Some((pid, abs_dir))
}

/// List a remote host's resumable Claude sessions for `project`.
///
/// Claude stores transcripts on the REMOTE host, so the desktop's local JSONL
/// reader can't see them — the New Session "Resume" picker would only ever offer
/// "Continue". This runs, over the host's own connection (reusing
/// ControlMaster / plink-share), a script that resolves the real cwd, finds the
/// project's `.jsonl` files newest-first, and prints one
/// `mtime_ms<TAB>uuid<TAB><first user line JSON>` record per session. The caller
/// parses each record into a `SessionSummary` (the snippet via
/// `discovery::first_user_prompt` on the raw line). Returns the raw stdout, or
/// `None` for a local/unknown host or a failed connection. Real-IO build only.
#[cfg(not(coverage))]
pub fn fetch_remote_session_list(host_target: &str, project: &str) -> Option<String> {
    let cd = cd_target(project);
    let script = format!(
        "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\n\
         cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0\n\
         enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g')\n\
         dir=\"$base/projects/$enc\"\n\
         for f in $(ls -t \"$dir\"/*.jsonl 2>/dev/null); do \
           u=$(basename \"$f\" .jsonl); \
           m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
           l=$(grep -m1 '\"type\":\"user\"' \"$f\" 2>/dev/null | head -c 4000); \
           printf '%s000\\t%s\\t%s\\n' \"$m\" \"$u\" \"$l\"; \
         done"
    );
    run_remote_script(host_target, &script)
}

/// Resolve a host's execution target into the argv prefix every remote command is
/// run through (see docs/remote-hosts.md). Returns:
/// - `None` for the local machine (`None` / `"local"` / `"localhost"` / empty),
/// - an `ssh` prefix (with the standard option set) for `user@host` or a bare alias,
/// - a `wsl.exe` prefix for `wsl:<distro>`,
/// - a `plink` prefix (PuTTY connection sharing) for `plink:<ssh-target>`.
pub fn exec_prefix_for(host_target: Option<&str>) -> Option<Vec<String>> {
    let t = host_target.map(str::trim).filter(|s| !s.is_empty())?;
    if t == "local" || t == "localhost" {
        return None;
    }
    if let Some(distro) = t.strip_prefix("wsl:") {
        if distro.is_empty() {
            return None;
        }
        return Some(vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            distro.to_string(),
        ]);
    }
    // WSL-routed SSH: run `ssh` INSIDE a WSL distro so it gets real ControlMaster
    // (interactive MFA login once → automated connections reuse the master socket),
    // using THAT distro's `~/.ssh` config + keys. This is the Windows answer for
    // Duo/MFA hosts when the user's SSH lives in WSL — native `ssh.exe` can't do
    // ControlMaster. Format: `wslssh:<distro>:<ssh-target>` (the target is an alias
    // or `user@host` resolved by the distro's ssh, NOT by Windows). ControlMaster
    // is ALWAYS applied here (the ssh binary is Linux-in-WSL, where it works),
    // unlike the native-ssh branch which gates it on `cfg(unix)`.
    if let Some(rest) = t.strip_prefix("wslssh:") {
        let mut parts = rest.splitn(2, ':');
        let distro = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        if distro.is_empty() || target.is_empty() {
            return None;
        }
        let opt = |k: &str| ["-o".to_string(), k.to_string()];
        let mut argv = vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            distro.to_string(),
            "ssh".to_string(),
            "-T".to_string(),
        ];
        argv.extend(opt("BatchMode=yes"));
        argv.extend(opt("ConnectTimeout=10"));
        argv.extend(opt("ServerAliveInterval=30"));
        argv.extend(opt("ServerAliveCountMax=3"));
        argv.extend(opt("TCPKeepAlive=yes"));
        argv.extend(opt("ControlMaster=auto"));
        argv.extend(opt("ControlPath=~/.ssh/marsad-cm-%r@%h:%p"));
        argv.extend(opt("ControlPersist=600"));
        argv.push(target.to_string());
        return Some(argv);
    }
    // PuTTY plink with SSH connection sharing — the Windows alternative to
    // ControlMaster (which `ssh.exe` can't do: `getsockname failed: Not a socket`).
    // The interactive login (bridge.rs) runs `plink -share <inner>` and becomes the
    // sharing "upstream"; these automated connections add `-batch` (disable
    // interactive prompts) and `-share` (downstream onto that upstream) so a Duo/MFA
    // host authenticates once then is reused. Flags per the PuTTY manual: `-share`
    // enable connection sharing, `-batch` disable interactive prompts. ControlMaster
    // ssh options do NOT apply to plink and are intentionally omitted.
    if let Some(inner) = t.strip_prefix("plink:") {
        if inner.is_empty() {
            return None;
        }
        return Some(vec![
            "plink".to_string(),
            "-share".to_string(),
            "-batch".to_string(),
            inner.to_string(),
        ]);
    }
    // SSH host. BatchMode for the automated connection; ControlMaster multiplexing
    // lets it reuse an interactive login's master socket (MFA hosts); keepalive
    // stops idle drops. Mirrors the SSH option set in docs/remote-hosts.md.
    let opt = |k: &str| ["-o".to_string(), k.to_string()];
    let mut argv = vec!["ssh".to_string()];
    // `-T`: no PTY. On Windows `ssh.exe` otherwise allocates a ConPTY that flashes a
    // console window; our remote commands (tmux control, `cat` FIFO bridging) never
    // need a TTY, so disable it everywhere.
    argv.push("-T".to_string());
    argv.extend(opt("BatchMode=yes"));
    argv.extend(opt("ConnectTimeout=10"));
    argv.extend(opt("ServerAliveInterval=30"));
    argv.extend(opt("ServerAliveCountMax=3"));
    argv.extend(opt("TCPKeepAlive=yes"));
    // ControlMaster multiplexing (reuse an interactive login's master socket) needs
    // Unix-domain sockets, which Windows OpenSSH lacks — there it errors `getsockname
    // failed: Not a socket`. Enable only when the agent runs on a Unix host.
    #[cfg(unix)]
    {
        argv.extend(opt("ControlMaster=auto"));
        argv.extend(opt("ControlPath=~/.ssh/marsad-cm-%r@%h:%p"));
        argv.extend(opt("ControlPersist=600"));
    }
    argv.push(t.to_string());
    Some(argv)
}

/// Build the `claude` invocation written into the remote `run.sh` (which the remote
/// tmux session runs via `bash -l run.sh < in.fifo > out.fifo 2>err.log`). Returns
/// just `cd '<project>' && exec claude '<arg>'…` — NO redirections and NO nested
/// shell wrapping. The FIFO wiring lives in `spawn_script`, and the command body goes
/// into run.sh through a heredoc, so it's quoted exactly ONCE (the old approach nested
/// `bash -lc` inside tmux inside `bash -lc` and produced mangled quoting → claude
/// never ran).
pub fn build_remote_spawn_cmd(
    project: &str,
    argv: &[String],
    setup: Option<&str>,
    env: &[(String, String)],
) -> String {
    let bin = argv.first().map(String::as_str).unwrap_or("claude");
    let claude_args = argv.get(1..).unwrap_or(&[]);
    let mut body = String::new();
    // Optional setup command, run before claude.
    if let Some(s) = setup.filter(|s| !s.is_empty()) {
        body.push_str(s);
        body.push('\n');
    }
    // One `export 'K'='V'` line per env pair (key and value POSIX single-quoted
    // via `remote_shell_quote`).
    for (k, v) in env {
        body.push_str(&format!(
            "export {}={}\n",
            remote_shell_quote(k),
            remote_shell_quote(v)
        ));
    }
    body.push_str(&format!(
        "cd {} && exec {} {}",
        cd_target(project),
        remote_shell_quote(bin),
        claude_args
            .iter()
            .map(|a| remote_shell_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    ));
    body
}

/// Shell-quote a project path for `cd`, expanding a leading `~`. Single-quoting a
/// path like `~/` (as `remote_shell_quote` would) makes `cd '~/'` look for a literal
/// `~` directory → "No such file or directory". Map a leading `~`/`~/…` to `$HOME`
/// inside DOUBLE quotes so it expands; quote everything else literally.
fn cd_target(project: &str) -> String {
    if project == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = project.strip_prefix("~/") {
        // "$HOME/<rest>" — double-quoted so $HOME expands and spaces are safe.
        format!("\"$HOME/{rest}\"")
    } else {
        remote_shell_quote(project)
    }
}

// ── Durable remote session helpers ────────────────────────────────────────────
//
// These functions build the shell scripts and argv vectors for the durable
// remote session model (design 2B: FIFO + setsid + keepalive).  Pure logic
// (no IO), so they are NOT gated on `cfg(not(coverage))` and are always
// coverage-tested via the new tests in `tests/remote.rs`.

/// Return the remote dir path for a session (`$HOME/.marsad/<sid>`).
///
/// The shell literal `$HOME` is used (not expanded at build time) so it
/// resolves correctly on the remote host regardless of the local platform.
pub fn remote_session_dir(sid: &str) -> String {
    format!("$HOME/.marsad/{}", remote_shell_quote(sid))
}

/// Build the shell script that launches a NEW durable remote session.
///
/// Run via `bash -l -s` on the remote. Steps:
///
/// 1. Create `$HOME/.marsad/<sid>/` and a named FIFO `in.fifo`.
/// 2. Start a keepalive holder (`nohup setsid sleep 999999 > in.fifo`) that keeps
///    the FIFO write-end open so claude never sees EOF when the agent disconnects.
/// 3. Start the detached claude (`nohup setsid sh -c '<run_cmd> < in.fifo >>
///    out.log 2>&1'`). `nohup` + `setsid` detach it from the ssh session.
/// 4. Write `ka.pid` (the keepalive `$!`) and `claude.pid`.
///
/// `claude.pid` is written from INSIDE `start.sh` as `$$` — the REAL claude pid.
/// The exec chain `start.sh (sh) → exec bash -l run.sh → exec claude` keeps one
/// pid throughout, so `$$` captured at the top of `start.sh` IS claude's pid. We
/// must NOT rely on the launcher's `$!` (the `nohup setsid` wrapper): `setsid`
/// forks on some systems and the wrapper then exits immediately, leaving `$!`
/// pointing at a dead pid. That bug made `tail --pid=<claude.pid>` quit instantly
/// ("reader EOF after 0 lines") and `remote_liveness_script` report DEAD on a
/// claude that was actually alive — breaking launch display AND resume. The
/// launcher still seeds `claude.pid` with `$!` as a placeholder; `start.sh`
/// overwrites it with `$$`, and a short `sleep` lets that land before the reader
/// resolves the pid.
///
/// `run_cmd` is the `build_remote_spawn_cmd` output (`cd '<proj>' && exec
/// 'claude' <args>`); it is embedded verbatim — no extra shell quoting.
pub fn remote_durable_launch_script(sid: &str, run_cmd: &str) -> String {
    let dir = remote_session_dir(sid);
    // The run_cmd comes from build_remote_spawn_cmd which already applies
    // remote_shell_quote to all paths and args.  Embedding it inside a
    // `sh -c '...'` would create nested quoting problems (the run_cmd uses
    // POSIX single-quoting internally).  We instead write it to `run.sh` via
    // a heredoc (single-quoted marker `'MARSAD_RUN'` prevents `$`/`\`
    // expansion inside the body, so the run_cmd is safe).  A separate
    // `start.sh` wrapper execs `run.sh` with the FIFO redirections — this
    // avoids passing the dir path through any additional quoting layer and
    // lets `nohup setsid` operate on a simple executable path.
    format!(
        r#"set -e
dir={dir}
mkdir -p "$dir"
rm -f "$dir/in.fifo"
mkfifo "$dir/in.fifo"
touch "$dir/out.log"
cat > "$dir/run.sh" << 'MARSAD_RUN'
{run_cmd}
MARSAD_RUN
printf '#!/bin/sh\necho $$ > "%s/claude.pid"\nexec bash -l "%s/run.sh" < "%s/in.fifo" >> "%s/out.log" 2>&1\n' "$dir" "$dir" "$dir" "$dir" > "$dir/start.sh"
chmod +x "$dir/run.sh" "$dir/start.sh"
nohup setsid sh -c 'exec sleep 999999 > '"$dir/in.fifo"'' </dev/null >/dev/null 2>/dev/null &
echo $! > "$dir/ka.pid"
sleep 0.1
nohup setsid "$dir/start.sh" </dev/null >/dev/null 2>/dev/null &
echo $! > "$dir/claude.pid"
sleep 0.3
echo "marsad-durable-launch: sid={sid} dir=$dir"
"#,
        dir = dir,
        run_cmd = run_cmd,
        sid = sid,
    )
}

/// Build the shell script that checks whether a durable remote session is live.
///
/// Returns `LIVE` on stdout if ALL of:
///  - the session dir exists,
///  - `in.fifo` is a named FIFO (proves the session was set up),
///  - the keepalive PID recorded in `ka.pid` is still running, AND
///  - the claude PID recorded in `claude.pid` is still running.
///
/// Returns `DEAD` otherwise.  Requiring `claude.pid` (not just the keepalive)
/// prevents the agent from "reconnecting" to a zombie session whose keepalive
/// still holds the FIFO open but whose claude process has already exited (e.g.
/// crashed on startup, or finished) — such a reconnect would attach a reader to
/// a stream that will never produce output.  Intended for `run_remote_script`.
pub fn remote_liveness_script(sid: &str) -> String {
    let dir = remote_session_dir(sid);
    format!(
        r#"dir={dir}
if [ ! -p "$dir/in.fifo" ]; then echo DEAD; exit 0; fi
ka=$(cat "$dir/ka.pid" 2>/dev/null || echo "")
cl=$(cat "$dir/claude.pid" 2>/dev/null || echo "")
if [ -n "$ka" ] && kill -0 "$ka" 2>/dev/null \
   && [ -n "$cl" ] && kill -0 "$cl" 2>/dev/null; then echo LIVE; else echo DEAD; fi
"#,
        dir = dir,
    )
}

/// Build the shell script that tears down a durable remote session.
///
/// Sends SIGTERM to the keepalive holder and the detached claude process (both
/// PIDs are stored in `$HOME/.marsad/<sid>/`), waits briefly for them to exit,
/// then removes the session dir entirely.  Idempotent — missing PIDs or a
/// missing dir are silently ignored.
pub fn remote_kill_script(sid: &str) -> String {
    let dir = remote_session_dir(sid);
    format!(
        r#"dir={dir}
if [ -d "$dir" ]; then
  ka=$(cat "$dir/ka.pid" 2>/dev/null || echo "")
  cl=$(cat "$dir/claude.pid" 2>/dev/null || echo "")
  [ -n "$ka" ] && kill "$ka" 2>/dev/null || true
  [ -n "$cl" ] && kill "$cl" 2>/dev/null || true
  sleep 0.2
  [ -n "$ka" ] && kill -9 "$ka" 2>/dev/null || true
  [ -n "$cl" ] && kill -9 "$cl" 2>/dev/null || true
  rm -rf "$dir"
  echo "marsad-kill: removed $dir"
else
  echo "marsad-kill: dir not found, nothing to do"
fi
"#,
        dir = dir,
    )
}

/// Build the argv for the writer child: an ssh/wsl process running
/// `cat >> $HOME/.marsad/<sid>/in.fifo` with its stdin piped from the agent.
///
/// The agent writes prompts (stream-json user messages) to the child's stdin;
/// the child forwards them into the remote FIFO.  The child's stdin EOF closes
/// the `cat` on the remote side — but the FIFO is kept open by the keepalive
/// holder, so the detached claude does NOT see EOF.
/// Build the argv for the writer child: `tee -a <dir>/in.fifo` with its stdin
/// piped from the agent. The agent writes prompts (stream-json user messages) to
/// the child's stdin; `tee` appends them into the remote FIFO (stdout → the
/// child's null). The child's stdin EOF closes `tee` on the remote side — but the
/// FIFO is kept open by the keepalive holder, so detached claude does NOT see EOF.
///
/// `dir` is the RESOLVED ABSOLUTE session dir (from `remote_resolve_pid_dir`), and
/// the args are passed as separate argv tokens — NO shell, NO quotes, NO `$()`.
/// This is essential: a command string with embedded `"$(...)"` (the old form) is
/// mangled when Rust's Windows arg-quoting passes it through `wsl.exe`/`ssh.exe`,
/// which made `tail`/`cat` fail and the session die instantly ("reader EOF after
/// 0 lines"). Keeping every token shell-free and quote-free avoids that entirely.
pub fn remote_writer_argv(prefix: &[String], dir: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tee".to_string());
    argv.push("-a".to_string());
    argv.push(format!("{dir}/in.fifo"));
    argv
}

/// Build the argv for the reader child: `tail -n0 --pid=<pid> -f <dir>/out.log`
/// with its stdout piped to the agent. `-n0` streams only NEW lines (history comes
/// from `TranscriptBackfill`); `--pid=<claude>` makes `tail` EOF when the detached
/// claude dies (stop/crash/kill) so the reader tears down cleanly instead of
/// hanging "alive". `pid` + `dir` are resolved literals (see
/// `remote_resolve_pid_dir`); passed as separate argv tokens — NO shell, NO
/// quotes, NO `$()` — to survive Rust→`wsl.exe`/`ssh.exe` arg passing.
pub fn remote_reader_argv(prefix: &[String], pid: &str, dir: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tail".to_string());
    argv.push("-n0".to_string());
    argv.push(format!("--pid={pid}"));
    argv.push("-f".to_string());
    argv.push(format!("{dir}/out.log"));
    argv
}
