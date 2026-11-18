//! The hub's [`Provisioner`]: install or update `marsad-agent` on a host over the same
//! command transport as the link (ARCHITECTURE §4.1).
//!
//! 1. One bounded probe: OS, architecture and the installed agent's `--version` line
//!    (POSIX shell first, then PowerShell for Windows OpenSSH hosts).
//! 2. The binary for the host's target triple: the running executable when the triple
//!    matches, else `MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]`.
//! 3. If the installed build differs (or nothing is installed): stream the binary over the
//!    transport's stdin into `~/.marsad/bin/marsad-agent.new`, then rename it into place.
//! 4. `marsad-agent stop` on the host, so the next `bridge` starts the new daemon. Running
//!    sessions are unaffected (their shims hold them).
//!
//! Binaries for other triples are assumed to come from the same build as the hub (a release
//! bundle), so a host running one of them reports the hub's build id.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use marsad_proto::daemon::VersionInfo;
use marsad_proto::hosts::{HostEntry, HostPlatform};
use marsad_proto::{HostId, MarsadError};
use tokio::sync::mpsc;
use wasit_control::{
    classify_command_failure, HostSpec, ProvisionError, ProvisionReason, Provisioner,
};

use super::transport::{
    agent_command, platform_from, script_argv, POSIX_PROBE, POSIX_UPLOAD, WINDOWS_PROBE,
    WINDOWS_UPLOAD,
};
use crate::util::{run, Output};

/// Environment variable naming the directory with binaries for other target triples.
pub const BIN_DIR_ENV: &str = "MARSAD_AGENT_BIN_DIR";

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Host entries shared between the hosts registry and the provisioner.
pub type Entries = Arc<RwLock<HashMap<HostId, HostEntry>>>;

/// A platform discovered by a probe, for the hosts registry to persist.
#[derive(Debug, Clone)]
pub struct PlatformFound {
    pub host: HostId,
    pub platform: HostPlatform,
}

pub struct AgentProvisioner {
    pub entries: Entries,
    pub platforms: mpsc::Sender<PlatformFound>,
    pub own: VersionInfo,
    pub bin_dir: Option<PathBuf>,
}

struct Probe {
    platform: HostPlatform,
    installed: Option<VersionInfo>,
}

fn perr(message: impl Into<String>, retryable: bool) -> ProvisionError {
    ProvisionError {
        message: message.into(),
        retryable,
    }
}

/// Turn a failed transport command into a provisioning error: anything needing a human is
/// not retried automatically.
fn transport_err(what: &str, out: &Output) -> ProvisionError {
    let kind = classify_command_failure(out.code, &out.stderr);
    perr(
        format!(
            "{what} failed ({kind:?}, exit {:?}): {}",
            out.code,
            out.stderr.trim()
        ),
        !kind.needs_human(),
    )
}

fn merr(what: &str, e: MarsadError) -> ProvisionError {
    let retryable = matches!(e, MarsadError::Timeout { .. });
    perr(format!("{what}: {e}"), retryable)
}

/// Parse a probe's stdout: OS line, arch line, then a version JSON line or `none`.
fn parse_probe(stdout: &str) -> Option<Probe> {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let os = lines.next()?;
    let arch = lines.next()?;
    let platform = platform_from(os, arch)?;
    let installed = lines.find_map(|l| serde_json::from_str::<VersionInfo>(l).ok());
    Some(Probe {
        platform,
        installed,
    })
}

