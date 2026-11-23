//! Where an export reads logs from: a host's `DiagService` over the control link ([`HostLogs`]),
//! or files of this machine through the same confined reader the sinks use ([`FileLogs`]).
//!
//! **Confinement.** A source is chosen by name from a list: what `diag.sources` returned (the
//! host decides what it offers and validates every name again when it is read), or the fixed
//! names [`FileLogs`] was built with (`daemon`, `aux`, `app`, `shim:<id>` of a session
//! directory whose name parses as a session id). No path from a caller or from a remote host is
//! ever opened, and the reader refuses any file that does not start with a diagnostics header.
//! The session journal, `session.json`, `store/`, `wasl/`, `marsad-agent.log` and `shim.stderr`
//! have no source name.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use wasit_diag::{reader, ErrorInfo};
use wasit_runtime::wire::{DiagChunk, DiagGen, DiagSource, SessionId};
use wasit_runtime::{ClientError, RuntimeClient};

use crate::diag;

/// Why a read of a source failed.
#[derive(Debug)]
pub enum Fail {
    /// One call took longer than its deadline.
    CallDeadline,
    /// The time this host may take altogether was used up.
    HostDeadline,
    /// The host (or the control link to it) answered with an error.
    Client(ClientError),
    /// Reading a file of this machine failed.
    Io(io::Error),
}

impl Fail {
    /// The word of the section's `reason`.
    pub fn word(&self) -> &'static str {
        match self {
            Fail::CallDeadline => "call_deadline",
            Fail::HostDeadline => "host_deadline",
            Fail::Client(_) | Fail::Io(_) => "read_failed",
        }
    }

    /// The error as a record field: a code, and for a host's answer only a length and a
    /// fingerprint of its text (it is written by a peer).
    pub fn info(&self) -> Option<ErrorInfo> {
        match self {
            Fail::CallDeadline | Fail::HostDeadline => None,
            Fail::Client(e) => Some(diag::foreign(&client_code(e), &e.to_string())),
            Fail::Io(e) => Some(diag::io(e)),
        }
    }

    /// The same, in words for the status of the export: the code only.
    pub fn words(&self) -> String {
        match self {
            Fail::CallDeadline => "a call took too long".into(),
            Fail::HostDeadline => "the host took too long".into(),
            Fail::Client(e) => format!("the host answered with an error ({})", client_code(e)),
            Fail::Io(e) => format!("a file could not be read ({:?})", e.kind()),
        }
    }
}

/// The machine word of a failed client call.
pub fn client_code(e: &ClientError) -> String {
    use wasit_control::CallError;
    match e {
        ClientError::Runtime(r) => r.code().to_owned(),
        ClientError::Command(_) => "command".to_owned(),
        ClientError::Call(c) => match c {
            CallError::Rpc(r) => r.code.clone(),
            CallError::Timeout => "timeout".to_owned(),
            CallError::NotConnected => "not_connected".to_owned(),
            CallError::ConnectionLost(_) => "connection_lost".to_owned(),
            CallError::Encode(_) => "encode".to_owned(),
            CallError::Decode(_) => "decode".to_owned(),
        },
    }
}

/// The time one host may take: every call has a deadline, and the calls of one host add up to
/// at most `host`. A call's time counts from its start to its end, also when several run at
/// once (the sum is then an over-count: the host gets less than its time, never more).
pub struct HostClock {
    call: Duration,
    host: Duration,
    spent: Mutex<Duration>,
}

impl HostClock {
    pub fn new(call: Duration, host: Duration) -> HostClock {
        HostClock {
            call,
            host,
            spent: Mutex::new(Duration::ZERO),
        }
    }

    fn left(&self) -> Duration {
        let spent = *self.spent.lock().unwrap_or_else(|e| e.into_inner());
        self.host.saturating_sub(spent)
    }

