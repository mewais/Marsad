//! How the hub reaches a host: the command whose stdio is the link (`ssh …`, `wsl.exe …`),
//! the remote install location, and remote scripts run over the same command.

use marsad_proto::hosts::{HostEntry, HostKind, HostPlatform};

/// Install location on POSIX hosts (relative to `$HOME`).
pub const POSIX_BIN: &str = ".marsad/bin/marsad-agent";

/// ssh options always passed: never prompt, detect dead links, bounded connect.
pub fn ssh_base_options() -> Vec<String> {
    [
        "BatchMode=yes",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ConnectTimeout=15",
        "ControlMaster=no",
    ]
    .iter()
    .flat_map(|o| ["-o".to_string(), o.to_string()])
    .collect()
}

/// The argv prefix that runs one remote command (the command follows as the last element,
/// interpreted by the remote shell). `None` for the local host.
pub fn transport_prefix(kind: &HostKind) -> Option<Vec<String>> {
    match kind {
        HostKind::Local => None,
        HostKind::Ssh {
            target,
            port,
            identity_file,
            options,
        } => {
            let mut argv = vec!["ssh".to_string(), "-T".to_string()];
            argv.extend(ssh_base_options());
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
            argv.push("--".to_string());
            argv.push(target.clone());
            Some(argv)
        }
        HostKind::Wsl { distro } => Some(vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            distro.clone(),
            "--".to_string(),
            "sh".to_string(),
            "-c".to_string(),
        ]),
    }
}

fn is_windows(p: Option<&HostPlatform>) -> bool {
    p.is_some_and(|p| p.os == "windows")
}

/// The remote command that runs the installed agent with `args`.
pub fn agent_command(entry: &HostEntry, args: &str) -> String {
    if is_windows(entry.platform.as_ref()) {
        format!(
            "powershell -NoProfile -NonInteractive -Command \"& \\\"$env:LOCALAPPDATA\\Marsad\\bin\\marsad-agent.exe\\\" {args}\""
        )
    } else {
        format!("exec \"$HOME/{POSIX_BIN}\" {args}")
    }
}

/// Full argv of the hub link (`<transport> marsad-agent bridge`).
pub fn connect_argv(entry: &HostEntry) -> Option<Vec<String>> {
    let mut argv = transport_prefix(&entry.kind)?;
    argv.push(agent_command(entry, "bridge"));
    Some(argv)
}

/// argv running a remote script.
pub fn script_argv(entry: &HostEntry, script: &str) -> Option<Vec<String>> {
    let mut argv = transport_prefix(&entry.kind)?;
    argv.push(script.to_string());
    Some(argv)
}

/// Probe (POSIX): OS, architecture and the installed agent's version line.
pub const POSIX_PROBE: &str = "uname -s; uname -m; \"$HOME/.marsad/bin/marsad-agent\" --version 2>/dev/null || echo none";

/// Probe (Windows OpenSSH, whose default shell is cmd or PowerShell).
pub const WINDOWS_PROBE: &str = "powershell -NoProfile -NonInteractive -Command \"Write-Output Windows; Write-Output $env:PROCESSOR_ARCHITECTURE; $a=\\\"$env:LOCALAPPDATA\\Marsad\\bin\\marsad-agent.exe\\\"; if (Test-Path $a) { & $a --version } else { Write-Output none }\"";

/// Upload (POSIX): stdin becomes the new binary, renamed into place atomically.
pub const POSIX_UPLOAD: &str = "set -e; d=\"$HOME/.marsad/bin\"; mkdir -p \"$d\"; cat > \"$d/marsad-agent.new\"; chmod 755 \"$d/marsad-agent.new\"; mv -f \"$d/marsad-agent.new\" \"$d/marsad-agent\"";

/// Upload (Windows): a running .exe cannot be overwritten but can be renamed, so the old one
/// is moved aside first.
pub const WINDOWS_UPLOAD: &str = "powershell -NoProfile -NonInteractive -Command \"$d=\\\"$env:LOCALAPPDATA\\Marsad\\bin\\\"; New-Item -ItemType Directory -Force $d | Out-Null; $n=\\\"$d\\marsad-agent.exe.new\\\"; $s=[Console]::OpenStandardInput(); $f=[IO.File]::Create($n); $s.CopyTo($f); $f.Close(); $t=\\\"$d\\marsad-agent.exe\\\"; if (Test-Path $t) { Move-Item -Force $t \\\"$t.old\\\" }; Move-Item -Force $n $t\"";

/// Map `uname -s` / `uname -m` (or Windows' `PROCESSOR_ARCHITECTURE`) to a platform.
pub fn platform_from(os: &str, arch: &str) -> Option<HostPlatform> {
    let os_l = os.trim().to_ascii_lowercase();
    let arch_l = arch.trim().to_ascii_lowercase();
    let (os, triple_os) = match os_l.as_str() {
        "linux" => ("linux", "unknown-linux-gnu"),
        "darwin" => ("macos", "apple-darwin"),
        "windows" => ("windows", "pc-windows-msvc"),
        _ => return None,
    };
    let arch = match arch_l.as_str() {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => return None,
    };
    Some(HostPlatform {
        os: os.into(),
        arch: arch.into(),
        target: format!("{arch}-{triple_os}"),
    })
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
        }
    }

    #[test]
    fn ssh_argv_has_batch_mode_keepalive_port_and_identity() {
        let e = entry(HostKind::Ssh {
            target: "me@box".into(),
            port: Some(2222),
            identity_file: Some("/k".into()),
            options: vec!["UserKnownHostsFile=/kh".into()],
        });
        let a = connect_argv(&e).unwrap();
        assert_eq!(a[0], "ssh");
        for o in ["BatchMode=yes", "ServerAliveInterval=15", "IdentitiesOnly=yes", "UserKnownHostsFile=/kh"] {
            assert!(a.iter().any(|x| x == o), "{o} in {a:?}");
        }
        let at = a.iter().position(|x| x == "-p").unwrap();
        assert_eq!(a[at + 1], "2222");
        assert_eq!(a[a.len() - 2], "me@box");
        assert_eq!(a.last().unwrap(), "exec \"$HOME/.marsad/bin/marsad-agent\" bridge");
        let w = entry(HostKind::Wsl { distro: "Ubuntu".into() });
        assert_eq!(&connect_argv(&w).unwrap()[..3], ["wsl.exe", "-d", "Ubuntu"]);
        assert!(connect_argv(&entry(HostKind::Local)).is_none());
    }

    #[test]
    fn platforms() {
        assert_eq!(platform_from("Linux", "x86_64").unwrap().target, "x86_64-unknown-linux-gnu");
        assert_eq!(platform_from("Darwin", "arm64").unwrap().target, "aarch64-apple-darwin");
        assert_eq!(platform_from("Windows", "AMD64").unwrap().target, "x86_64-pc-windows-msvc");
        assert!(platform_from("Plan9", "x86_64").is_none());
    }
}
