//! Connecting to and provisioning a host over ONE command per attempt (ARCHITECTURE §4.1,
//! with the owner's interactive-auth rule).
//!
//! [`BootstrapConnector`] (the hub's `LinkConnector`) runs `<transport> <bootstrap>` (see
//! [`super::transport`]) with `SSH_ASKPASS` pointing at this binary, reads the bootstrap's
//! report (OS, arch, installed build), uploads the matching binary through the same channel
//! when the build differs, and then speaks the control protocol over it. A failed attempt is
//! classified from ssh's exit status and stderr (wasit-control's classifier), so anything
//! needing a human parks the link in `auth_needed`.
//!
//! A host that has needed an interactive answer only connects when armed by an explicit
//! `hosts.reconnect`; otherwise an attempt fails at once with `auth_needed`, so a dropped link
//! never fires prompts or pushes on its own.
//!
//! [`AgentProvisioner`] handles what only the hello reveals (a daemon of another build still
//! running): it marks the host so the next attempt sends `RESTART`.
//!
//! Binaries: the host's (os, arch) picks the shipped helper triple
//! ([`super::transport::helper_target`]: any Linux → the static musl build); that is the
//! running executable when it was built for that triple, else
//! `MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]` (assumed to be the hub's build), else —
//! development builds — the running executable when it runs on the same (os, arch).
//! No build for the platform, or a missing file, fails the link (no retry loop) with a message
//! saying what to do.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use marsad_proto::daemon::VersionInfo;
use marsad_proto::hosts::{HostEntry, HostPlatform};
use marsad_proto::HostId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use wasit_control::{
    classify_command_failure, ClientOptions, CommandFailureKind, CommandSpec, CommandTransport,
    ConnectError, ControlClient, HostSpec, LinkConnector, ProvisionError, ProvisionReason,
    Provisioner,
};

use super::auth::Prompts;
use super::transport::{
    bootstrap_argv, helper_target, native_windows_sign, platform_from, triple_platform, RemoteShell,
};

/// Environment variable naming the directory with binaries for other target triples.
pub const BIN_DIR_ENV: &str = "MARSAD_AGENT_BIN_DIR";
/// Set for the askpass helper: which host's link is asking.
pub const ASKPASS_HOST_ENV: &str = "MARSAD_ASKPASS_HOST";
/// Set for the askpass helper: the hub's endpoint path.
pub const ASKPASS_SOCKET_ENV: &str = "MARSAD_ASKPASS_SOCKET";

/// Host entries shared between the hosts registry and the connector.
pub type Entries = Arc<RwLock<HashMap<HostId, HostEntry>>>;

/// Something a connect attempt learned, for the hosts registry to persist.
#[derive(Debug, Clone)]
pub enum Learned {
    Platform {
        host: HostId,
        platform: HostPlatform,
    },
    /// The host asked an interactive question while signing in.
    Interactive { host: HostId },
    /// The machine key the host's daemon reported in its hello.
    Machine { host: HostId, machine: String },
}

/// Hosts whose next attempt must restart the remote daemon.
pub type Restarts = Arc<Mutex<HashSet<HostId>>>;

pub struct BootstrapConnector {
    pub entries: Entries,
    pub learned: mpsc::Sender<Learned>,
    pub prompts: Prompts,
    pub restarts: Restarts,
    pub own: VersionInfo,
    pub bin_dir: Option<PathBuf>,
    /// The hub's endpoint path (for the askpass helper).
    pub socket: PathBuf,
}

fn cmd_err(
    kind: CommandFailureKind,
    exit_code: Option<i32>,
    stderr: impl Into<String>,
) -> ConnectError {
    ConnectError::Command {
        kind,
        exit_code,
        stderr: stderr.into(),
    }
}

/// Read one `\n`-terminated line byte by byte (never past it).
async fn read_line<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    loop {
        let mut b = [0u8; 1];
        if r.read(&mut b).await? == 0 {
            return Ok(None);
        }
        if b[0] == b'\n' {
            return Ok(Some(
                String::from_utf8_lossy(&buf)
                    .trim_end_matches('\r')
                    .to_string(),
            ));
        }
        buf.push(b[0]);
        if buf.len() > 4096 {
            return Err(std::io::Error::other("bootstrap line too long"));
        }
    }
}

/// Which binary to upload (see the module docs).
#[derive(Debug, PartialEq, Eq)]
pub enum Helper {
    /// The running executable.
    Running,
    File(PathBuf),
}