impl AgentProvisioner {
    fn entry(&self, id: &HostId) -> Result<HostEntry, ProvisionError> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| perr(format!("unknown host {id}"), false))
    }

    async fn remote(
        &self,
        entry: &HostEntry,
        script: &str,
        stdin: Option<&std::path::Path>,
        deadline: Duration,
    ) -> Result<Output, MarsadError> {
        let argv = script_argv(entry, script)
            .ok_or_else(|| MarsadError::invalid("the local host needs no provisioning"))?;
        run(&argv, None, &[], stdin, deadline, 1024 * 1024).await
    }

    async fn probe(&self, entry: &HostEntry) -> Result<Probe, ProvisionError> {
        let out = self
            .remote(entry, POSIX_PROBE, None, PROBE_TIMEOUT)
            .await
            .map_err(|e| merr("probe", e))?;
        if let Some(p) = out
            .success()
            .then(|| parse_probe(&out.stdout_str()))
            .flatten()
        {
            return Ok(p);
        }
        let kind = classify_command_failure(out.code, &out.stderr);
        if kind.needs_human() {
            return Err(transport_err("probe", &out));
        }
        let win = self
            .remote(entry, WINDOWS_PROBE, None, PROBE_TIMEOUT)
            .await
            .map_err(|e| merr("probe", e))?;
        if let Some(p) = win
            .success()
            .then(|| parse_probe(&win.stdout_str()))
            .flatten()
        {
            return Ok(p);
        }
        Err(perr(
            format!(
                "cannot tell the host's platform (sh: exit {:?}, {}; powershell: exit {:?}, {})",
                out.code,
                out.stderr.trim(),
                win.code,
                win.stderr.trim()
            ),
            true,
        ))
    }

    /// The binary to install for `target`.
    pub fn artifact(&self, target: &str) -> Result<PathBuf, ProvisionError> {
        if target == self.own.target {
            return std::env::current_exe()
                .map_err(|e| perr(format!("cannot locate the running binary: {e}"), false));
        }
        let exe = if target.contains("windows") {
            "marsad-agent.exe"
        } else {
            "marsad-agent"
        };
        let dir = self.bin_dir.as_ref().ok_or_else(|| {
            perr(
                format!("no marsad-agent build for {target}: set {BIN_DIR_ENV} to a directory with <triple>/{exe}"),
                false,
            )
        })?;
        let p = dir.join(target).join(exe);
        if p.is_file() {
            Ok(p)
        } else {
            Err(perr(
                format!("no marsad-agent build for {target} at {}", p.display()),
                false,
            ))
        }
    }
}

#[async_trait]
impl Provisioner for AgentProvisioner {
    async fn ensure(
        &self,
        host: &HostSpec,
        problem: ProvisionReason,
    ) -> Result<(), ProvisionError> {
        let entry = self.entry(&host.id)?;
        tracing::info!(host = %host.id, ?problem, "provisioning");
        let probe = self.probe(&entry).await?;
        if entry.platform.as_ref() != Some(&probe.platform) {
            let os_changed = entry
                .platform
                .as_ref()
                .map(|p| p.os.as_str())
                .unwrap_or("linux")
                != probe.platform.os.as_str()
                && (entry.platform.is_some() || probe.platform.os == "windows");
            if self
                .platforms
                .send(PlatformFound {
                    host: host.id.clone(),
                    platform: probe.platform.clone(),
                })
                .await
                .is_err()
            {
                return Err(perr("the hosts registry is gone", false));
            }
            if os_changed {
                // The link command depends on the OS; the registry re-creates the link.
                return Err(perr(
                    format!(
                        "{} runs {}; reconnecting with its command line",
                        host.id, probe.platform.os
                    ),
                    true,
                ));
            }
        }
        let windows = probe.platform.os == "windows";
        let mut entry = entry;
        entry.platform = Some(probe.platform.clone());
        let installed_ok = probe
            .installed
            .as_ref()
            .is_some_and(|v| v.build == self.own.build);
        if !installed_ok {
            let file = self.artifact(&probe.platform.target)?;
            let script = if windows {
                WINDOWS_UPLOAD
            } else {
                POSIX_UPLOAD
            };
            let out = self
                .remote(&entry, script, Some(&file), UPLOAD_TIMEOUT)
                .await
                .map_err(|e| merr("upload", e))?;
            if !out.success() {
                return Err(transport_err("upload", &out));
            }
            tracing::info!(host = %host.id, target = %probe.platform.target, "installed marsad-agent");
        }
        // Whatever daemon runs there is the wrong one (or none): ask it to exit.
        let out = self
            .remote(&entry, &agent_command(&entry, "stop"), None, STOP_TIMEOUT)
            .await
            .map_err(|e| merr("stopping the old daemon", e))?;
        if !out.success() {
            return Err(transport_err("stopping the old daemon", &out));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output() {
        let p = parse_probe(
            "Linux\nx86_64\n{\"build\":\"0.2.0+abc\",\"target\":\"x86_64-unknown-linux-gnu\"}\n",
        )
        .unwrap();
        assert_eq!(p.platform.target, "x86_64-unknown-linux-gnu");
        assert_eq!(p.installed.unwrap().build, "0.2.0+abc");
        let p = parse_probe("Darwin\narm64\nnone\n").unwrap();
        assert!(p.installed.is_none());
        assert!(parse_probe("garbage").is_none());
    }
}
