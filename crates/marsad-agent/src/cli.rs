//! Command-line options shared by `serve`, `bridge` and `stop`.
//!
//! ```text
//! marsad-agent serve  [--hub] [--data <dir>] [--socket <path>] [--host-id <id>] [options]
//! marsad-agent bridge [same options]      (starts `serve` with them if no daemon runs)
//! marsad-agent stop   [--data <dir>] [--socket <path>]
//! marsad-agent shim   <wasit-shim arguments>
//! marsad-agent mcp
//! marsad-agent --version
//! ```
//!
//! Options: `--driver-bin <driver>=<path>` and `--driver-env <driver>=<KEY>=<VALUE>`
//! (repeatable) override a driver's binary / add environment; `--handshake-timeout-ms`,
//! `--stop-grace-ms`, `--command-timeout-ms`.
//!
//! Defaults: data `~/.marsad` (Windows: `%LOCALAPPDATA%\Marsad`), socket `<data>/marsad.sock`
//! (Windows: a named pipe derived from it). Keep the data path short: session sockets live at
//! `<data>/sessions/<12 chars>/ctl.sock` (Unix socket paths are limited to ~104 bytes).

use std::path::PathBuf;
use std::time::Duration;

use wasit_control::Endpoint;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub data: PathBuf,
    pub socket: Option<PathBuf>,
    pub host_id: Option<String>,
    pub hub: bool,
    pub driver_bin: Vec<(String, String)>,
    pub driver_env: Vec<(String, String, String)>,
    pub handshake_timeout: Option<Duration>,
    pub stop_grace: Option<Duration>,
    pub command_timeout: Option<Duration>,
    /// The options exactly as given (passed on when `bridge` starts `serve`).
    pub raw: Vec<String>,
}

/// The default data directory.
pub fn default_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Marsad"))
    }
    #[cfg(not(windows))]
    {
        crate::util::home_dir().map(|h| h.join(".marsad"))
    }
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Options, String> {
        let mut o = Options {
            raw: args.to_vec(),
            ..Options::default()
        };
        let mut data = None;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
            let ms = |v: String| {
                v.parse::<u64>()
                    .map(Duration::from_millis)
                    .map_err(|e| format!("{flag}: {e}"))
            };
            match flag.as_str() {
                "--data" => data = Some(PathBuf::from(val()?)),
                "--socket" => o.socket = Some(PathBuf::from(val()?)),
                "--host-id" => o.host_id = Some(val()?),
                "--hub" => o.hub = true,
                "--driver-bin" => {
                    let v = val()?;
                    let (d, p) = v
                        .split_once('=')
                        .ok_or(format!("--driver-bin expects DRIVER=PATH, got {v:?}"))?;
                    o.driver_bin.push((d.into(), p.into()));
                }
                "--driver-env" => {
                    let v = val()?;
                    let bad = || format!("--driver-env expects DRIVER=KEY=VALUE, got {v:?}");
                    let (d, kv) = v.split_once('=').ok_or_else(bad)?;
                    let (k, x) = kv.split_once('=').ok_or_else(bad)?;
                    o.driver_env.push((d.into(), k.into(), x.into()));
                }
                "--handshake-timeout-ms" => o.handshake_timeout = Some(ms(val()?)?),
                "--stop-grace-ms" => o.stop_grace = Some(ms(val()?)?),
                "--command-timeout-ms" => o.command_timeout = Some(ms(val()?)?),
                other => return Err(format!("unknown option {other}")),
            }
        }
        o.data = match data {
            Some(d) => d,
            None => default_data_dir().ok_or("no home directory: pass --data <dir>")?,
        };
        Ok(o)
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket
            .clone()
            .unwrap_or_else(|| self.data.join("marsad.sock"))
    }

    pub fn endpoint(&self) -> Endpoint {
        wasit_control::local_endpoint(&self.socket_path())
    }
}

/// This machine's default host id.
pub fn default_host_id() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "local".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        let a: Vec<String> = [
            "--data",
            "/d",
            "--hub",
            "--driver-bin",
            "claude=/bin/fake",
            "--driver-env",
            "claude=A=b=c",
            "--stop-grace-ms",
            "500",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let o = Options::parse(&a).unwrap();
        assert!(o.hub);
        assert_eq!(o.socket_path(), PathBuf::from("/d/marsad.sock"));
        assert_eq!(o.driver_env, [("claude".into(), "A".into(), "b=c".into())]);
        assert_eq!(o.stop_grace, Some(Duration::from_millis(500)));
        assert!(Options::parse(&["--nope".to_string()]).is_err());
    }
}
