//! How the hub reaches a host: the command whose stdio is the link (`ssh …`, `wsl.exe …`)
//! and the remote bootstrap that runs on it.
//!
//! One connection per connect attempt does everything, so a user who signs in interactively
//! does so once (on a Linux or macOS hub that connection is the host's shared one, see
//! [`SshRoute`]: the attempt's sign-in step opens it and the bootstrap, a link step, rides it):
//! the bootstrap reports `MARSAD-BOOT <os> <arch> <installed --version|none>`,
//! then reads one command line from stdin — `RUN`, `RESTART` (stop the running daemon) or
//! `UPLOAD <n>` followed by exactly `n` bytes of binary (written to `marsad-agent.new`, checked
//! to be exactly `n` bytes long — `head -c` ends cleanly on a short input, so a truncated upload
//! would otherwise be installed — then renamed into place atomically, old daemon stopped; any
//! failure ends the bootstrap with exit status 70 and its reason on stderr, which the hub
//! shows; on Windows see [`WINDOWS_BOOT_CS`]) —
//! answers `MARSAD-OK`, and finally
//! `exec`s `marsad-agent bridge --host-id <id>` on the same channel (so a daemon it starts
//! knows itself by the id the hub routes by; its Wasl node addresses agents with it). The hub
//! sends nothing after the command until it has read `MARSAD-OK`, so nothing the protocol
//! needs is consumed early.

use std::path::{Path, PathBuf};

use base64::Engine;
use marsad_proto::hosts::{HostEntry, HostKind, HostPlatform};

/// ssh options always passed: detect dead links and bound the TCP connect. No `BatchMode` by
/// default: prompts go to the `SSH_ASKPASS` helper instead.
pub fn ssh_base_options() -> Vec<String> {
    [
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ConnectTimeout=15",
    ]
    .iter()
    .flat_map(|o| ["-o".to_string(), o.to_string()])
    .collect()
}

/// How an ssh command of a host relates to the host's shared connection (Linux and macOS hubs;
/// see [`SshRoute`]).
///
/// One authenticated connection, the "master", is kept open per ssh host; every command that
/// talks to the host rides it, so the user signs in once and a restart of the hub or the app
/// does not ask again. Only the [`SshRoute::SignIn`] step may prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshRoute<'a> {
    /// No shared connection (a Windows hub: its ssh client cannot multiplex). The command is the
    /// whole connect attempt and may prompt. `ControlPath=none` keeps a user's ssh
    /// configuration from putting it on a connection of its own.
    Plain,
    /// The sign-in step: ssh authenticates (every prompt of the attempt happens here), becomes
    /// the master listening on `control` and detaches into the background (`ControlPersist`),
    /// the process that was started exiting with status 0 once it has. It runs without `-f`:
    /// the hub starts it as its own child, so the hub can end it when the sign-in fails and
    /// watch for the socket instead of trusting the exit alone (see `Masters::sign_in`). `-n`:
    /// it never reads the hub's stdin. Its messages go to `<control>.log` (`-E`), because the
    /// detached master keeps writing there for as long as it lives.
    SignIn { control: &'a Path },
    /// A link step: the bootstrap rides the master at `control`. `BatchMode=yes`: it can never
    /// prompt. When no master is there it connects directly (a key host) or fails without
    /// asking (a host that needs the user).
    Link { control: &'a Path },
}

/// The log of the shared connection whose socket is `control`.
pub fn master_log(control: &Path) -> PathBuf {
    let mut os = control.as_os_str().to_owned();
    os.push(".log");
    PathBuf::from(os)
}

/// A path as an ssh option value: `%` is ssh's own escape in `ControlPath`.
fn control_option(control: &Path) -> String {
    format!(
        "ControlPath={}",
        control.display().to_string().replace('%', "%%")
    )
}

/// The connection options of an ssh host that every ssh command about it carries, so that its
/// `Match` blocks evaluate as they do for the connection itself.
fn ssh_host_options(
    port: &Option<u16>,
    identity_file: &Option<String>,
    options: &[String],
) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(p) = port {
        argv.extend(["-p".to_string(), p.to_string()]);
    }
    if let Some(i) = identity_file {
        argv.extend([
            "-i".to_string(),
            i.clone(),
            "-o".to_string(),
            "IdentitiesOnly=yes".to_string(),
        ]);
    }
    for o in options {
        argv.extend(["-o".to_string(), o.clone()]);
    }
    argv
}

/// The argv prefix that runs one remote command (appended as the last element), by the same
/// route a connect attempt without a shared connection uses ([`SshRoute::Plain`]). `None` for
/// the local host.
pub fn transport_prefix(kind: &HostKind) -> Option<Vec<String>> {
    transport_prefix_via(kind, SshRoute::Plain)
}

