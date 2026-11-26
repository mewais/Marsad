# Installing Marsad

Download the installer for your OS from the
[latest release](https://github.com/mewais/Marsad/releases) and verify it against `SHA256SUMS`
(`sha256sum -c SHA256SUMS --ignore-missing`, or `Get-FileHash` on Windows).

| OS | File |
|---|---|
| Debian/Ubuntu | `Marsad_<version>_amd64.deb` |
| Fedora/RHEL/openSUSE | `Marsad-<version>-1.x86_64.rpm` |
| Any Linux (x86_64) | `Marsad_<version>_amd64.AppImage` |
| Windows 10/11 (x64) | `Marsad_<version>_x64-setup.exe` (NSIS) or `Marsad_<version>_x64_en-US.msi` |
| macOS, Apple silicon | `Marsad_<version>_aarch64.dmg` |
| macOS, Intel | `Marsad_<version>_x64.dmg` |

`marsad-agent-<target-triple>[.exe]` files in the release are the standalone daemon builds that
Marsad uploads to remote hosts; you normally never need them.

## Linux

* deb: `sudo apt install ./Marsad_*_amd64.deb`
* rpm: `sudo dnf install ./Marsad-*.rpm`
* AppImage: `chmod +x Marsad_*.AppImage && ./Marsad_*.AppImage`

Requires WebKitGTK 4.1 (pulled in by the deb/rpm packages).

## Windows (unsigned builds)

Release builds are not code-signed unless the maintainer configured a certificate. Windows
SmartScreen then shows "Windows protected your PC": click **More info**, then **Run anyway**.
The MSI/NSIS installer installs per user and needs no administrator rights for NSIS.

## macOS (unsigned builds)

Open the `.dmg` and drag Marsad to Applications. An unsigned/un-notarized app is blocked by
Gatekeeper ("Marsad is damaged" or "cannot be opened because the developer cannot be
verified"). Either right-click the app, choose **Open**, then **Open** again, or run
`xattr -dr com.apple.quarantine /Applications/Marsad.app` once. Builds signed and notarized
with the maintainer's Apple credentials open normally.

## Where data lives

| OS | Data root |
|---|---|
| Linux, macOS | `~/.marsad` |
| Windows | `%LOCALAPPDATA%\Marsad` |

This machine's durable data is in `<root>/machines/<key>/` (sessions, hub store, Wasl journal,
diagnostics). The key comes from the OS machine id; `MARSAD_MACHINE_KEY` overrides it. The root
may be a home directory shared by several machines. Sockets and locks live in a machine-local
runtime directory, not under the root.

The desktop app takes its data root from the `MARSAD_DATA` environment variable when it is set.
The `marsad-agent` command takes `--data <dir>` instead. The helper cache on this machine is
`<root>/bin/<os>-<arch>/`. On a remote host the helper is uploaded to
`~/.marsad/bin/<os>-<arch>/marsad-agent`, where `<os>-<arch>` comes from `uname -s` and
`uname -m`.

On a POSIX host, every bootstrap also points `~/.marsad/bin/marsad-agent` at the build it
runs (a symlink, replaced atomically). Sessions launch their MCP servers through that pointer, so a
continued session follows a helper upgrade. Windows hosts keep the per-build path: a running `.exe`
cannot be replaced safely, so a continued session there keeps the helper it started with.

## Diagnostics and logs

Diagnostics are typed records of starts, stops, connection and session state, failures with
their codes, and sizes and timings. Prompt text, agent replies, tool input and output, file
contents and secrets are never written, and project folders appear only as a hash. An error
message from an agent CLI is kept only as a code, its length and a fingerprint. What `ssh` or
a host printed when a connection failed is kept, shortened, with registered secrets and
project paths masked.

Files under `<root>/machines/<key>/` (the diagnostics directory is private, `0700` and `0600`
on Linux and macOS; on Windows it is `%LOCALAPPDATA%\Marsad\machines\<key>\diag\`):

* `diag/app.ndjson`, `.1`, `.2`: the desktop app
* `diag/daemon.ndjson`, `.1` to `.3`: the background daemon
* `diag/aux.ndjson`, `.1`: short-lived processes (bridge, `stop`, askpass)
* `diag/exports/`: exports made by the hub
* `sessions/<id>/shim.log`, `.1`: the shim of one session
* `marsad-agent.log`: the daemon's raw stderr (crash output), never exported

**Export from the app:** Settings → Diagnostics → "Export diagnostics". Choose a time window
(Last 24 hours, Last 3 days or Last 7 days; Last 24 hours is selected at first), then click
**Export**. The card lists each source (This app, This computer, each connected host) with its
state, and counts the finished sources as the hub reports them. When the export is finished,
click **Save…** and choose where the file goes. This does not need Developer mode.

**Export from a terminal** (when the app will not start), as your own user:

    marsad-agent diagnostics export --since 24h

This writes `marsad-diagnostics-<host>-<id>.ndjson.gz` to the current directory and prints its
path. `--since` takes `s`, `m`, `h` or `d` (default `24h`, at most 31 days); `--out <path>`,
`--no-trails` and `--data <dir>` are optional. Without a running daemon it exports this
machine's own files. This page does not say where the installers place `marsad-agent`; if the
command is not found, use its full path.

**Developer mode** (Settings → Diagnostics) adds a Diagnostics tab to each session and a
connection log to each host. `MARSAD_LOG=<level>` in the environment separately writes plain
`tracing` text to stderr.

## Uninstall

* deb: `sudo apt remove marsad`; rpm: `sudo dnf remove marsad`; AppImage: delete the file.
* Windows: Settings > Apps > Marsad > Uninstall (stops the background daemon first).
* macOS: drag Marsad.app to the Trash.

Then optionally delete the data directory above. Running agent sessions live in detached
processes; stop the daemon first with `marsad-agent stop` if you want them gone.

## Remote hosts

Nothing to install by hand. When you add an SSH/WSL host, Marsad connects, detects its OS and
architecture and uploads the matching `marsad-agent` build over the same SSH connection to
`~/.marsad/bin/` (and re-uploads when the version differs). The installer carries builds for
Linux x86_64/aarch64 (static musl), Windows x86_64 and macOS arm64/x86_64 under its resources
`agents/<target-triple>/` directory. Any Linux host (glibc or musl, any distribution) gets the
static musl build for its architecture; Windows on ARM gets the x64 build. Other platforms (for
example 32-bit ARM or RISC-V) are refused with a clear message instead of retrying.

**WSL** (Windows only): *Add host → WSL* lists the distributions installed on this PC (WSL's
default preselected); the helper is installed in that distribution's default user's home
(`~/.marsad/bin`). The WSL choice only appears when Marsad runs on Windows and `wsl.exe` exists.

**One shared SSH connection (Linux and macOS).** On Linux and macOS Marsad keeps one
authenticated SSH connection open per SSH host and runs everything for that host over it, so
restarting the app or its background service does not ask you to sign in again. It stays open
until you choose *Sign out* on the host, remove the host or change how it is reached (its
target, port, key or options), or the connection itself dies (the network drops, the server
closes it); only then is a password, code or approval asked for again, and a host that needs you
to sign in waits for your click. It is not ended when Marsad stops, so a stopped Marsad leaves
it open. Its control socket and log are in the runtime directory, in `ssh/` (for example
`/run/user/<uid>/marsad/<8 hex>/ssh/<16 hex>`, and `…<16 hex>.log`, which holds what ssh wrote
when the connection ended, kept to what the last connection wrote); `ssh -O exit` on the socket
ends it too. In plain words: while it is open, programs running as your own local user can
reach that host without signing in again, which is the same trust as an ssh-agent; the owner
of this installation accepted that trade-off. A server that refuses extra sessions on one
connection (`MaxSessions 1`) cannot be used this way. Windows' SSH client cannot share a
connection, so on Windows every connection signs in on its own and there is no *Sign out*.

A host that cannot work as configured (unknown WSL distribution, no helper for its platform,
a shell that prints something before the bootstrap, a helper the host refuses to execute)
shows what is wrong and waits for *Try again*; only dropped connections retry on their own.

## Cutting a release

1. Bump the version in `Cargo.toml` (`[workspace.package]`), `apps/desktop/src-tauri/Cargo.toml`
   and `apps/desktop/src-tauri/tauri.conf.json`.
2. `git tag v0.3.0 && git push origin v0.3.0` (a tag containing `-`, such as `v0.3.0-rc.1`,
   makes a prerelease).
3. The **Release** workflow builds the helper binaries and every installer and creates a
   **draft** GitHub Release with installers, `SHA256SUMS` and the helpers. Review it and
   publish.

A manual run (Actions > Release > Run workflow) only uploads workflow artifacts; tick
*draft_release* to also get a draft prerelease.

Optional signing, active only when the secrets exist: macOS `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (and for notarization `APPLE_ID`,
`APPLE_PASSWORD`, `APPLE_TEAM_ID`). Windows signing has a documented hook in
`.github/workflows/release.yml`.
