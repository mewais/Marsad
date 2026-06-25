# Remote hosts — execution architecture

Goal: an SSH or WSL host is a **full host** — sessions actually run *on it*, with the
same liveness / reconnect / resume guarantees as local sessions.

## Core idea

**Local** sessions use the tmux+FIFO container model (unix) or the named-pipe shim
(windows): a detached container holds the claude process so it survives the app closing,
and the agent reads/writes its FIFOs/pipes.

**Remote** sessions use a different, simpler model: a **single piped connection**. The
agent spawns ONE child process

```
ssh <opts> <target> bash -lc '<setup>; cd <proj>; exec <claudebin> --print --input-format stream-json --output-format stream-json --verbose --model <M> …'
```

(or the `wsl.exe -d <distro> sh -c …` / `plink -share -batch <inner> bash -lc …` variant)
with claude's **stdin and stdout piped directly to the agent**. The agent writes the
`initialize` handshake + user messages to the child's stdin and reads stream-json events
from its stdout. The child is kept alive for the session and is killed on stop (killing it
closes the pipes, so the remote claude dies when the connection drops).

This replaced the earlier remote tmux+FIFO model, which opened THREE connections per
session (one to spawn `tmux` + `mkfifo`, a separate `cat out.fifo` reader, and a separate
`cat > in.fifo` writer). On Windows that model was broken: claude answered the `initialize`
handshake then its stdin closed and it exited ("reader EOF after 2 lines"). The
single-connection model holds one stdin open for the session's lifetime and is proven to
work end-to-end against a real host.

### Liveness tradeoff
A single-connection remote session is one child of the agent process — there is **no
detached remote container**. So unlike local sessions, a remote session does **not**
survive an agent restart: when the agent dies, the child (and its remote claude) die with
it. On relaunch the agent SKIPS reconnecting remote sessions; they must be explicitly
resumed (`claude --resume`/`--continue`). Local sessions still reconnect transparently.

## The contract (shared by all slices)

`exec_prefix: Vec<String>` — argv prepended to every remote command. The remote types
treat it as **opaque**:
- run command `C` with args `A…` as: `Command::new(prefix[0]).args(&prefix[1..]).arg(C).args(A)`.
- run a shell script `S` as: `… .arg("sh").arg("-c").arg(S)`.

The agent builds the prefix from the host:
- SSH host (`user@host` / bare alias `etx`): `["ssh", <opts…>, "<target>"]`
- WSL host (`wsl:<distro>`): `["wsl.exe", "-d", "<distro>"]`
- plink host (`plink:<ssh-target>`): `["plink", "-share", "-batch", "<inner>"]`

The run command is appended to the prefix via `remote_shell_argv`: for ssh/plink it is
passed as a single token to a remote LOGIN shell (`bash -lc '<run command>'`) so the
profile/modules are sourced and `<claudebin>` is on `PATH`; for wsl it is `sh -c '<run
command>'`.

### SSH option set (built by the agent / desktop, not by lifecycle/transport)
Always pass, in this order, before the target:
```
-o BatchMode=yes              (automated connections only; NOT the interactive login)
-o ConnectTimeout=10
-o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o TCPKeepAlive=yes
-o ControlMaster=auto
-o ControlPath=~/.ssh/marsad-cm-%r@%h:%p
-o ControlPersist=600
```
`ControlMaster`/`ControlPath`/`ControlPersist` mean: the **first** connection opens a
master socket; every later `ssh` (health probe, the session connection) rides it with
no re-auth. An MFA / keyboard-interactive host is handled by opening **one** interactive
login in a real OS terminal (NO `BatchMode`, SAME `ControlPath`) — once it succeeds the
master persists and all automated connections reuse it. No in-app PTY needed.

### PuTTY plink (`plink:<target>`) — the Windows-without-WSL alternative
Windows `ssh.exe` does **not** support `ControlMaster` connection multiplexing (it needs
Unix-domain sockets — `getsockname failed: Not a socket`), so a Duo/MFA host re-prompts on
every connection. WSL users get multiplexing via WSL's Linux `ssh`; users without WSL use
PuTTY's `plink`, which **does** support SSH connection sharing on Windows. Add such a host
with the `plink:` prefix on its SSH target (e.g. `plink:user@host`, `plink:etx`); `plink`
(PuTTY) must be installed and on `PATH`.

