//! How the hub reaches a host: the command whose stdio is the link (`ssh …`, `wsl.exe …`)
//! and the remote bootstrap that runs on it.
//!
//! One connection per connect attempt does everything, so a user who signs in interactively
//! does so once: the bootstrap reports `MARSAD-BOOT <os> <arch> <installed --version|none>`,
//! then reads one command line from stdin — `RUN`, `RESTART` (stop the running daemon) or
//! `UPLOAD <n>` followed by exactly `n` bytes of binary (written to `marsad-agent.new`,
//! renamed into place atomically, old daemon stopped) — answers `MARSAD-OK`, and finally
//! `exec`s `marsad-agent bridge --host-id <id>` on the same channel (so a daemon it starts
//! knows itself by the id the hub routes by; its Wasl node addresses agents with it). The hub
//! sends nothing after the command until it has read `MARSAD-OK`, so nothing the protocol
//! needs is consumed early.

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

/// The POSIX bootstrap (no single quotes inside: it is wrapped in `sh -c '…'`).
pub const POSIX_BOOTSTRAP: &str = concat!(
    "b=\"$HOME/.marsad/bin/$(uname -s)-$(uname -m)/marsad-agent\"; v=none; ",
    "if [ -x \"$b\" ]; then v=$(\"$b\" --version 2>/dev/null | tr -d \" \"); [ -n \"$v\" ] || v=none; fi; ",
    "echo \"MARSAD-BOOT $(uname -s) $(uname -m) $v\"; ",
    "read c n || exit 65; ",
    "case \"$c\" in ",
    "UPLOAD) t=\"$b.new.$(uname -n).$$\"; mkdir -p \"${b%/*}\" && head -c \"$n\" > \"$t\" && chmod 755 \"$t\" && mv -f \"$t\" \"$b\" || { rm -f \"$t\"; exit 70; }; ",
    "\"$b\" stop >/dev/null 2>&1; echo MARSAD-OK;; ",
    "RESTART) \"$b\" stop >/dev/null 2>&1; echo MARSAD-OK;; ",
    "RUN) echo MARSAD-OK;; ",
    "*) exit 64;; esac; ",
    "exec \"$b\" bridge --host-id "
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
  $n=[long]$cmd[1]; $t="$b.new.$PID"; $f=[IO.File]::Create($t); $buf=New-Object byte[] 65536
  while ($n -gt 0) { $r=$i.Read($buf,0,[Math]::Min($buf.Length,$n)); if ($r -le 0) { exit 70 }; $f.Write($buf,0,$r); $n-=$r }
  $f.Close(); & $b stop | Out-Null
  if (Test-Path $b) { Move-Item -Force $b "$b.old" }; Move-Item -Force $t $b
} elseif ($cmd[0] -eq 'RESTART') { & $b stop | Out-Null } elseif ($cmd[0] -ne 'RUN') { exit 64 }
Say 'MARSAD-OK'
& $b bridge --host-id @HOST_ID@
exit $LASTEXITCODE
"#;

/// Whether `id` can be passed to the remote shell unquoted (host ids are restricted to this
/// at `hosts.add`).
pub fn safe_host_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// The POSIX bootstrap for host `id` (see [`safe_host_id`]).
pub fn posix_bootstrap(id: &str) -> String {
    format!("{POSIX_BOOTSTRAP}{id}")
}

/// Which bootstrap a host gets: POSIX `sh`, or PowerShell for native Windows (OpenSSH for
/// Windows runs commands with cmd.exe or PowerShell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteShell {
    Posix,
    PowerShell,
}

impl RemoteShell {
    /// What the host's known platform says (a host never connected is tried with `sh`).
    pub fn for_entry(entry: &HostEntry) -> RemoteShell {
        match &entry.kind {
            HostKind::Ssh { .. } if entry.platform.as_ref().is_some_and(|p| p.os == "windows") => {
                RemoteShell::PowerShell
            }
            _ => RemoteShell::Posix,
        }
    }
}