/// Pick the helper for `platform`, given the hub's own target triple and bin dir.
pub fn pick_helper(
    own_target: &str,
    bin_dir: Option<&std::path::Path>,
    platform: &HostPlatform,
) -> Result<Helper, ConnectError> {
    let Some(triple) = helper_target(&platform.os, &platform.arch) else {
        return Err(cmd_err(
            CommandFailureKind::UnsupportedPlatform,
            None,
            format!(
                "Marsad has no helper for {} on {} (it ships helpers for Linux, macOS and Windows on x86_64 and ARM64). This host cannot run Marsad sessions; remove it.",
                os_word(&platform.os),
                platform.arch
            ),
        ));
    };
    let exe_name = if triple.contains("windows") {
        "marsad-agent.exe"
    } else {
        "marsad-agent"
    };
    if triple == own_target {
        return Ok(Helper::Running);
    }
    let looked = bin_dir.map(|d| d.join(triple).join(exe_name));
    if let Some(p) = looked.as_ref().filter(|p| p.is_file()) {
        return Ok(Helper::File(p.clone()));
    }
    // A development build for another libc of the same OS and architecture.
    let (own_os, own_arch) = triple_platform(own_target);
    if own_os == platform.os && own_arch == platform.arch {
        return Ok(Helper::Running);
    }
    Err(cmd_err(
        CommandFailureKind::HelperMissing,
        None,
        match looked {
            Some(p) => format!(
                "This Marsad installation is missing its helper for {} {} (expected at {}). Reinstall Marsad, then try again.",
                os_word(&platform.os),
                platform.arch,
                wasit_process::plain_path(&p).display()
            ),
            None => format!(
                "This Marsad build has no helpers for other machines ({triple} needed): set {BIN_DIR_ENV} to a directory with {triple}/{exe_name}, or use an installed Marsad."
            ),
        },
    ))
}

fn os_word(os: &str) -> &str {
    match os {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        o => o,
    }
}

/// Lines a host may print before the bootstrap's report (a `.profile` or login banner).
const MAX_PREAMBLE_LINES: usize = 32;

/// Read until the `MARSAD-BOOT …` line. `Err(raw)`: the stream ended (or too much came) before
/// it; `raw` is everything read (possibly UTF-16LE from wsl.exe).
async fn read_boot_line<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<String, Vec<u8>> {
    let mut raw = Vec::new();
    for _ in 0..MAX_PREAMBLE_LINES {
        match read_line(r).await {
            Ok(Some(l)) if l.starts_with("MARSAD-BOOT ") => return Ok(l),
            Ok(Some(l)) => {
                raw.extend_from_slice(l.as_bytes());
                raw.push(b'\n');
            }
            Ok(None) | Err(_) => return Err(raw),
        }
    }
    Err(raw)
}

