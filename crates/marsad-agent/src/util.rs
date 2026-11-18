//! Small helpers shared by the services.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use marsad_proto::MarsadError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use wasit_control::RpcError;

pub fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| MarsadError::invalid(e.to_string()).into_rpc())
}

/// Params of a method whose params are optional (`null` = default).
pub fn params_or_default<T: DeserializeOwned + Default>(v: Value) -> Result<T, RpcError> {
    if v.is_null() {
        Ok(T::default())
    } else {
        params(v)
    }
}

pub fn ok<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(format!("cannot encode result: {e}")))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` / `~/`.
pub fn expand_tilde(path: &str) -> Result<PathBuf, MarsadError> {
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        let home = home_dir().ok_or_else(|| MarsadError::invalid("no home directory is known"))?;
        return Ok(if path.len() > 2 {
            home.join(&path[2..])
        } else {
            home
        });
    }
    Ok(PathBuf::from(path))
}

/// Output of a finished command.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Run a command to completion under a deadline (killed on timeout), optionally feeding
/// `stdin` from a file. stdout is bounded by `max_stdout` (excess is discarded, the process
/// still runs to completion).
pub async fn run(
    argv: &[String],
    cwd: Option<&std::path::Path>,
    env: &[(String, String)],
    stdin_file: Option<&std::path::Path>,
    deadline: Duration,
    max_stdout: usize,
) -> Result<Output, MarsadError> {
    use tokio::io::AsyncReadExt;
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| MarsadError::invalid("empty command"))?;
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let stdin_src = match stdin_file {
        Some(p) => Some(
            tokio::fs::File::open(p)
                .await
                .map_err(|e| MarsadError::io(format!("{}: {e}", p.display())))?,
        ),
        None => None,
    };
    cmd.stdin(if stdin_src.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            MarsadError::ToolMissing { tool: prog.clone() }
        } else {
            MarsadError::io(format!("{prog}: {e}"))
        }
    })?;
    let stdin = child.stdin.take();
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let work = async move {
        let feed = async {
            if let (Some(mut src), Some(mut dst)) = (stdin_src, stdin) {
                // A peer that exits early closes the pipe; its exit status tells why.
                let _ = tokio::io::copy(&mut src, &mut dst).await;
                drop(dst);
            }
        };
        let read_out = async {
            let mut buf = Vec::new();
            if let Some(o) = out.as_mut() {
                let mut chunk = [0u8; 16 * 1024];
                loop {
                    match o.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let room = max_stdout.saturating_sub(buf.len());
                            buf.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                }
            }
            buf
        };
        let read_err = async {
            let mut buf = Vec::new();
            if let Some(e) = err.as_mut() {
                // stderr is diagnostics; a read error just ends it.
                let _ = e.take(64 * 1024).read_to_end(&mut buf).await;
            }
            String::from_utf8_lossy(&buf).into_owned()
        };
        let ((), stdout, stderr) = tokio::join!(feed, read_out, read_err);
        let status = child.wait().await;
        (status, stdout, stderr)
    };
    match tokio::time::timeout(deadline, work).await {
        Err(_) => Err(MarsadError::Timeout {
            what: format!("`{}` did not finish within {deadline:?}", argv.join(" ")),
        }),
        Ok((status, stdout, stderr)) => {
            let status = status.map_err(|e| MarsadError::io(format!("{prog}: {e}")))?;
            Ok(Output {
                code: status.code(),
                stdout,
                stderr,
            })
        }
    }
}
