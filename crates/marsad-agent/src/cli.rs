//! Command-line options shared by `serve`, `bridge` and `stop`.
//!
//! ```text
//! marsad-agent serve  [--hub] [--data <dir>] [--run-dir <dir>] [--socket <path>] [--host-id <id>] [options]
//! marsad-agent bridge [same options]      (starts `serve` with them if no daemon runs)
//! marsad-agent boot   --host-id <id> --hub-id <hub> [--installed] [same options]
//!                                         (the remote end of a hub's bootstrap, see `boot.rs`)
//! marsad-agent stop   [--data <dir>] [--socket <path>]
//! marsad-agent shim   <wasit-shim arguments>
//! marsad-agent kilo-bridge -- <cli argv>   (the wrapper a Kilo session's process runs through)
//! marsad-agent mcp
//! marsad-agent --version
//! ```
//!
//! Options: `--driver-bin <driver>=<path>` and `--driver-env <driver>=<KEY>=<VALUE>`
//! (repeatable) override a driver's binary / add environment; `--handshake-timeout-ms`,
//! `--stop-grace-ms`, `--command-timeout-ms`, `--auth-prompt-timeout-ms` (hub: how long a
//! sign-in prompt waits; default 120 s), `--export-cap-bytes` and `--export-host-budget-bytes`
//! (hub: the size cap of a diagnostics export's compressed file, default 20 MiB, and the raw
//! bytes one source contributes, default 16 MiB).
//!
//! Defaults: data root `~/.marsad` (Windows: `%LOCALAPPDATA%\Marsad`), which may be shared by
//! several machines; this machine's durable data lives in `<data>/machines/<machine key>/` and
//! its sockets and locks in a machine-local runtime directory (`--run-dir`, see
//! `marsad_proto::layout`). Socket `<run>/marsad.sock` (Windows: a named pipe derived from it).

use std::path::PathBuf;
use std::time::Duration;

use marsad_proto::layout::Layout;
use wasit_control::Endpoint;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The data root (possibly shared between machines).
    pub data: PathBuf,
    pub run_dir: Option<PathBuf>,
    /// Resolved by [`Options::parse`].
    pub layout: Option<Layout>,
    pub socket: Option<PathBuf>,
    pub host_id: Option<String>,
    pub hub: bool,
    /// The hub this daemon serves (`serve`, `boot`; see [`marsad_proto::daemon::DaemonInfo`]).
    pub hub_id: Option<String>,
    /// `boot`: this run follows an install by the bootstrap (reported as `install=ok`).
    pub installed: bool,
    pub driver_bin: Vec<(String, String)>,
    pub driver_env: Vec<(String, String, String)>,
    pub handshake_timeout: Option<Duration>,
    pub stop_grace: Option<Duration>,
    pub command_timeout: Option<Duration>,
    /// How long a sign-in prompt waits for the user (hub).
    pub auth_prompt_timeout: Option<Duration>,
    /// The cap of a diagnostics export's compressed file, in bytes (hub; default 20 MiB).
    pub export_cap: Option<u64>,
    /// Raw bytes one source contributes to a diagnostics export (hub; default 16 MiB).
    pub export_host_budget: Option<u64>,
    /// `--panic-test <marker>`: the daemon panics, once its diagnostics run, with a message
    /// holding the marker (tests/diag_panic.rs; only in builds with the `panic-test` feature,
    /// which only this package's own dev-dependency turns on).
    #[cfg(feature = "panic-test")]
    pub panic_test: Option<String>,
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
            let bytes = |v: String| v.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
            match flag.as_str() {
                "--data" => data = Some(PathBuf::from(val()?)),
                "--run-dir" => o.run_dir = Some(PathBuf::from(val()?)),
                "--socket" => o.socket = Some(PathBuf::from(val()?)),
                "--host-id" => o.host_id = Some(val()?),
                "--hub" => o.hub = true,
                "--hub-id" => o.hub_id = Some(val()?),
                "--installed" => o.installed = true,
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
                "--auth-prompt-timeout-ms" => o.auth_prompt_timeout = Some(ms(val()?)?),
                "--export-cap-bytes" => o.export_cap = Some(bytes(val()?)?),
                "--export-host-budget-bytes" => o.export_host_budget = Some(bytes(val()?)?),
                #[cfg(feature = "panic-test")]
                "--panic-test" => o.panic_test = Some(val()?),
                other => return Err(format!("unknown option {other}")),
            }
        }
        o.data = match data {
            Some(d) => d,
            None => default_data_dir().ok_or("no home directory: pass --data <dir>")?,
        };
        o.layout = Some(
            Layout::resolve(o.data.clone(), o.run_dir.clone())
                .map_err(|e| format!("cannot prepare the runtime directory: {e}"))?,
        );
        Ok(o)
    }

    /// The resolved layout (set by [`Options::parse`]).
    pub fn layout(&self) -> &Layout {
        self.layout
            .as_ref()
            .expect("Options::parse resolves the layout")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket
            .clone()
            .unwrap_or_else(|| self.layout().socket())
    }

    pub fn endpoint(&self) -> Endpoint {
        wasit_control::local_endpoint(&self.socket_path())
    }
}

/// This machine's default host id.
pub fn default_host_id() -> String {
    wasit_runtime::machine::host_name()
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
        assert_eq!(o.socket_path(), o.layout().run.join("marsad.sock"));
        assert!(o.layout().data.starts_with("/d/machines"));
        assert_eq!(o.driver_env, [("claude".into(), "A".into(), "b=c".into())]);
        assert_eq!(o.stop_grace, Some(Duration::from_millis(500)));
        assert!(Options::parse(&["--nope".to_string()]).is_err());
    }
}