impl BootstrapConnector {
    fn entry(&self, id: &HostId) -> Result<HostEntry, ConnectError> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| ConnectError::Spawn(format!("unknown host {id}")))
    }

    /// The binary to install on a host running `platform`.
    pub fn artifact(&self, platform: &HostPlatform) -> Result<PathBuf, ConnectError> {
        match pick_helper(&self.own.target, self.bin_dir.as_deref(), platform)? {
            Helper::File(p) => Ok(p),
            Helper::Running => std::env::current_exe()
                .map_err(|e| ConnectError::Spawn(format!("cannot locate the running binary: {e}"))),
        }
    }

    async fn learn(&self, l: Learned) {
        if self.learned.send(l).await.is_err() {
            tracing::error!("the hosts registry is gone");
        }
    }

    /// Classify an attempt that ended before the protocol started.
    async fn failed(
        &self,
        host: &HostId,
        child: &wasit_control::ChildHandle,
        what: &str,
    ) -> ConnectError {
        let exit = child.wait_exit(Duration::from_secs(5)).await;
        let stderr = child.stderr_tail();
        let code = exit.and_then(|e| e.code);
        let mut kind = classify_command_failure(code, &stderr);
        if self.prompts.asked(host) > 0 && kind == CommandFailureKind::Other {
            // A cancelled or timed-out prompt makes ssh give up without a recognisable message.
            kind = CommandFailureKind::AuthNeeded;
        }
        let detail = if stderr.is_empty() {
            format!("{what} (exit {code:?})")
        } else {
            stderr
        };
        cmd_err(kind, code, detail)
    }
    /// The host never printed the bootstrap's report: classify what it printed instead
    /// (stdout and stderr; wsl.exe prints its errors to stdout, in UTF-16LE).
    async fn no_bootstrap(
        &self,
        entry: &HostEntry,
        child: &wasit_control::ChildHandle,
        stdout: &str,
    ) -> ConnectError {
        let host = &entry.id;
        let exit = child.wait_exit(Duration::from_secs(5)).await;
        let stderr = child.stderr_tail();
        let code = exit.and_then(|e| e.code);
        let both = match (stdout.is_empty(), stderr.is_empty()) {
            (true, _) => stderr.clone(),
            (false, true) => stdout.to_string(),
            (false, false) => format!("{stdout}\n{stderr}"),
        };
        let mut kind = classify_command_failure(code, &both);
        if self.prompts.asked(host) > 0 && kind == CommandFailureKind::Other {
            // A cancelled or timed-out prompt makes ssh give up without a recognisable message.
            kind = CommandFailureKind::AuthNeeded;
        }
        if kind == CommandFailureKind::Other && !stdout.is_empty() {
            kind = CommandFailureKind::UnexpectedOutput;
        }
        let detail = match kind {
            CommandFailureKind::UnknownDistro => {
                let distro = match &entry.kind {
                    marsad_proto::hosts::HostKind::Wsl { distro } => distro.as_str(),
                    _ => "",
                };
                let list = super::wsl::distros().await;
                let names: Vec<&str> = list.distros.iter().map(|d| d.name.as_str()).collect();
                if names.is_empty() {
                    format!("WSL has no distribution named \"{distro}\", and none is installed. Install one (wsl --install), then edit this host.")
                } else {
                    format!(
                        "WSL has no distribution named \"{distro}\". Installed: {}. Edit this host and pick one of them.",
                        names.join(", ")
                    )
                }
            }
            CommandFailureKind::WslUnavailable => format!("WSL is not ready on this PC: {both}"),
            CommandFailureKind::PermissionDenied => format!(
                "The host refused to run the Marsad helper (permission denied). Check that your home directory allows running programs (for example a noexec mount), then try again. ({both})"
            ),
            CommandFailureKind::UnexpectedOutput => format!(
                "The host printed something other than the Marsad bootstrap: {}. Make sure your shell startup files print nothing for non-interactive shells, then try again.",
                truncate(&both, 300)
            ),
            _ if both.is_empty() => format!("the connection closed before the bootstrap ran (exit {code:?})"),
            _ => both,
        };
        cmd_err(kind, code, detail)
    }
}

/// The remote command ran (the connection and sign-in worked) but printed no bootstrap report.
fn remote_command_ran(e: &ConnectError) -> bool {
    matches!(
        e,
        ConnectError::Command {
            kind: CommandFailureKind::Other
                | CommandFailureKind::CommandNotFound
                | CommandFailureKind::UnexpectedOutput
                | CommandFailureKind::PermissionDenied,
            ..
        }
    )
}

fn truncate(s: &str, max: usize) -> String {
    let t: String = s.chars().take(max).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
}