/// The argv prefix of the ssh command of `route` for `kind` (for a WSL host, whatever the
/// route: it has no ssh). `None` for the local host.
pub fn transport_prefix_via(kind: &HostKind, route: SshRoute<'_>) -> Option<Vec<String>> {
    match kind {
        HostKind::Local => None,
        HostKind::Ssh {
            target,
            port,
            identity_file,
            options,
        } => {
            let mut argv = vec!["ssh".to_string()];
            match route {
                SshRoute::Plain | SshRoute::Link { .. } => argv.push("-T".to_string()),
                SshRoute::SignIn { .. } => {}
            }
            argv.extend(ssh_base_options());
            let o = |s: String| ["-o".to_string(), s];
            match route {
                SshRoute::Plain => {
                    argv.extend(o("ControlMaster=no".into()));
                    argv.extend(o("ControlPath=none".into()));
                }
                SshRoute::Link { control } => {
                    argv.extend(o("ControlMaster=no".into()));
                    argv.extend(o(control_option(control)));
                    argv.extend(o("BatchMode=yes".into()));
                }
                SshRoute::SignIn { control } => {
                    argv.extend(["-M".to_string(), "-N".to_string(), "-n".to_string()]);
                    argv.extend(o("ControlMaster=yes".into()));
                    argv.extend(o(control_option(control)));
                    argv.extend(o("ControlPersist=yes".into()));
                    argv.extend(["-E".to_string(), master_log(control).display().to_string()]);
                }
            }
            argv.extend(ssh_host_options(port, identity_file, options));
            argv.push("--".to_string());
            argv.push(target.clone());
            Some(argv)
        }
        // `--exec`: wsl.exe runs `sh` itself with exactly these arguments. (After a plain `--`
        // it would hand the rest of its command line to the user's login shell as one string,
        // which expands the bootstrap's `$…` before `sh` sees it.) `--cd ~`: the default
        // user's home, not the Windows directory the hub runs in; `-l`: their login PATH.
        HostKind::Wsl { distro } => Some(vec![
            super::wsl::wsl_exe()
                .map(|p| wasit_process::plain_path(&p).to_string_lossy().into_owned())
                .unwrap_or_else(|| "wsl.exe".to_string()),
            "--distribution".to_string(),
            distro.clone(),
            "--cd".to_string(),
            "~".to_string(),
            "--exec".to_string(),
            "sh".to_string(),
            "-lc".to_string(),
        ]),
    }
}

/// What `ssh -O <what>` asks the master at `control` (local: it talks to the socket only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterCommand {
    /// Is a master listening (`Master running (pid=<n>)`, status 0)?
    Check,
    /// End the master (and with it every command riding it).
    Exit,
}

/// The argv of `ssh -O check|exit` for an ssh host (`None` for any other kind).
pub fn master_command_argv(
    kind: &HostKind,
    control: &Path,
    what: MasterCommand,
) -> Option<Vec<String>> {
    let HostKind::Ssh {
        target,
        port,
        identity_file,
        options,
    } = kind
    else {
        return None;
    };
    let mut argv = vec![
        "ssh".to_string(),
        "-O".to_string(),
        match what {
            MasterCommand::Check => "check".to_string(),
            MasterCommand::Exit => "exit".to_string(),
        },
        "-o".to_string(),
        control_option(control),
    ];
    argv.extend(ssh_host_options(port, identity_file, options));
    argv.push("--".to_string());
    argv.push(target.clone());
    Some(argv)
}

/// The bytes a platform allows in a Unix socket path, terminator included (`sun_path`): 108 on
/// Linux, 104 on macOS and the BSDs.
pub const fn socket_path_limit() -> usize {
    if cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd"
    )) {
        104
    } else {
        108
    }
}

/// What ssh appends to a control path while it creates the socket (`.` and 16 random
/// characters): it binds `<path>.<16 random>` first and renames it into place, and refuses a
/// path for which that does not fit.
pub const SOCKET_TEMP_SUFFIX: usize = 17;

/// Whether ssh can create a control socket at `path` on a platform whose socket paths have
/// room for `limit` bytes (terminator included).
pub fn control_path_fits(path: &Path, limit: usize) -> bool {
    path.as_os_str().len() + SOCKET_TEMP_SUFFIX < limit
}

/// The control socket of the shared connection of a host: `<dir>/<16 hex>`, the hex being a
/// hash of the host's id and everything about its route (target, port, key, options). Not ssh's
/// `%C` (40 hex): the shorter name leaves room under the platform's socket path limit
/// ([`control_path_fits`]), and a host whose route is edited has a new name, so that it can
/// never ride the old connection.
pub fn control_socket(dir: &Path, id: &str, kind: &HostKind) -> PathBuf {
    let route = serde_json::to_string(kind).unwrap_or_default();
    let name = wasit_runtime::machine::key_from_raw(&format!("ssh-master:{id}:{route}"));
    dir.join(name)
}

