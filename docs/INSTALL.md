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

| OS | Hub data (hosts, store, daemon log, socket) |
|---|---|
| Linux, macOS | `~/.marsad` |
| Windows | `%LOCALAPPDATA%\Marsad` |

Override with the `MARSAD_DATA` environment variable. On remote hosts the helper is stored at
`~/.marsad/bin/marsad-agent`.

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
