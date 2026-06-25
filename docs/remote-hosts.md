# Remote hosts — execution architecture

Goal: an SSH or WSL host is a **full host** — sessions actually run *on it*, with the
same liveness / reconnect / resume guarantees as local sessions.

## Core idea

Reuse the local tmux+FIFO container model, but run it **on the remote box** by
prefixing every command with an opaque *exec prefix*:

- Local:  `tmux new-session …`,                     FIFOs opened as local files.
- SSH:    `ssh <target> -- tmux new-session …`,      FIFOs read/written via `ssh <target> -- cat …`.
- WSL:    `wsl.exe -d <distro> -- tmux new-session …`, FIFOs via `wsl.exe -d <distro> -- cat …`.

This keeps the kernel-FIFO/tmux liveness semantics intact (tmux on the remote means the
session survives the ssh link dropping and the app closing).

## The contract (shared by all slices)

`exec_prefix: Vec<String>` — argv prepended to every remote command. The remote types
treat it as **opaque**:
- run command `C` with args `A…` as: `Command::new(prefix[0]).args(&prefix[1..]).arg(C).args(A)`.
- run a shell script `S` as: `… .arg("sh").arg("-c").arg(S)`.

The agent builds the prefix from the host:
- SSH host (`user@host` / bare alias `etx`): `["ssh", <opts…>, "<target>"]`
- WSL host (`wsl:<distro>`): `["wsl.exe", "-d", "<distro>"]`
- plink host (`plink:<ssh-target>`): `["plink", "-share", "-batch", "<inner>"]`

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
master socket; every later `ssh` (health probe, container, FIFO transport) rides it with
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
Remote session dir: `~/.marsad/sessions/<id>/` containing `in.fifo`, `out.fifo`,
`err.log` (the remote shell expands `~`).

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

**Remote `run.sh` body** (`build_remote_spawn_cmd`) — with the FIFO wiring added later
by `spawn_script`. Example for `claude_bin=genie`, `setup="module load x"`,
`env=[("A","b")]`:

```
module load x
export 'A'='b'
cd '/proj' && exec 'genie' '--model' 'haiku'
```

Setup (when present) is the first line; one `export 'K'='V'` line per env pair follows
(key and value POSIX single-quoted via `remote_shell_quote`); then `cd '<project>' &&
exec '<bin>' '<args>'`. The local spawn command prepends the same setup + `export` lines
before `cd … && '<bin>' … < in.fifo > out.fifo 2>err.log`.

## Slices

1. **marsad-lifecycle** — `RemoteTmuxContainer` (`Container` impl) parameterized by an
   `exec_prefix`. Mirrors `TmuxContainer` (exists/attach/spawn/kill) but runs tmux/mkfifo
   through the prefix on the remote. Pure argv/command-string building must be unit-tested
   to 100%; real spawning gated `#[cfg(not(coverage))]`.
2. **marsad-transport** — `RemoteFifoTransport` (`Transport` impl): reader streams
   `<prefix> cat <out.fifo>`, writer pipes into `<prefix> sh -c 'cat > <in.fifo>'`. Plus add
   the ControlMaster/keepalive options to `SshStdio`. 100% coverage like `SshStdio`.
3. **desktop** — an `connect_host_interactive` command that opens a real OS terminal
   running `ssh <target>` with the SAME ControlPath (no BatchMode) so the user completes
   MFA once; cross-platform terminal launch.

Integration (agent dispatch: choose local vs remote container+transport by host, plumb
the host target/prefix + remote paths through LaunchSpec) is done after the slices land.
