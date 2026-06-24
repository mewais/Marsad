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

### Remote layout
Remote session dir: `~/.marsad/sessions/<id>/` containing `in.fifo`, `out.fifo`,
`err.log` (the remote shell expands `~`).

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
