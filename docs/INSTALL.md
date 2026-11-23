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

## Diagnostics and logs

Diagnostics are typed records of starts, stops, connection and session state, failures with
their codes, and sizes and timings. Free text is never written: text from the OS or another
program, which can name paths, users and hosts, is kept only as its length and a fingerprint.

Files under `<root>/machines/<key>/` (the diagnostics directory is private, `0700` and `0600`
on Linux and macOS; on Windows it is `%LOCALAPPDATA%\Marsad\machines\<key>\diag\`):

* `diag/app.ndjson`, `.1`, `.2`: the desktop app
* `diag/daemon.ndjson`, `.1` to `.3`: the background daemon
* `diag/aux.ndjson`, `.1`: short-lived processes (bridge, `stop`, askpass)
* `diag/exports/`: exports made by the hub
* `sessions/<id>/shim.log`, `.1`: the shim of one session
* `marsad-agent.log`: the daemon's raw stderr (crash output), never exported

**Export from the app:** not available in this build. The Settings → Diagnostics page has no
export control yet.

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