#[async_trait]
impl LinkConnector for BootstrapConnector {
    async fn connect(
        &self,
        spec: &HostSpec,
        opts: ClientOptions,
        attempt: u64,
    ) -> Result<ControlClient, ConnectError> {
        let entry = self.entry(&spec.id)?;
        let host = entry.id.clone();
        if entry.interactive_auth && !self.prompts.take_arm(&host) {
            return Err(cmd_err(
                CommandFailureKind::AuthNeeded,
                None,
                "this host signs in interactively: connect to sign in",
            ));
        }
        let exe = std::env::current_exe().map_err(|e| ConnectError::Spawn(e.to_string()))?;
        self.prompts.begin_attempt(&host);
        // A host never connected is tried with `sh`. One that turns out to be native Windows
        // is asked again at once with PowerShell (its platform is then learned): its `sh` is a
        // Windows port that reports MINGW/MSYS/CYGWIN, or the remote command ran but printed no
        // report (cmd.exe has no `sh` and mangles the POSIX line; its messages are localised,
        // so any such failure gets one PowerShell attempt, and a host where that fails too
        // keeps the POSIX attempt's error).
        let mut shell = RemoteShell::for_entry(&entry);
        let mut posix_failed: Option<ConnectError> = None;
        let (mut io, child, boot) = loop {
            let argv = bootstrap_argv(&entry, shell)
                .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))?;
            // The transport's spawn and exit are recorded under this link and attempt.
            let mut cmd = CommandSpec::new(argv).traced(spec.diag_link(), attempt);
            cmd.env = vec![
                (
                    "SSH_ASKPASS".into(),
                    wasit_process::plain_path(&exe).display().to_string(),
                ),
                ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
                ("DISPLAY".into(), ":0".into()),
                (ASKPASS_HOST_ENV.into(), host.to_string()),
                (ASKPASS_SOCKET_ENV.into(), self.socket.display().to_string()),
            ];
            let (mut io, child) = CommandTransport::spawn(&cmd)?;
            let child = Arc::new(child);
            // Only a host that never connected (once connected, its platform is known).
            let first_posix = shell == RemoteShell::Posix
                && entry.platform.is_none()
                && matches!(entry.kind, marsad_proto::hosts::HostKind::Ssh { .. });
            match read_boot_line(&mut io).await {
                Ok(l)
                    if first_posix
                        && native_windows_sign(l.split_whitespace().nth(1).unwrap_or("")) =>
                {
                    tracing::info!(host = %host, report = %l, "native Windows behind a POSIX sh port; using PowerShell");
                    shell = RemoteShell::PowerShell;
                }
                Ok(l) => break (io, child, l),
                Err(raw) => {
                    let out = wasit_process::decode_console_output(&raw);
                    let err = self.no_bootstrap(&entry, &child, out.trim()).await;
                    if first_posix && remote_command_ran(&err) {
                        tracing::info!(host = %host, error = %err, "no report from sh; trying the host as native Windows");
                        posix_failed = Some(err);
                        shell = RemoteShell::PowerShell;
                        continue;
                    }
                    return Err(match posix_failed {
                        Some(ConnectError::Command {
                            kind,
                            exit_code,
                            stderr,
                        }) => cmd_err(
                            kind,
                            exit_code,
                            format!("{stderr} (tried as a Windows host too: {err})"),
                        ),
                        _ => err,
                    });
                }
            }
        };
        if self.prompts.asked(&host) > 0 && !entry.interactive_auth {
            self.learn(Learned::Interactive { host: host.clone() })
                .await;
        }
        let mut parts = boot.split_whitespace();
        parts.next(); // MARSAD-BOOT
        let (os, arch) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let installed = parts
            .next()
            .and_then(|v| serde_json::from_str::<VersionInfo>(v).ok());
        let platform = platform_from(os, arch).ok_or_else(|| {
            cmd_err(
                CommandFailureKind::UnsupportedPlatform,
                None,
                format!("Marsad has no helper for {os} on {arch} (it ships helpers for Linux, macOS and Windows on x86_64 and ARM64). This host cannot run Marsad sessions; remove it."),
            )
        })?;
        if entry.platform.as_ref() != Some(&platform) {
            self.learn(Learned::Platform {
                host: host.clone(),
                platform: platform.clone(),
            })
            .await;
        }
        let restart = self
            .restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&host);
        let up_to_date = installed
            .as_ref()
            .is_some_and(|v| v.build == self.own.build);
        let io_err = |e: std::io::Error| ConnectError::Io(e.to_string());
        if up_to_date {
            let c: &[u8] = if restart { b"RESTART\n" } else { b"RUN\n" };
            io.write_all(c).await.map_err(io_err)?;
        } else {
            let file = self.artifact(&platform)?;
            let mut f = tokio::fs::File::open(&file).await.map_err(io_err)?;
            let len = f.metadata().await.map_err(io_err)?.len();
            tracing::info!(host = %host, target = %platform.target, bytes = len, "installing marsad-agent");
            io.write_all(format!("UPLOAD {len}\n").as_bytes())
                .await
                .map_err(io_err)?;
            tokio::io::copy(&mut f, &mut io).await.map_err(io_err)?;
        }
        io.flush().await.map_err(io_err)?;
        match read_line(&mut io).await {
            Ok(Some(l)) if l == "MARSAD-OK" => {}
            Ok(Some(l)) => {
                return Err(ConnectError::Protocol(format!(
                    "unexpected bootstrap answer {l:?}"
                )))
            }
            Ok(None) | Err(_) => {
                return Err(self
                    .failed(&host, &child, "installing marsad-agent failed")
                    .await)
            }
        }
        let client = ControlClient::connect(io, opts).await?;
        let hello = client.connection().remote_hello().clone();
        if let Some(m) = &hello.machine {
            let twin = self
                .entries
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .find(|e| e.id != host && e.machine.as_deref() == Some(m.as_str()))
                .map(|e| e.name.clone());
            if let Some(twin) = twin {
                if hello.host_id != host {
                    // Restarting it under this id would take it away from the other host,
                    // which would then restart it back.
                    return Err(ConnectError::Protocol(format!(
                        "this is the same machine as the host {twin:?} (machine key {m}); \
                         remove one of the two hosts"
                    )));
                }
                tracing::warn!(host = %host, twin = %twin, machine = %m, "two hosts are the same machine");
            }
            self.learn(Learned::Machine {
                host: host.clone(),
                machine: m.clone(),
            })
            .await;
        }
        let theirs = hello.host_id;
        // (Another build is restarted through the provisioner, which also fixes the id.)
        if theirs != host && hello.build == self.own.build {
            // A daemon started earlier under another id (e.g. by hand): the next attempt
            // restarts it as this host.
            tracing::info!(host = %host, theirs = %theirs, "the host's daemon runs under another id; restarting it");
            self.restarts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(host.clone());
            self.prompts.arm(&host);
            return Err(ConnectError::Protocol(format!(
                "the daemon on this host runs as {theirs:?}, not {host:?}; restarting it"
            )));
        }
        Ok(client
            .with_attachment(child.clone())
            .with_diagnostics(child))
    }
}

