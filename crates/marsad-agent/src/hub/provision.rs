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
//! Binaries: the running executable for its own target triple, else
//! `MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]` (assumed to be the hub's build).

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
use super::transport::{bootstrap_argv, platform_from};

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

impl BootstrapConnector {
    fn entry(&self, id: &HostId) -> Result<HostEntry, ConnectError> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| ConnectError::Spawn(format!("unknown host {id}")))
    }

    /// The binary to install for `target`.
    pub fn artifact(&self, target: &str) -> Result<PathBuf, String> {
        if target == self.own.target {
            return std::env::current_exe()
                .map_err(|e| format!("cannot locate the running binary: {e}"));
        }
        let exe = if target.contains("windows") {
            "marsad-agent.exe"
        } else {
            "marsad-agent"
        };
        let dir = self
            .bin_dir
            .as_ref()
            .ok_or_else(|| format!("no marsad-agent build for {target}: set {BIN_DIR_ENV} to a directory with <triple>/{exe}"))?;
        let p = dir.join(target).join(exe);
        if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "no marsad-agent build for {target} at {}",
                p.display()
            ))
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
}

#[async_trait]
impl LinkConnector for BootstrapConnector {
    async fn connect(
        &self,
        spec: &HostSpec,
        opts: ClientOptions,
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
        let argv = bootstrap_argv(&entry)
            .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))?;
        let exe = std::env::current_exe().map_err(|e| ConnectError::Spawn(e.to_string()))?;
        let mut cmd = CommandSpec::new(argv);
        cmd.env = vec![
            ("SSH_ASKPASS".into(), exe.display().to_string()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ("DISPLAY".into(), ":0".into()),
            (ASKPASS_HOST_ENV.into(), host.to_string()),
            (ASKPASS_SOCKET_ENV.into(), self.socket.display().to_string()),
        ];
        self.prompts.begin_attempt(&host);
        let (mut io, child) = CommandTransport::spawn(&cmd)?;
        let child = Arc::new(child);
        let boot = match read_line(&mut io).await {
            Ok(Some(l)) => l,
            Ok(None) | Err(_) => {
                return Err(self
                    .failed(
                        &host,
                        &child,
                        "the connection closed before the bootstrap ran",
                    )
                    .await)
            }
        };
        if self.prompts.asked(&host) > 0 && !entry.interactive_auth {
            self.learn(Learned::Interactive { host: host.clone() })
                .await;
        }
        let mut parts = boot.split_whitespace();
        if parts.next() != Some("MARSAD-BOOT") {
            return Err(ConnectError::Protocol(format!(
                "unexpected bootstrap output {boot:?}"
            )));
        }
        let (os, arch) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let installed = parts
            .next()
            .and_then(|v| serde_json::from_str::<VersionInfo>(v).ok());
        let platform = platform_from(os, arch).ok_or_else(|| {
            ConnectError::Protocol(format!("unsupported host platform {os} {arch}"))
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
            let file = self
                .artifact(&platform.target)
                .map_err(ConnectError::Spawn)?;
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
