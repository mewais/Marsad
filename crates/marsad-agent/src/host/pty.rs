//! `pty.*`: interactive shells (portable-pty) with a byte-offset output topic and bounded
//! scrollback replay.
//!
//! Each terminal has one reader thread (blocking reads of the pty master) and one writer
//! thread (fed by a bounded channel), so no async task ever blocks on the pty. The scrollback
//! and the live broadcast are updated under one short memory-only lock, which is also taken
//! to start a subscription, so replay and live output join with no gap and no duplicate.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use futures::stream::StreamExt;
use marsad_proto::pty::{PtyEvent, PtyInfo};
use marsad_proto::MarsadError;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, mpsc};
use wasit_control::{CloseReason, Cursor, EventStream};

/// Scrollback kept per terminal.
pub const SCROLLBACK: usize = 256 * 1024;
const INPUT_QUEUE: usize = 64;
const LIVE_QUEUE: usize = 512;
const INPUT_TIMEOUT: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Only plain data behind these locks; a poisoned one is still consistent.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Scroll {
    /// Offset of `data[0]`.
    start: u64,
    data: VecDeque<u8>,
    exited: Option<Option<i32>>,
}

impl Scroll {
    fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }
}

type Live = (u64, PtyEvent);

struct Pty {
    info: Mutex<PtyInfo>,
    scroll: Mutex<Scroll>,
    live: broadcast::Sender<Live>,
    input: mpsc::Sender<Vec<u8>>,
    /// Taken (closed) once the shell has exited.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

/// All terminals of this daemon. Cheap to clone.
#[derive(Clone, Default)]
pub struct Ptys {
    map: Arc<Mutex<HashMap<String, Arc<Pty>>>>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

fn new_id() -> String {
    const A: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    (0..10).map(|_| A[fastrand::usize(..32)] as char).collect()
}

impl Ptys {
    fn get(&self, id: &str) -> Result<Arc<Pty>, MarsadError> {
        lock(&self.map)
            .get(id)
            .cloned()
            .ok_or_else(|| MarsadError::NotFound {
                what: format!("terminal {id}"),
            })
    }