/// The POSIX bootstrap could not run because the host is native Windows: its shell (cmd.exe
/// or PowerShell) has no `sh`, or the `sh` it found is a Windows port (Git for Windows, MSYS2,
/// Cygwin, busybox-w32) whose `uname -s` is not a platform Marsad ships for. Given what the
/// host printed instead of a report, or the `<os>` of its report.
pub fn native_windows_sign(printed: &str) -> bool {
    let missing_sh = printed.contains("'sh' is not recognized as an internal or external command")
        || printed.contains("The term 'sh' is not recognized");
    let os = printed.trim().to_ascii_uppercase();
    missing_sh
        || ["MINGW", "MSYS", "CYGWIN", "WINDOWS_NT"]
            .iter()
            .any(|p| os.starts_with(p))
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

/// Full argv of one connect attempt: transport + the bootstrap for `shell`.
/// `None` for the local host, or a host id that is not [`safe_host_id`].
pub fn bootstrap_argv(entry: &HostEntry, shell: RemoteShell) -> Option<Vec<String>> {
    let mut argv = transport_prefix(&entry.kind)?;
    let id = entry.id.as_str();
    if !safe_host_id(id) {
        return None;
    }
    match (&entry.kind, shell) {
        (HostKind::Wsl { .. }, _) => argv.push(posix_bootstrap(id)),
        (_, RemoteShell::PowerShell) => argv.push(powershell_command(
            &POWERSHELL_BOOTSTRAP.replace("@HOST_ID@", id),
        )),
        (_, RemoteShell::Posix) => argv.push(format!("sh -c '{}'", posix_bootstrap(id))),
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
            machine: None,
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
        let a = bootstrap_argv(&e, RemoteShell::for_entry(&e)).unwrap();
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
        let wa = bootstrap_argv(&w, RemoteShell::for_entry(&w)).unwrap();
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
                &posix_bootstrap("h")
            ]
        );
        assert_eq!(wa.last().unwrap(), &posix_bootstrap("h"));
        assert!(wa.last().unwrap().ends_with("bridge --host-id h"));
        let mut bad = e.clone();
        bad.id = "a b;c".into();
        assert!(bootstrap_argv(&bad, RemoteShell::Posix).is_none());
        let mut win = e.clone();
        win.platform = platform_from("Windows", "AMD64");
        assert_eq!(RemoteShell::for_entry(&win), RemoteShell::PowerShell);
        assert!(bootstrap_argv(&win, RemoteShell::for_entry(&win))
            .unwrap()
            .last()
            .unwrap()
            .contains("-EncodedCommand "));
        // A host not connected yet is tried with sh; told it is Windows, with PowerShell.
        assert!(bootstrap_argv(&e, RemoteShell::PowerShell)
            .unwrap()
            .last()
            .unwrap()
            .starts_with("powershell -NoProfile"));
        assert!(bootstrap_argv(&entry(HostKind::Local), RemoteShell::Posix).is_none());
    }

    #[test]
    fn native_windows_is_recognised_from_what_the_posix_bootstrap_got() {
        // cmd.exe (OpenSSH for Windows' default shell) and PowerShell without `sh`.
        assert!(native_windows_sign(
            "'sh' is not recognized as an internal or external command,\r\noperable program or batch file."
        ));
        assert!(native_windows_sign(
            "sh : The term 'sh' is not recognized as the name of a cmdlet, function, script file"
        ));
        // `sh` from Git for Windows / MSYS2 / Cygwin / busybox-w32 answered the report.
        for os in [
            "MINGW64_NT-10.0-20348",
            "MSYS_NT-10.0",
            "CYGWIN_NT-10.0-19045",
            "Windows_NT",
        ] {
            assert!(native_windows_sign(os), "{os}");
            assert!(platform_from(os, "x86_64").is_none(), "{os}");
        }
        for other in [
            "Linux",
            "Darwin",
            "sh: 1: marsad-agent: not found",
            "Permission denied",
        ] {
            assert!(!native_windows_sign(other), "{other}");
        }
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
}