- Interactive login (desktop `connect_host_interactive`): opens a visible console running
  `plink -share <inner>` (NO `-batch`). This first connection becomes the connection-sharing
  **upstream** (the Duo prompt is answered here once).
- Automated connections (agent `exec_prefix_for`): `["plink", "-share", "-batch", "<inner>"]`.
  `-share` makes them **downstreams** that ride the upstream; `-batch` disables interactive
  prompts so headless connections fail cleanly instead of hanging.
- Health probe: `plink -share -batch <inner> true` — reads Connected once the upstream
  exists, Unreachable/Reconnecting otherwise (e.g. plink missing → Reconnecting).

plink flags (PuTTY manual): `-share` = enable SSH connection sharing; `-batch` = disable
interactive prompts. ControlMaster ssh options do **not** apply to plink and are omitted.

### Remote layout
The single-connection model writes **no** remote session files — claude's stdin/stdout
are piped over the connection, so there are no remote FIFOs, `run.sh`, or `err.log`. (The
remote `~/.marsad` dir is only touched by the legacy `RemoteTmuxContainer`, which the agent
no longer drives.)

### Per-host launch configuration
Each host (local, WSL, and SSH/plink alike) carries three OPTIONAL launch settings,
applied whenever a session launches on that host:

- `claude_bin: Option<String>` — the executable run in place of `claude` (a wrapper
  like `genie`, or an absolute path). Default = `claude`. Becomes `argv[0]` in
  `claude_argv`.
- `setup: Option<String>` — a shell command run BEFORE claude (e.g. `module load cuda`).
- `env: Vec<(String, String)>` — environment variables exported before claude.

Plumbing mirrors `host_target` exactly: stored on `Host` / `PersistedHost` /
`PersistedSession` / `LaunchSpec` / `LaunchSessionMsg`; resolved by `Core` from the
registry (by `host_id`) into the LaunchSession IPC (`claude_bin`/`setup` as strings,
`env` as a JSON array of `[k, v]` pairs); the agent sets them on the spec, persists
them on the session (so resume reuses them), and folds them into the spawn command.

The UI exposes them in two places on the Hosts settings tab: a fixed **Local machine**
card (saved via `update_host("localhost", …)`, whose target stays `this machine`), and
the same three fields in the **Add a host** form. The environment textarea is one
`KEY=VALUE` per line (blank/invalid lines ignored).

**Remote run command** (`build_remote_spawn_cmd`) — the body passed to the remote login
shell as the single piped child (NO redirections, NO FIFO wiring). Example for
`claude_bin=genie`, `setup="module load x"`, `env=[("A","b")]`:

```
module load x
export 'A'='b'
cd '/proj' && exec 'genie' '--model' 'haiku'
```

Setup (when present) is the first line; one `export 'K'='V'` line per env pair follows
(key and value POSIX single-quoted via `remote_shell_quote`); then `cd '<project>' &&
exec '<bin>' '<args>'`. The local (non-remote) spawn command prepends the same setup +
`export` lines before `cd … && '<bin>' … < in.fifo > out.fifo 2>err.log` (tmux+FIFO).

## Implementation

- **marsad-agent** — drives the model. `build_remote_spawn_cmd` builds the pure run-command
  body (unit-tested to 100%). For a remote launch the per-session reader thread
  (`spawn_session_reader`) spawns ONE child via `remote_shell_argv(prefix, run_command)`
  with stdin/stdout piped; the child is stored in a `session_children` registry so
  `kill_container` can terminate it on stop, and reaped when the reader loop ends.
  `spawn_container` is a no-op for remote hosts (the child is owned by the reader thread),
  and the agent-restart reconnect loop SKIPS remote sessions (see liveness tradeoff above).
- **marsad-lifecycle** — `remote_shell_argv` (ssh/plink → `bash -lc '<token>'`; wsl →
  `sh -c <script>`) builds the argv. `RemoteTmuxContainer` and
  `marsad-transport::RemoteFifoTransport` remain in their crates (pub + tested) but are no
  longer driven by the agent — they are the legacy multi-connection types.
- **desktop** — `connect_host_interactive` opens a real OS terminal running `ssh <target>`
  (or `plink -share <inner>`) with the SAME ControlPath/sharing so the user completes MFA
  once; cross-platform terminal launch.