/// Handles a running daemon of another build (seen in its hello): the next attempt restarts it.
pub struct AgentProvisioner {
    pub restarts: Restarts,
    pub prompts: Prompts,
}

#[async_trait]
impl Provisioner for AgentProvisioner {
    async fn ensure(
        &self,
        host: &HostSpec,
        problem: ProvisionReason,
    ) -> Result<(), ProvisionError> {
        tracing::info!(host = %host.id, ?problem, "the host runs another daemon build; restarting it");
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host.id.clone());
        // The user just signed in for this link; the restart attempt is part of that.
        self.prompts.arm(&host.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(e: &ConnectError) -> CommandFailureKind {
        match e {
            ConnectError::Command { kind, .. } => *kind,
            other => panic!("not a command failure: {other:?}"),
        }
    }

    #[test]
    fn helper_for_a_gnu_linux_host_is_the_bundled_musl_build() {
        // The owner's report: a Linux server reports x86_64-unknown-linux-gnu; the Windows
        // installer ships agents\x86_64-unknown-linux-musl\marsad-agent.
        let dir = std::env::temp_dir().join(format!("marsad-pick-{}", std::process::id()));
        let musl = dir.join("x86_64-unknown-linux-musl");
        std::fs::create_dir_all(&musl).unwrap();
        std::fs::write(musl.join("marsad-agent"), b"x").unwrap();
        let linux = platform_from("Linux", "x86_64").unwrap();
        assert_eq!(linux.target, "x86_64-unknown-linux-gnu");
        assert_eq!(
            pick_helper("x86_64-pc-windows-msvc", Some(&dir), &linux).unwrap(),
            Helper::File(musl.join("marsad-agent"))
        );
        // The released Linux app's own sidecar is the musl build.
        assert_eq!(
            pick_helper("x86_64-unknown-linux-musl", Some(&dir), &linux).unwrap(),
            Helper::Running
        );
        // A gnu development build serves a same-arch Linux host itself when no musl build is
        // around.
        assert_eq!(
            pick_helper("x86_64-unknown-linux-gnu", None, &linux).unwrap(),
            Helper::Running
        );
        // ARM64 Linux, not in the bin dir: the installation is broken, not the host.
        let arm = platform_from("Linux", "aarch64").unwrap();
        let e = pick_helper("x86_64-pc-windows-msvc", Some(&dir), &arm).unwrap_err();
        assert_eq!(kind(&e), CommandFailureKind::HelperMissing);
        assert!(e.to_string().contains("Reinstall Marsad"), "{e}");
        assert!(!e.to_string().contains(r"\\?\"), "{e}");
        // No build exists for armv7 / riscv64: a configuration error naming os and arch.
        for arch in ["armv7l", "riscv64"] {
            let p = platform_from("Linux", arch).unwrap();
            let e = pick_helper("x86_64-pc-windows-msvc", Some(&dir), &p).unwrap_err();
            assert_eq!(kind(&e), CommandFailureKind::UnsupportedPlatform);
            assert!(e.to_string().contains(&format!("Linux on {arch}")), "{e}");
            assert!(kind(&e).is_configuration());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn bootstrap_report_after_a_banner_and_foreign_output() {
        let mut ok: &[u8] = b"Welcome!\nlast login: never\nMARSAD-BOOT Linux x86_64 none\nrest";
        assert_eq!(
            read_boot_line(&mut ok).await.unwrap(),
            "MARSAD-BOOT Linux x86_64 none"
        );
        // The owner's report: wsl.exe's UTF-16LE error instead of the bootstrap.
        let utf16: Vec<u8> = "There is no distribution with the supplied name.\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let raw = read_boot_line(&mut utf16.as_slice()).await.unwrap_err();
        let text = wasit_process::decode_console_output(&raw);
        assert_eq!(
            text.trim(),
            "There is no distribution with the supplied name."
        );
        assert_eq!(
            classify_command_failure(Some(1), &text),
            CommandFailureKind::UnknownDistro
        );
    }
}