/// What a bootstrap is for: the host's id, the hub's id and the hub's build (the helper
/// directory it looks in and installs into).
#[derive(Debug, Clone, Copy)]
pub struct BootTarget<'a> {
    pub host_id: &'a str,
    pub hub_id: &'a str,
    pub build: &'a str,
}

/// The directory name of `build` under the helper directory: only characters every shell and
/// file system takes as they are (`[A-Za-z0-9._+-]`), anything else `_`.
pub fn build_dir(build: &str) -> String {
    let d: String = build
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if d.is_empty() || d.chars().all(|c| c == '.') {
        "unknown".to_owned()
    } else {
        d
    }
}

/// Whether `id` can be passed to a remote shell unquoted (host ids are restricted to this at
/// `hosts.add`; hub ids are hex).
pub fn safe_host_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// What starts a POSIX bootstrap: `sh` says it is there ([`crate::bootwire::SH_READY`]) and runs
/// the one line the hub sends next ([`posix_script`]). The script travels on the connection, not
/// on the command line, so no shell between the hub and `sh` ever parses it.
pub const SH_STARTER: &str = "echo MARSAD-SH; IFS= read -r l; eval \"$l\"";

/// The command line of an ssh host, whatever its default shell (B9, U8: no platform is assumed
/// and none is detected by failing). The same text is three programs:
///
/// * a POSIX shell (sh, bash, zsh, fish, csh/tcsh) runs `exec sh -c '<SH_STARTER>'` and never
///   reaches the rest (it parses it: every part is valid in all of them);
/// * cmd.exe (OpenSSH for Windows' default) has no `'` quoting and splits at `&`: `exec` is not
///   a command, then `cmd.exe /d /s /c "<windows line>"` runs, then `rem` ends the line;
/// * PowerShell (a configured default) finds neither `exec` nor `rem` (errors on stderr, it goes
///   on) and runs `cmd.exe --% /d /s /c "<windows line>"` (`--%`: the rest is passed as is).
///
/// The Windows line holds no `"` or `'` ([`windows_line`]), which is what keeps it inside the
/// quotes of all three.
pub fn universal_command(t: BootTarget<'_>) -> String {
    let win = windows_line(t);
    format!("exec sh -c '{SH_STARTER}' ; rem '& cmd.exe /d /s /c \"{win}\" & rem ' ; cmd.exe --% /d /s /c \"{win}\"")
}

/// The helper's arguments after `boot`.
fn boot_args(t: BootTarget<'_>) -> String {
    format!("boot --host-id {} --hub-id {}", t.host_id, t.hub_id)
}

/// The POSIX bootstrap, one line, sent after `MARSAD-SH`. A helper of the hub's build in
/// `~/.marsad/bin/<build>/<uname -s>-<uname -m>/` takes over (`exec … boot`). Without one, it
/// reports itself (`helper_build=none` and the checksum tool it has: `sha256sum`,
/// `shasum -a 256` or `openssl`, else `none` and the hub installs nothing), receives
/// `UPLOAD <n> <sha256>`, writes exactly `n` bytes to a temporary file, checks their number and
/// their SHA-256, links the file into place (atomic, never over a helper another machine of a shared home
/// just installed) and hands over to the new helper with
/// `--installed`. Any failure removes the temporary file and answers `MARSAD-FAIL`. A Windows
/// port of `sh` (MSYS2, Git for Windows, Cygwin) as the account's default shell: `uname -s`
/// says so, and the Windows bootstrap runs instead (its path translation off).
pub fn posix_script(t: BootTarget<'_>) -> String {
    let build = build_dir(t.build);
    let args = boot_args(t);
    // `sh` runs this copy: its `$` must reach cmd.exe as it is.
    let win = windows_line(t).replace('$', "\\$");
    [
        &format!("b=\"$HOME/.marsad/bin/{build}/$(uname -s)-$(uname -m)\"; a=\"$b/marsad-agent\"; "),
        &format!("case \"$(uname -s)\" in MINGW*|MSYS*|CYGWIN*) MSYS2_ARG_CONV_EXCL=\"*\" MSYS_NO_PATHCONV=1 exec cmd.exe /d /s /c \"{win}\";; esac; "),
        &format!("[ -x \"$a\" ] && exec \"$a\" {args}; "),
        "h=none; for t in sha256sum shasum openssl; do if command -v $t >/dev/null 2>&1; then h=$t; break; fi; done; ",
        "echo \"MARSAD-BOOT2 os=$(uname -s) arch=$(uname -m) helper_build=none daemon=unknown hasher=$h\"; ",
        "IFS=\" \" read -r c n s || exit 65; ",
        "f() { rm -f \"$t\"; echo \"MARSAD-FAIL stage=install reason=$1\"; echo \"marsad bootstrap: $1\" >&2; exit 70; }; t=; ",
        "[ \"$c\" = UPLOAD ] || f unexpected%20command; ",
        "[ \"$h\" != none ] || f no%20checksum%20tool; ",
        "t=\"$a.new.$(uname -n).$$\"; mkdir -p \"$b\" || f cannot%20create%20the%20helper%20directory; ",
        "head -c \"$n\" > \"$t\" || f cannot%20write%20the%20helper; ",
        "r=$(wc -c < \"$t\" | tr -d \" \"); [ \"$r\" = \"$n\" ] || f received%20$r%20of%20$n%20bytes%20of%20the%20helper; ",
        "case $h in sha256sum) x=$(sha256sum \"$t\");; shasum) x=$(shasum -a 256 \"$t\");; *) x=$(openssl dgst -sha256 -r \"$t\");; esac; x=${x%% *}; ",
        "[ \"$x\" = \"$s\" ] || f \"checksum%20mismatch%20(received%20$x)\"; ",
        // `ln`: into place only if no other machine of a shared home installed this build
        // meanwhile (replacing a helper another bootstrap runs would pull it from under it);
        // `mv` where the file system has no hard links.
        "chmod 755 \"$t\" || f cannot%20install%20the%20helper; ",
        "ln \"$t\" \"$a\" 2>/dev/null || [ -x \"$a\" ] || mv -f \"$t\" \"$a\" || f cannot%20install%20the%20helper; rm -f \"$t\"; ",
        &format!("exec \"$a\" {args} --installed"),
    ]
    .concat()
}