    /// Runs `call` within the call deadline and what is left of the host's time.
    pub async fn within<T>(
        &self,
        call: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, Fail> {
        let left = self.left();
        if left.is_zero() {
            return Err(Fail::HostDeadline);
        }
        let allowed = self.call.min(left);
        let started = Instant::now();
        let result = tokio::time::timeout(allowed, call).await;
        *self.spent.lock().unwrap_or_else(|e| e.into_inner()) += started.elapsed();
        match result {
            Ok(r) => r.map_err(Fail::Client),
            Err(_) if allowed < self.call => Err(Fail::HostDeadline),
            Err(_) => Err(Fail::CallDeadline),
        }
    }

    /// Whether any of the host's time is left.
    pub fn exhausted(&self) -> bool {
        self.left().is_zero()
    }
}

/// What a source lists.
pub struct LogSources {
    /// The host's wall clock when it answered (`None` for files of this machine).
    pub now_ms: Option<u64>,
    pub sources: Vec<DiagSource>,
}

#[async_trait]
pub trait Logs: Send + Sync {
    /// The sources and their generations (oldest first).
    async fn sources(&self) -> Result<LogSources, Fail>;
    /// Whole lines of one generation of one source (at most `max` bytes).
    async fn read(&self, source: &str, gen: u64, offset: u64, max: u32) -> Result<DiagChunk, Fail>;
}

/// A host's `DiagService`, through the hub's own routing.
pub struct HostLogs {
    pub client: RuntimeClient,
    pub clock: std::sync::Arc<HostClock>,
}

#[async_trait]
impl Logs for HostLogs {
    async fn sources(&self) -> Result<LogSources, Fail> {
        let s = self.clock.within(self.client.diag_sources()).await?;
        Ok(LogSources {
            now_ms: Some(s.now_ms),
            sources: s.sources,
        })
    }

    async fn read(&self, source: &str, gen: u64, offset: u64, max: u32) -> Result<DiagChunk, Fail> {
        self.clock
            .within(self.client.diag_read(source, gen, offset, max))
            .await
    }
}

/// One source of [`FileLogs`]: a file name in a directory.
struct FileSource {
    name: String,
    dir: PathBuf,
    file: &'static str,
}

/// Diagnostics files of this machine, read through `wasit_diag::reader` (the sinks' own reader:
/// whole lines, a generation named by its header, nothing that is not a diagnostics file).
pub struct FileLogs {
    sources: Vec<FileSource>,
}

impl FileLogs {
    /// The desktop app's log: `app.ndjson` in `diag_dir`, as the source `app`.
    pub fn app(diag_dir: PathBuf) -> FileLogs {
        FileLogs {
            sources: vec![FileSource {
                name: "app".into(),
                dir: diag_dir,
                file: "app.ndjson",
            }],
        }
    }

    /// Everything of this machine that is diagnostics: `daemon`, `aux` and `app` of `diag_dir`
    /// and `shim:<id>` of every directory of `sessions_dir` whose name is a session id.
    /// Blocking (lists the directory): call it from the blocking pool.
    pub fn machine(diag_dir: PathBuf, sessions_dir: PathBuf) -> FileLogs {
        let mut sources: Vec<FileSource> = [
            ("daemon", "daemon.ndjson"),
            ("aux", "aux.ndjson"),
            ("app", "app.ndjson"),
        ]
        .into_iter()
        .map(|(name, file)| FileSource {
            name: name.into(),
            dir: diag_dir.clone(),
            file,
        })
        .collect();
        let mut ids: Vec<(SessionId, PathBuf)> = std::fs::read_dir(&sessions_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let id = SessionId::parse(e.file_name().to_str()?)?;
                Some((id, e.path()))
            })
            .collect();
        ids.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        sources.extend(ids.into_iter().map(|(id, dir)| FileSource {
            name: format!("shim:{id}"),
            dir,
            file: "shim.log",
        }));
        FileLogs { sources }
    }
}

#[async_trait]
impl Logs for FileLogs {
    async fn sources(&self) -> Result<LogSources, Fail> {
        let wanted: Vec<(String, PathBuf, &'static str)> = self
            .sources
            .iter()
            .map(|s| (s.name.clone(), s.dir.clone(), s.file))
            .collect();
        let listed = tokio::task::spawn_blocking(move || {
            wanted
                .into_iter()
                .map(|(name, dir, file)| DiagSource {
                    name,
                    gens: reader::source(&dir, file)
                        .gens
                        .into_iter()
                        .map(|g| DiagGen {
                            gen: g.gen,
                            bytes: g.bytes,
                            first_t: g.first_t,
                            last_t: g.last_t,
                        })
                        .collect(),
                })
                .filter(|s| !s.gens.is_empty())
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Fail::Io(io::Error::other(e)))?;
        Ok(LogSources {
            now_ms: None,
            sources: listed,
        })
    }

    async fn read(&self, source: &str, gen: u64, offset: u64, max: u32) -> Result<DiagChunk, Fail> {
        let Some(s) = self.sources.iter().find(|s| s.name == source) else {
            return Err(Fail::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown diagnostics source",
            )));
        };
        let (dir, file) = (s.dir.clone(), s.file);
        let chunk = tokio::task::spawn_blocking(move || {
            reader::read(&dir, file, gen, offset, max as usize)
        })
        .await
        .map_err(|e| Fail::Io(io::Error::other(e)))?
        .map_err(Fail::Io)?;
        Ok(DiagChunk {
            data: chunk.data,
            next: chunk.next,
            eof: chunk.eof,
            gone: chunk.gone,
        })
    }
}