    pub fn list(&self) -> Vec<PtyInfo> {
        let mut v: Vec<PtyInfo> = lock(&self.map)
            .values()
            .map(|p| {
                let mut i = lock(&p.info).clone();
                let s = lock(&p.scroll);
                i.offset = s.end();
                i.exited = s.exited.flatten().or(i.exited);
                i
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Spawn a shell in `cwd`.
    pub async fn start(
        &self,
        cwd: &Path,
        session: Option<String>,
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<PtyInfo, MarsadError> {
        if cols == 0 || rows == 0 {
            return Err(MarsadError::invalid("cols and rows must be positive"));
        }
        if !tokio::fs::metadata(cwd)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return Err(MarsadError::NotFound {
                what: format!("directory {}", cwd.display()),
            });
        }
        let shell = shell
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_shell);
        let cwd_s = cwd.display().to_string();
        let cwd2 = cwd.to_path_buf();
        // Opening a pty and spawning are blocking syscalls.
        let spawned = tokio::task::spawn_blocking(move || {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| MarsadError::io(format!("openpty: {e}")))?;
            let mut cmd = CommandBuilder::new(&shell);
            cmd.cwd(&cwd2);
            cmd.env("TERM", "xterm-256color");
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| MarsadError::io(format!("spawn {shell}: {e}")))?;
            // The child holds the only slave handle: the reader sees EOF when it exits.
            drop(pair.slave);
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|e| MarsadError::io(format!("pty reader: {e}")))?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|e| MarsadError::io(format!("pty writer: {e}")))?;
            Ok::<_, MarsadError>((pair.master, child, reader, writer))
        })
        .await
        .map_err(|e| MarsadError::io(format!("pty task: {e}")))??;
        let (master, mut child, reader, writer) = spawned;
        let id = new_id();
        let (input, input_rx) = mpsc::channel(INPUT_QUEUE);
        let pty = Arc::new(Pty {
            info: Mutex::new(PtyInfo {
                id: id.clone(),
                cwd: cwd_s,
                session,
                cols,
                rows,
                offset: 0,
                exited: None,
            }),
            scroll: Mutex::new(Scroll {
                start: 0,
                data: VecDeque::new(),
                exited: None,
            }),
            live: broadcast::channel(LIVE_QUEUE).0,
            input,
            master: Mutex::new(Some(master)),
            killer: Mutex::new(child.clone_killer()),
        });
        lock(&self.map).insert(id.clone(), pty.clone());
        spawn_writer(writer, input_rx);
        let p = pty.clone();
        let read = std::thread::Builder::new()
            .name(format!("pty-read-{id}"))
            .spawn(move || pump(&p, reader))
            .map_err(|e| MarsadError::io(format!("pty reader thread: {e}")))?;
        // The exit is the shell's, not the reader's EOF: a ConPTY keeps its output pipe open
        // until the pseudo console is closed. So wait for the shell, close the master (which
        // ends the reader once it has drained everything), then report the exit last.
        let p = pty.clone();
        std::thread::Builder::new()
            .name(format!("pty-wait-{id}"))
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code() as i32);
                drop(lock(&p.master).take());
                let _ = read.join();
                let mut s = lock(&p.scroll);
                s.exited = Some(code);
                let end = s.end();
                // No subscribers is fine.
                let _ = p.live.send((end + 1, PtyEvent::Exited { code }));
            })
            .map_err(|e| MarsadError::io(format!("pty wait thread: {e}")))?;
        let info = lock(&pty.info).clone();
        Ok(info)
    }

    pub async fn input(&self, id: &str, data: String) -> Result<(), MarsadError> {
        let p = self.get(id)?;
        match tokio::time::timeout(INPUT_TIMEOUT, p.input.send(data.into_bytes())).await {
            Err(_) => Err(MarsadError::Timeout {
                what: format!("terminal {id} is not reading input"),
            }),
            Ok(Err(_)) => Err(MarsadError::invalid(format!("terminal {id} has exited"))),
            Ok(Ok(())) => Ok(()),
        }
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), MarsadError> {
        if cols == 0 || rows == 0 {
            return Err(MarsadError::invalid("cols and rows must be positive"));
        }
        let p = self.get(id)?;
        let p2 = p.clone();
        tokio::task::spawn_blocking(move || {
            lock(&p2.master)
                .as_ref()
                .ok_or_else(|| MarsadError::invalid("the terminal has exited"))?
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| MarsadError::io(format!("resize: {e}")))
        })
        .await
        .map_err(|e| MarsadError::io(format!("resize task: {e}")))??;
        let mut i = lock(&p.info);
        i.cols = cols;
        i.rows = rows;
        Ok(())
    }

    /// Kill the shell and forget the terminal.
    pub fn close(&self, id: &str) -> Result<(), MarsadError> {
        let p = lock(&self.map)
            .remove(id)
            .ok_or_else(|| MarsadError::NotFound {
                what: format!("terminal {id}"),
            })?;
        if lock(&p.scroll).exited.is_none() {
            // The shell may exit on its own at the same moment.
            if let Err(e) = lock(&p.killer).kill() {
                tracing::debug!(error = %e, "kill pty child");
            }
        }
        Ok(())
    }

    /// The output topic: replay after `from` (from the scrollback), then live.
    pub fn subscribe(&self, id: &str, from: Option<u64>) -> Result<EventStream, MarsadError> {
        let p = self.get(id)?;
        let (initial, rx, mut last) = {
            let s = lock(&p.scroll);
            let from = from.unwrap_or(0);
            let end = s.end();
            let mut initial: Vec<Live> = Vec::new();
            if from < end {
                let at = from.max(s.start);
                let skip = (at - s.start) as usize;
                let bytes: Vec<u8> = s.data.iter().skip(skip).copied().collect();
                initial.push((
                    end,
                    PtyEvent::Output {
                        offset: at,
                        data: b64(&bytes),
                    },
                ));
            }
            if let Some(code) = s.exited {
                if from <= end {
                    initial.push((end + 1, PtyEvent::Exited { code }));
                }
            }
            (initial, p.live.subscribe(), from)
        };
        let done = initial
            .iter()
            .any(|(_, e)| matches!(e, PtyEvent::Exited { .. }));
        let head = futures::stream::iter(initial.into_iter().map(Ok));
        let live = futures::stream::unfold((rx, done), move |(mut rx, done)| async move {
            if done {
                return None;
            }
            match rx.recv().await {
                Ok(ev) => {
                    let fin = matches!(ev.1, PtyEvent::Exited { .. });
                    Some((Ok(ev), (rx, fin)))
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Some((Err(CloseReason::Lagged), (rx, true)))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });
        Ok(head
            .chain(live)
            .filter_map(move |item| {
                let out = match item {
                    Ok((cursor, ev)) if cursor > last => {
                        last = cursor;
                        Some(
                            serde_json::to_value(&ev)
                                .map(|v| (Cursor::from_u64(cursor), v))
                                .map_err(|e| CloseReason::Error {
                                    error: wasit_control::RpcError::internal(e.to_string()),
                                }),
                        )
                    }
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                };
                futures::future::ready(out)
            })
            .boxed())
    }
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    let spawned = std::thread::Builder::new()
        .name("pty-write".into())
        .spawn(move || {
            while let Some(bytes) = rx.blocking_recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot start a pty writer thread");
    }
}

fn pump(p: &Pty, mut reader: Box<dyn Read + Send>) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let mut s = lock(&p.scroll);
        let offset = s.end();
        s.data.extend(&buf[..n]);
        let excess = s.data.len().saturating_sub(SCROLLBACK);
        if excess > 0 {
            s.data.drain(..excess);
            s.start += excess as u64;
        }
        let end = s.end();
        // No subscribers is fine.
        let _ = p.live.send((
            end,
            PtyEvent::Output {
                offset,
                data: b64(&buf[..n]),
            },
        ));
    }
}
