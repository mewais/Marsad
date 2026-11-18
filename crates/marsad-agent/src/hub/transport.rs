//! How the hub reaches a host: the command whose stdio is the link (`ssh …`, `wsl.exe …`)
//! and the remote bootstrap that runs on it.
//!
//! One connection per connect attempt does everything, so a user who signs in interactively
//! does so once: the bootstrap reports `MARSAD-BOOT <os> <arch> <installed --version|none>`,
//! then reads one command line from stdin — `RUN`, `RESTART` (stop the running daemon) or
//! `UPLOAD <n>` followed by exactly `n` bytes of binary (written to `marsad-agent.new`,
//! renamed into place atomically, old daemon stopped) — answers `MARSAD-OK`, and finally
//! `exec`s `marsad-agent bridge` on the same channel. The hub sends nothing after the command
//! until it has read `MARSAD-OK`, so nothing the protocol needs is consumed early.

use base64::Engine;
use marsad_proto::hosts::{HostEntry, HostKind, HostPlatform};

/// ssh options always passed: detect dead links, bounded TCP connect, no multiplexing (not
/// available on Windows OpenSSH; never required). No `BatchMode`: prompts go to the
/// `SSH_ASKPASS` helper instead.
pub fn ssh_base_options() -> Vec<String> {
    [
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ConnectTimeout=15",
        "ControlMaster=no",
    ]
    .iter()
    .flat_map(|o| ["-o".to_string(), o.to_string()])
    .collect()
}

/// The argv prefix that runs one remote command (appended as the last element). `None` for
/// the local host.
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

/// The POSIX bootstrap (no single quotes inside: it is wrapped in `sh -c '…'`).
pub const POSIX_BOOTSTRAP: &str = concat!(
    "b=\"$HOME/.marsad/bin/marsad-agent\"; v=none; ",
    "if [ -x \"$b\" ]; then v=$(\"$b\" --version 2>/dev/null | tr -d \" \"); [ -n \"$v\" ] || v=none; fi; ",
    "echo \"MARSAD-BOOT $(uname -s) $(uname -m) $v\"; ",
    "read c n || exit 65; ",
    "case \"$c\" in ",
    "UPLOAD) mkdir -p \"$HOME/.marsad/bin\" && head -c \"$n\" > \"$b.new\" && chmod 755 \"$b.new\" && mv -f \"$b.new\" \"$b\" || exit 70; ",
    "\"$b\" stop >/dev/null 2>&1; echo MARSAD-OK;; ",
    "RESTART) \"$b\" stop >/dev/null 2>&1; echo MARSAD-OK;; ",
    "RUN) echo MARSAD-OK;; ",
    "*) exit 64;; esac; ",
    "exec \"$b\" bridge"
);

/// The PowerShell bootstrap for Windows OpenSSH hosts (same line protocol).
pub const POWERSHELL_BOOTSTRAP: &str = r#"$ErrorActionPreference='Stop'
$d=Join-Path $env:LOCALAPPDATA 'Marsad\bin'; $b=Join-Path $d 'marsad-agent.exe'; $v='none'
if (Test-Path $b) { $v=((& $b --version) -join '') -replace ' ','' }
$o=[Console]::OpenStandardOutput(); $i=[Console]::OpenStandardInput()
function Say($t) { $x=[Text.Encoding]::ASCII.GetBytes("$t`n"); $o.Write($x,0,$x.Length); $o.Flush() }
function ReadLine { $s=''; while ($true) { $c=$i.ReadByte(); if ($c -lt 0 -or $c -eq 10) { return $s }; $s+=[char]$c } }
Say "MARSAD-BOOT Windows $env:PROCESSOR_ARCHITECTURE $v"
$cmd=(ReadLine).Split(' ')
if ($cmd[0] -eq 'UPLOAD') {
  New-Item -ItemType Directory -Force $d | Out-Null
  $n=[long]$cmd[1]; $f=[IO.File]::Create("$b.new"); $buf=New-Object byte[] 65536
  while ($n -gt 0) { $r=$i.Read($buf,0,[Math]::Min($buf.Length,$n)); if ($r -le 0) { exit 70 }; $f.Write($buf,0,$r); $n-=$r }
  $f.Close(); & $b stop | Out-Null
  if (Test-Path $b) { Move-Item -Force $b "$b.old" }; Move-Item -Force "$b.new" $b
} elseif ($cmd[0] -eq 'RESTART') { & $b stop | Out-Null } elseif ($cmd[0] -ne 'RUN') { exit 64 }
Say 'MARSAD-OK'
& $b bridge
exit $LASTEXITCODE
"#;

fn is_windows(p: Option<&HostPlatform>) -> bool {
    p.is_some_and(|p| p.os == "windows")
}

/// `powershell -EncodedCommand <base64 UTF-16LE>`: no quoting through ssh/cmd at all.
pub fn powershell_command(script: &str) -> String {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(utf16)
    )
}

/// Full argv of one connect attempt: transport + bootstrap.
pub fn bootstrap_argv(entry: &HostEntry) -> Option<Vec<String>> {
    let mut argv = transport_prefix(&entry.kind)?;
    match (&entry.kind, is_windows(entry.platform.as_ref())) {
        (HostKind::Wsl { .. }, _) => argv.push(POSIX_BOOTSTRAP.to_string()),
        (_, true) => argv.push(powershell_command(POWERSHELL_BOOTSTRAP)),
        (_, false) => argv.push(format!("sh -c '{POSIX_BOOTSTRAP}'")),
    }
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
    let arch = match arch.trim().to_ascii_lowercase().as_str() {
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
            interactive_auth: false,
        }
    }

    #[test]
    fn ssh_argv_keepalive_port_identity_and_no_batch_mode() {
        let e = entry(HostKind::Ssh {
            target: "me@box".into(),
            port: Some(2222),
            identity_file: Some("/k".into()),
            options: vec!["UserKnownHostsFile=/kh".into()],
        });
        let a = bootstrap_argv(&e).unwrap();
        assert_eq!(a[0], "ssh");
        for o in [
            "ServerAliveInterval=15",
            "IdentitiesOnly=yes",
            "UserKnownHostsFile=/kh",
        ] {
            assert!(a.iter().any(|x| x == o), "{o} in {a:?}");
        }
        assert!(!a.iter().any(|x| x.starts_with("BatchMode")));
        assert_eq!(a[a.len() - 2], "me@box");
        assert!(a.last().unwrap().starts_with("sh -c 'b="));
        assert!(!POSIX_BOOTSTRAP.contains('\''));
        let w = entry(HostKind::Wsl {
            distro: "Ubuntu".into(),
        });
        let wa = bootstrap_argv(&w).unwrap();
        assert_eq!(&wa[..3], ["wsl.exe", "-d", "Ubuntu"]);
        assert_eq!(wa.last().unwrap(), POSIX_BOOTSTRAP);
        let mut win = e.clone();
        win.platform = platform_from("Windows", "AMD64");
        assert!(bootstrap_argv(&win)
            .unwrap()
            .last()
            .unwrap()
            .contains("-EncodedCommand "));
        assert!(bootstrap_argv(&entry(HostKind::Local)).is_none());
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
    }
}