/// The Windows bootstrap's protocol side when no helper of the hub's build is installed: a small
/// C# program, compiled on the host by [`POWERSHELL_BOOTSTRAP`] into
/// `%LOCALAPPDATA%\Marsad\bin\marsad-boot-<hash>.exe`. It reports (`helper_build=none`), receives
/// `UPLOAD <n> <sha256>` with exact, unbuffered reads of stdin, checks the size and SHA-256 and
/// renames the file into the build's directory; cmd.exe then starts the new helper's `boot`
/// (see [`windows_line`]). A failure answers `MARSAD-FAIL` and exits 1, so nothing more starts.
///
/// Why not PowerShell itself: OpenSSH for Windows hands the command a stdin pipe opened for
/// overlapped I/O, and Windows PowerShell 5.1 (the .NET Framework's console stream) reads it
/// synchronously — such a read returns what is already there, then never completes. This
/// program opens it as an asynchronous `FileStream`. PowerShell gets stdin from NUL, so its own
/// input reader takes nothing from the connection.
pub const WINDOWS_BOOT_CS: &str = include_str!("windows_boot.cs");

/// Run from `%MARSAD_PS%` (UTF-8, base64). Compiles [`WINDOWS_BOOT_CS`] (raw deflate, base64,
/// in `%MARSAD_BOOT_CS%`) to
/// `marsad-boot-@HASH@.exe` unless it is there. Errors come back as one plain line on stderr and
/// exit 1 (no CLIXML error record).
pub const POWERSHELL_BOOTSTRAP: &str = r#"$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'
trap { $m=[Text.Encoding]::UTF8.GetBytes("Marsad bootstrap: $_`n"); $e=[Console]::OpenStandardError(); $e.Write($m,0,$m.Length); $e.Flush(); exit 1 }
$d=Join-Path $env:LOCALAPPDATA 'Marsad\bin'; $x=Join-Path $d 'marsad-boot-@HASH@.exe'
if (-not (Test-Path $x)) {
$t="$x.$PID.exe"
$z=New-Object IO.Compression.DeflateStream(([IO.MemoryStream][Convert]::FromBase64String($env:MARSAD_BOOT_CS)),[IO.Compression.CompressionMode]::Decompress)
Add-Type -OutputType ConsoleApplication -OutputAssembly $t -TypeDefinition (New-Object IO.StreamReader($z)).ReadToEnd()
if (Test-Path $x) { Remove-Item $t } else { Move-Item $t $x }
}
"#;

/// cmd.exe's longest command line; the whole [`universal_command`] must fit, as cmd.exe parses
/// all of it (the Windows line twice).
pub const CMD_MAX_LINE: usize = 8191;

/// The name part of the compiled boot program: changes with its source (FNV-1a).
fn windows_boot_hash() -> String {
    let h = WINDOWS_BOOT_CS
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{h:016x}")
}

