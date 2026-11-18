//! WSL distributions on a Windows hub (`hosts.wsl_distros`): what the Add-host dialog offers
//! and what `hosts.add` checks a WSL host against.
//!
//! `wsl.exe` writes UTF-16LE even into a pipe; every byte it prints is decoded with
//! [`wasit_process::decode_console_output`].

use std::time::Duration;

use marsad_proto::hosts::{WslDistro, WslDistros};

/// How long one `wsl.exe -l` may take (it can start the WSL service).
const LIST_DEADLINE: Duration = Duration::from_secs(20);

/// The `wsl.exe` to run: `%SystemRoot%\System32\wsl.exe` when it exists, else none.
pub fn wsl_exe() -> Option<std::path::PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"))?;
    let p = std::path::PathBuf::from(root)
        .join("System32")
        .join("wsl.exe");
    p.is_file().then_some(p)
}

/// The installed distributions (the hub's own machine).
pub async fn distros() -> WslDistros {
    let Some(exe) = wsl_exe() else {
        return WslDistros {
            available: false,
            distros: Vec::new(),
            problem: Some(if cfg!(windows) {
                "WSL is not installed on this PC. Install it with `wsl --install` in a terminal, then add the host again.".into()
            } else {
                "WSL hosts can only be added when Marsad runs on Windows.".into()
            }),
        };
    };
    let exe = exe.to_string_lossy().into_owned();
    let run = |args: &[&str]| {
        let argv: Vec<String> = std::iter::once(exe.clone())
            .chain(args.iter().map(|a| a.to_string()))
            .collect();
        async move { crate::util::run(&argv, None, &[], None, LIST_DEADLINE, 64 * 1024).await }
    };
    let verbose = run(&["--list", "--verbose"]).await;
    let (out, problem) = match verbose {
        Ok(o) if o.success() => (
            parse_verbose(&wasit_process::decode_console_output(&o.stdout)),
            None,
        ),
        Ok(o) => {
            let text = wasit_process::decode_console_output(&o.stdout);
            let text = if text.trim().is_empty() {
                o.stderr.clone()
            } else {
                text
            };
            if no_distros(&text) {
                (Vec::new(), Some(no_distros_message()))
            } else {
                // Old WSL builds have no `--verbose`: names only.
                match run(&["--list", "--quiet"]).await {
                    Ok(q) if q.success() => (
                        parse_quiet(&wasit_process::decode_console_output(&q.stdout)),
                        None,
                    ),
                    Ok(q) => {
                        let t = wasit_process::decode_console_output(&q.stdout);
                        let t = if t.trim().is_empty() { q.stderr } else { t };
                        (
                            Vec::new(),
                            Some(format!(
                                "wsl.exe could not list distributions: {}",
                                t.trim()
                            )),
                        )
                    }
                    Err(e) => (
                        Vec::new(),
                        Some(format!("wsl.exe could not list distributions: {e}")),
                    ),
                }
            }
        }
        Err(e) => (
            Vec::new(),
            Some(format!("wsl.exe could not list distributions: {e}")),
        ),
    };
    let problem = problem.or_else(|| out.is_empty().then(no_distros_message));
    WslDistros {
        available: true,
        distros: out,
        problem,
    }
}

fn no_distros(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("has no installed distributions") || t.contains("wsl_e_default_distro_not_found")
}

fn no_distros_message() -> String {
    "WSL has no Linux distribution installed. Install one (for example `wsl --install -d Ubuntu`), then add the host.".into()
}

/// `wsl.exe -l -v`:
/// ```text
///   NAME            STATE           VERSION
/// * Ubuntu-24.04    Running         2
///   Debian          Stopped         2
/// ```
/// The header and state words are localized, so the header is recognised by position (first
/// line) and "running" only in English (anything else reads as not running).
pub fn parse_verbose(text: &str) -> Vec<WslDistro> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let default = l.trim_start().starts_with('*');
            let mut t = l.trim_start().trim_start_matches('*').split_whitespace();
            let name = t.next()?.to_string();
            let state = t.next();
            let version = t.next().and_then(|v| v.parse().ok());
            Some(WslDistro {
                name,
                default,
                running: state.is_some_and(|s| s.eq_ignore_ascii_case("running")),
                version,
            })
        })
        .collect()
}

/// `wsl.exe -l -q`: one name per line, the default first.
pub fn parse_quiet(text: &str) -> Vec<WslDistro> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, n)| WslDistro {
            name: n.to_string(),
            default: i == 0,
            running: false,
            version: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn parses_verbose_listing_from_utf16() {
        let raw = utf16("  NAME            STATE           VERSION\r\n* Ubuntu-24.04    Running         2\r\n  Debian          Stopped         2\r\n  docker-desktop  Stopped         1\r\n");
        let d = parse_verbose(&wasit_process::decode_console_output(&raw));
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].name, "Ubuntu-24.04");
        assert!(d[0].default && d[0].running);
        assert_eq!(d[0].version, Some(2));
        assert_eq!(d[1].name, "Debian");
        assert!(!d[1].default && !d[1].running);
        assert_eq!(d[2].version, Some(1));
        // Localized header and states.
        let de =
            parse_verbose("  NAME      STATUS         VERSION\n* Ubuntu    Wird ausgeführt  2\n");
        assert_eq!(de[0].name, "Ubuntu");
        assert!(de[0].default && !de[0].running);
    }

    #[test]
    fn parses_quiet_listing_with_bom() {
        let mut raw = vec![0xFF, 0xFE];
        raw.extend(utf16("Ubuntu\r\nAlpine\r\n\r\n"));
        let d = parse_quiet(&wasit_process::decode_console_output(&raw));
        assert_eq!(
            d.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["Ubuntu", "Alpine"]
        );
        assert!(d[0].default && !d[1].default);
    }

    #[test]
    fn recognises_no_distributions() {
        assert!(no_distros("Windows Subsystem for Linux has no installed distributions.\nYou can resolve this by installing a distribution"));
        assert!(!no_distros("Ubuntu"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn unavailable_off_windows() {
        let d = distros().await;
        assert!(!d.available && d.distros.is_empty() && d.problem.is_some());
    }
}