/// The Windows bootstrap, one cmd.exe command line without `"` or `'` (see
/// [`universal_command`]): in `%LOCALAPPDATA%\Marsad\bin` (made if missing; `cd /d` takes a path
/// with spaces unquoted), a helper of the hub's build in `<build>\Windows-<arch>\` takes over;
/// without one, PowerShell (stdin from NUL) makes sure the boot program exists (its script
/// from an environment variable: no quoting), the boot program installs the upload, and the new helper
/// takes over with `--installed`. Each step runs only if the one before succeeded. The boot
/// program's source goes in an environment variable (base64 is plain text to cmd.exe), cleared
/// before any program that keeps running starts.
pub fn windows_line(t: BootTarget<'_>) -> String {
    let hash = windows_boot_hash();
    let b64 = base64::engine::general_purpose::STANDARD;
    // Comments are for the reader of this repository, not worth room on cmd.exe's line.
    let code: String = WINDOWS_BOOT_CS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = miniz_oxide::deflate::compress_to_vec(code.as_bytes(), 10);
    let script = POWERSHELL_BOOTSTRAP.replace("@HASH@", &hash);
    let p = format!("{}\\Windows-%PROCESSOR_ARCHITECTURE%", build_dir(t.build));
    let args = boot_args(t);
    // PowerShell's script travels as UTF-8 base64 in an environment variable too (a third of
    // `-EncodedCommand`'s UTF-16 size); `^(`/`^)`: parentheses inside cmd.exe's block.
    let ps = "powershell -NoProfile -NonInteractive -Command iex^([Text.Encoding]::UTF8.GetString^([Convert]::FromBase64String^($env:MARSAD_PS^)^)^) <NUL";
    format!(
        "set MARSAD_BOOT_CS={}&& set MARSAD_PS={}&& cd /d %LOCALAPPDATA% && (if not exist Marsad\\bin mkdir Marsad\\bin) && cd Marsad\\bin && (if exist {p}\\marsad-agent.exe (set MARSAD_BOOT_CS=&& set MARSAD_PS=&& {p}\\marsad-agent.exe {args}) else ({ps} && set MARSAD_BOOT_CS=&& set MARSAD_PS=&& marsad-boot-{hash}.exe {p} && {p}\\marsad-agent.exe {args} --installed))",
        b64.encode(source),
        b64.encode(script.as_bytes())
    )
}

/// Full argv of one link step or connect attempt by `route`: the transport of the route and the
/// starter of the bootstrap (for an ssh host the [`universal_command`], for a WSL host `sh`
/// running [`SH_STARTER`]). For [`SshRoute::SignIn`] there is no remote command (the sign-in step
/// runs none). `None` for the local host, or a host or hub id that is not [`safe_host_id`].
pub fn bootstrap_argv_via(
    entry: &HostEntry,
    t: BootTarget<'_>,
    route: SshRoute<'_>,
) -> Option<Vec<String>> {
    let mut argv = transport_prefix_via(&entry.kind, route)?;
    if !safe_host_id(entry.id.as_str()) || !safe_host_id(t.host_id) || !safe_host_id(t.hub_id) {
        return None;
    }
    if matches!(route, SshRoute::SignIn { .. }) {
        return Some(argv);
    }
    argv.push(match &entry.kind {
        HostKind::Wsl { .. } => SH_STARTER.to_owned(),
        _ => universal_command(t),
    });
    Some(argv)
}

/// Map `uname -s` / `uname -m` (or Windows' `PROCESSOR_ARCHITECTURE`) to a platform.
pub fn platform_from(os: &str, arch: &str) -> Option<HostPlatform> {
    let (os, triple_os) = match os.trim().to_ascii_lowercase().as_str() {
        "linux" => ("linux", "unknown-linux-gnu"),
        "darwin" => ("macos", "apple-darwin"),
        "windows" => ("windows", "pc-windows-msvc"),
        _ => return None,
    };
    let arch = normalize_arch(arch);
    if arch.is_empty() {
        return None;
    }
    Some(HostPlatform {
        os: os.into(),
        target: format!("{arch}-{triple_os}"),
        arch,
    })
}

/// `uname -m` / `PROCESSOR_ARCHITECTURE` spellings → Rust's (`amd64` → `x86_64`, `arm64` →
/// `aarch64`); anything else lowercased as is (`armv7l`, `riscv64`, `i686`).
pub fn normalize_arch(arch: &str) -> String {
    match arch.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        "aarch64" | "arm64" | "aarch64_be" | "armv8" | "armv8l" => "aarch64".into(),
        other => other.into(),
    }
}

/// The helper build Marsad ships for a host's (os, arch): any Linux libc → the static musl
/// build; Windows on ARM → the x64 build (Windows 11 runs it emulated). `None`: no build exists
/// for that platform.
pub fn helper_target(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, normalize_arch(arch).as_str()) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") | ("windows", "aarch64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

/// Every helper triple [`helper_target`] can name (what a release bundle must carry).
pub const HELPER_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// The (os, arch) a Rust target triple runs on (`x86_64-unknown-linux-gnu` → linux, x86_64).
pub fn triple_platform(triple: &str) -> (&'static str, String) {
    let arch = normalize_arch(triple.split('-').next().unwrap_or(""));
    let os = if triple.contains("linux") {
        "linux"
    } else if triple.contains("darwin") {
        "macos"
    } else if triple.contains("windows") {
        "windows"
    } else {
        ""
    };
    (os, arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: HostKind) -> HostEntry {
        HostEntry {
            id: "h".into(),
            name: "h".into(),
            kind,
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        }
    }

    const T: BootTarget<'static> = BootTarget {
        host_id: "h",
        hub_id: "0123456789abcdef",
        build: "0.2.0+abc.dirty",
    };

    #[test]
    fn ssh_argv_keepalive_port_identity_and_no_batch_mode() {
        let e = entry(HostKind::Ssh {
            target: "me@box".into(),
            port: Some(2222),
            identity_file: Some("/k".into()),
            options: vec!["UserKnownHostsFile=/kh".into()],
        });
        // Without a shared connection (a Windows hub): the whole attempt in one command that may
        // prompt, and a user's ssh configuration cannot put it on a connection of its own.
        let a = bootstrap_argv_via(&e, T, SshRoute::Plain).unwrap();
        assert_eq!(a[0], "ssh");
        for o in [
            "ServerAliveInterval=15",
            "IdentitiesOnly=yes",
            "UserKnownHostsFile=/kh",
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            assert!(a.iter().any(|x| x == o), "{o} in {a:?}");
        }
        assert!(!a.iter().any(|x| x.starts_with("BatchMode")));
        assert_eq!(a[a.len() - 2], "me@box");
        assert_eq!(a.last().unwrap(), &universal_command(T));
        // The sign-in step of a shared connection (only this step may prompt): no BatchMode,
        // no remote command, the keep-alive of the connection that will live on, and the master
        // a background process of ssh's own (`ControlPersist=yes`) that the hub starts without
        // `-f` and never reads stdin of (`-n`), logging next to its socket.
        let ctl = Path::new("/run/m/ssh/0123456789abcdef");
        let sign = bootstrap_argv_via(&e, T, SshRoute::SignIn { control: ctl }).unwrap();
        assert_eq!(&sign[..2], ["ssh", "-o"], "{sign:?}");
        for o in [
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "ConnectTimeout=15",
            "ControlMaster=yes",
            "ControlPath=/run/m/ssh/0123456789abcdef",
            "ControlPersist=yes",
            "IdentitiesOnly=yes",
            "UserKnownHostsFile=/kh",
            "-M",
            "-N",
            "-n",
            "-E",
            "/run/m/ssh/0123456789abcdef.log",
        ] {
            assert!(sign.iter().any(|x| x == o), "{o} in {sign:?}");
        }
        assert!(!sign.iter().any(|x| x == "-f"), "no -f: {sign:?}");
        assert!(!sign.iter().any(|x| x.starts_with("BatchMode")), "{sign:?}");
        assert!(!sign.iter().any(|x| x == "-T"), "{sign:?}");
        assert_eq!(
            sign[sign.len() - 2..],
            ["--", "me@box"],
            "no remote command"
        );
        // A link step rides the master and can never prompt.
        let link = bootstrap_argv_via(&e, T, SshRoute::Link { control: ctl }).unwrap();
        for o in [
            "-T",
            "BatchMode=yes",
            "ControlMaster=no",
            "ControlPath=/run/m/ssh/0123456789abcdef",
            "ServerAliveInterval=15",
            "IdentitiesOnly=yes",
            "-p",
            "2222",
        ] {
            assert!(link.iter().any(|x| x == o), "{o} in {link:?}");
        }
        assert_eq!(link[link.len() - 2], "me@box");
        assert!(!link.iter().any(|x| x == "-M" || x == "-f" || x == "-E"));
        // The questions asked of the master are local and carry no remote command either.
        for (what, word) in [
            (MasterCommand::Check, "check"),
            (MasterCommand::Exit, "exit"),
        ] {
            let q = master_command_argv(&e.kind, ctl, what).unwrap();
            assert_eq!(&q[..3], ["ssh", "-O", word]);
            assert!(q
                .iter()
                .any(|x| x == "ControlPath=/run/m/ssh/0123456789abcdef"));
            assert_eq!(q[q.len() - 2..], ["--", "me@box"]);
        }
        assert!(master_command_argv(&HostKind::Local, ctl, MasterCommand::Check).is_none());
        // `%` is ssh's escape in a ControlPath.
        let odd = Path::new("/run/100%/ssh/x");
        let q = master_command_argv(&e.kind, odd, MasterCommand::Check).unwrap();
        assert!(
            q.iter().any(|x| x == "ControlPath=/run/100%%/ssh/x"),
            "{q:?}"
        );
        // A WSL distribution has only POSIX `sh`: the starter, as one argument.
        let w = entry(HostKind::Wsl {
            distro: "Ubuntu".into(),
        });
        let wa = bootstrap_argv_via(&w, T, SshRoute::Plain).unwrap();
        assert!(wa[0].ends_with("wsl.exe"));
        assert_eq!(
            &wa[1..],
            [
                "--distribution",
                "Ubuntu",
                "--cd",
                "~",
                "--exec",
                "sh",
                "-lc",
                SH_STARTER
            ]
        );
        let mut bad = e.clone();
        bad.id = "a b;c".into();
        assert!(bootstrap_argv_via(&bad, T, SshRoute::Plain).is_none());
        let bad_hub = BootTarget { hub_id: "x;y", ..T };
        assert!(bootstrap_argv_via(&e, bad_hub, SshRoute::Plain).is_none());
        assert!(bootstrap_argv_via(&entry(HostKind::Local), T, SshRoute::Plain).is_none());
    }

    /// The one command line holds a part for every default shell, and the parts cannot break
    /// out of each other's quoting: the Windows line has no quote of either kind and the POSIX
    /// starter no single quote; cmd.exe parses the whole line, which must fit its limit with
    /// the longest host id.
    #[test]
    fn the_universal_command_fits_cmd_and_keeps_its_parts_quoted() {
        let long = "h".repeat(64);
        let t = BootTarget {
            host_id: &long,
            ..T
        };
        let win = windows_line(t);
        assert!(!win.contains('"') && !win.contains('\''), "{win}");
        assert!(!SH_STARTER.contains('\''));
        let line = universal_command(t);
        assert!(line.len() < CMD_MAX_LINE, "{} chars", line.len());
        assert!(line.starts_with("exec sh -c 'echo MARSAD-SH; "), "{line}");
        assert!(line.contains(&format!("' ; rem '& cmd.exe /d /s /c \"{win}\" & rem ' ; ")));
        assert!(line.ends_with(&format!("cmd.exe --% /d /s /c \"{win}\"")));
        // The helper of the hub's build, else the boot program and then the new helper.
        assert!(win.contains(&format!(
            "(if exist 0.2.0+abc.dirty\\Windows-%PROCESSOR_ARCHITECTURE%\\marsad-agent.exe (set MARSAD_BOOT_CS=&& set MARSAD_PS=&& 0.2.0+abc.dirty\\Windows-%PROCESSOR_ARCHITECTURE%\\marsad-agent.exe boot --host-id {long} --hub-id 0123456789abcdef) else (powershell -NoProfile "
        )), "{win}");
        assert!(win.ends_with(&format!(
            "marsad-agent.exe boot --host-id {long} --hub-id 0123456789abcdef --installed))"
        )));
        // The POSIX script is sent on the connection: one line.
        let script = posix_script(t);
        assert!(!script.contains('\n'));
        assert!(script.contains("$HOME/.marsad/bin/0.2.0+abc.dirty/$(uname -s)-$(uname -m)"));
        assert!(script.ends_with(&format!(
            "exec \"$a\" boot --host-id {long} --hub-id 0123456789abcdef --installed"
        )));
        // Both parse in every POSIX shell at hand (`-n`: parse only).
        #[cfg(unix)]
        for shell in ["sh", "bash", "zsh", "dash"] {
            for text in [&script, &line] {
                let Ok(out) = std::process::Command::new(shell)
                    .args(["-n", "-c", text])
                    .output()
                else {
                    continue; // not installed
                };
                assert!(
                    out.status.success(),
                    "{shell}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
    }

    #[test]
    fn build_directories_are_plain_names() {
        assert_eq!(build_dir("0.2.0+abc.dirty"), "0.2.0+abc.dirty");
        assert_eq!(build_dir("a b/c\\d\"e'f"), "a_b_c_d_e_f");
        assert_eq!(build_dir(".."), "unknown");
        assert_eq!(build_dir(""), "unknown");
    }

    #[test]
    fn platforms() {
        assert_eq!(
            platform_from("Linux", "x86_64").unwrap().target,
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            platform_from("Darwin", "arm64").unwrap().target,
            "aarch64-apple-darwin"
        );
        assert_eq!(
            platform_from("Windows", "AMD64").unwrap().target,
            "x86_64-pc-windows-msvc"
        );
        assert!(platform_from("Plan9", "x86_64").is_none());
        let arm = platform_from("Linux", "armv7l").unwrap();
        assert_eq!((arm.os.as_str(), arm.arch.as_str()), ("linux", "armv7l"));
    }

    #[test]
    fn helper_targets_by_os_and_arch() {
        // Remote Linux hosts report a gnu (or musl, or other libc) platform; the bundle ships
        // static musl builds.
        let cases = [
            (("Linux", "x86_64"), Some("x86_64-unknown-linux-musl")),
            (("Linux", "amd64"), Some("x86_64-unknown-linux-musl")),
            (("Linux", "aarch64"), Some("aarch64-unknown-linux-musl")),
            (("Linux", "arm64"), Some("aarch64-unknown-linux-musl")),
            (("Darwin", "arm64"), Some("aarch64-apple-darwin")),
            (("Darwin", "x86_64"), Some("x86_64-apple-darwin")),
            (("Windows", "AMD64"), Some("x86_64-pc-windows-msvc")),
            (("Windows", "ARM64"), Some("x86_64-pc-windows-msvc")),
            (("Linux", "armv7l"), None),
            (("Linux", "riscv64"), None),
            (("Linux", "i686"), None),
        ];
        for ((os, arch), want) in cases {
            let p = platform_from(os, arch).unwrap();
            assert_eq!(helper_target(&p.os, &p.arch), want, "{os} {arch}");
            if let Some(t) = want {
                assert!(HELPER_TARGETS.contains(&t));
            }
        }
        assert_eq!(
            helper_target("linux", "x86_64"),
            helper_target(
                triple_platform("x86_64-unknown-linux-gnu").0,
                &triple_platform("x86_64-unknown-linux-gnu").1
            )
        );
        assert_eq!(
            triple_platform("aarch64-apple-darwin"),
            ("macos", "aarch64".into())
        );
    }

    /// ssh binds `<control path>.<16 random>` before it moves the socket into place and refuses a
    /// path that does not leave room for that under the platform's socket path limit. The socket
    /// name is 16 hex characters, not ssh's 40-hex `%C`; the shapes of the runtime directories
    /// the product picks (Linux `/run/user/<uid>/marsad/<8 hex>`, `/tmp/marsad-<uid>/<8 hex>`;
    /// macOS the per-user temporary directory `/var/folders/xx/<28>/T`, 49 characters with its
    /// trailing slash) fit with the room ssh needs.
    #[test]
    fn the_control_socket_path_leaves_room_for_ssh_on_linux_and_macos() {
        let k = HostKind::Ssh {
            target: "me@box".into(),
            port: None,
            identity_file: None,
            options: vec![],
        };
        let linux_dirs = [
            "/run/user/4294967295/marsad/0123abcd/ssh",
            "/tmp/marsad-4294967295/0123abcd/ssh",
        ];
        let macos_dir = format!(
            "/var/folders/zz/{}/T/marsad/0123abcd/ssh",
            "zyxvpxvq6csfxvn_n0000000000000"
        );
        for dir in linux_dirs {
            let p = control_socket(Path::new(dir), "any-host", &k);
            assert_eq!(p.file_name().unwrap().len(), 16, "{p:?}");
            assert!(
                control_path_fits(&p, 108),
                "{} bytes: {p:?}",
                p.as_os_str().len()
            );
            assert!(control_path_fits(&p, 104), "also fits macOS: {p:?}");
        }
        let p = control_socket(Path::new(&macos_dir), "any-host", &k);
        assert!(p.as_os_str().len() <= 86, "{p:?}");
        assert!(
            control_path_fits(&p, 104),
            "{} + {SOCKET_TEMP_SUFFIX} bytes: {p:?}",
            p.as_os_str().len()
        );
        // The bound is exact: 90 bytes fit Linux (90 + 17 + the terminator = 108), 91 do not;
        // 86 fit macOS, 87 do not.
        let at = |n: usize| PathBuf::from("/".to_string() + &"x".repeat(n - 1));
        assert!(control_path_fits(&at(90), 108) && !control_path_fits(&at(91), 108));
        assert!(control_path_fits(&at(86), 104) && !control_path_fits(&at(87), 104));
        // ssh's own `%C` name would not have fitted the macOS shape.
        assert!(
            !control_path_fits(
                &Path::new(&macos_dir).join("0123456789012345678901234567890123456789"),
                104
            ),
            "the reason for a name of our own"
        );
        assert_eq!(
            socket_path_limit(),
            if cfg!(target_os = "macos") { 104 } else { 108 }
        );
    }

    /// The socket name follows the host's id and every part of its route: an edited target,
    /// port, key or option is another name, so that it never rides the old connection.
    #[test]
    fn the_control_socket_name_follows_the_host_and_its_route() {
        let dir = Path::new("/run/m/ssh");
        let ssh = |t: &str, port: Option<u16>, key: Option<&str>, opts: &[&str]| HostKind::Ssh {
            target: t.into(),
            port,
            identity_file: key.map(Into::into),
            options: opts.iter().map(|o| o.to_string()).collect(),
        };
        let base = ssh("me@box", Some(22), Some("/k"), &["A=1"]);
        let same = control_socket(dir, "h", &base);
        assert_eq!(same, control_socket(dir, "h", &base.clone()));
        for other in [
            control_socket(dir, "h2", &base),
            control_socket(dir, "h", &ssh("you@box", Some(22), Some("/k"), &["A=1"])),
            control_socket(dir, "h", &ssh("me@box", Some(23), Some("/k"), &["A=1"])),
            control_socket(dir, "h", &ssh("me@box", Some(22), Some("/k2"), &["A=1"])),
            control_socket(dir, "h", &ssh("me@box", Some(22), Some("/k"), &["A=2"])),
            control_socket(dir, "h", &ssh("me@box", Some(22), None, &["A=1"])),
        ] {
            assert_ne!(other, same);
        }
        assert_eq!(master_log(&same).file_name().unwrap().len(), 16 + 4);
    }
}
