//! The export file's writer: gzip in members, on a thread of its own.
//!
//! A gzip file may be a sequence of members; every decoder (`zcat`, `gzip -dc`, Python's
//! `gzip`) reads them as one stream. The writer compresses what it is given in members of at
//! most [`SLAB`] bytes, so a file that stops early (a crash, a full disk) is readable up to its
//! last whole member, and the size of the file is known exactly after every write, which is what
//! the size cap needs.
//!
//! Compressing and writing run on a dedicated thread (`marsad-export-gz`) fed through a bounded
//! channel: the async runtime only awaits the replies, it never compresses or touches the file.
//! Nothing here waits by itself: every call is a message and an answer.
//!
//! **The cap.** [`GzFile::put`] with `optional = true` writes the data only when the file stays
//! within `cap` minus a reserve kept for what must always fit (the lines that list what was
//! left out, and the end line); otherwise it writes nothing and says so. Data that is not
//! optional is always written. So the file never exceeds the cap by more than what the
//! mandatory lines need, and a section is in the file whole or not at all.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, oneshot};

/// Uncompressed bytes per gzip member.
pub const SLAB: usize = 1 << 20;
/// Deflate level of [`member`].
const LEVEL: u8 = 6;

/// CRC-32 (IEEE 802.3, as gzip uses it).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for b in data {
        c = TABLE[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// One gzip member holding `raw`.
pub fn member(raw: &[u8]) -> Vec<u8> {
    let deflated = miniz_oxide::deflate::compress_to_vec(raw, LEVEL);
    let mut out = Vec::with_capacity(deflated.len() + 18);
    // ID1 ID2, CM = deflate, FLG = 0, MTIME = 0, XFL = 0, OS = unknown.
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff]);
    out.extend_from_slice(&deflated);
    out.extend_from_slice(&crc32(raw).to_le_bytes());
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out
}

/// What [`GzFile::put`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// The data is in the file, which now has this many bytes.
    Written { file_bytes: u64 },
    /// The data was optional and would have taken the file past the cap: nothing was written.
    Skipped,
}

enum Msg {
    Put {
        data: Vec<u8>,
        optional: bool,
        reply: oneshot::Sender<io::Result<Put>>,
    },
    Finish {
        reply: oneshot::Sender<io::Result<u64>>,
    },
    Abort {
        reply: oneshot::Sender<()>,
    },
}

/// The file being written (a temporary name until [`GzFile::finish`]).
pub struct GzFile {
    tx: mpsc::Sender<Msg>,
}

impl GzFile {
    /// Starts writing `part` (created private and new; it becomes `done` when finished). Blocking
    /// (creates the file): call it from the blocking pool.
    pub fn create(part: PathBuf, done: PathBuf, cap: u64) -> io::Result<GzFile> {
        let file = wasit_diag::private::open(&part, |o| {
            o.write(true).create_new(true);
        })?;
        let (tx, rx) = mpsc::channel(1);
        std::thread::Builder::new()
            .name("marsad-export-gz".into())
            .spawn(move || run(rx, file, part, done, cap))?;
        Ok(GzFile { tx })
    }

    /// Compresses and writes `data` (see the module documentation for `optional`).
    pub async fn put(&self, data: Vec<u8>, optional: bool) -> io::Result<Put> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Msg::Put {
                data,
                optional,
                reply,
            })
            .await
            .map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?
    }

    /// Flushes the file to disk, renames it to its final name and returns its size.
    pub async fn finish(self) -> io::Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Msg::Finish { reply })
            .await
            .map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?
    }

    /// Ends the file without a result: what was written is deleted.
    pub async fn abort(self) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Msg::Abort { reply }).await.is_ok() {
            // The thread has gone either way once this returns.
            let _ = rx.await;
        }
    }
}

fn gone() -> io::Error {
    io::Error::other("the export's writer thread ended")
}

/// What must always fit after the optional data: the lines that list what was left out and the
/// end line.
fn reserve(cap: u64) -> u64 {
    (cap / 10).clamp(2048, 256 << 10)
}

fn run(mut rx: mpsc::Receiver<Msg>, mut file: File, part: PathBuf, done: PathBuf, cap: u64) {
    let mut len = 0u64;
    // After a failed write the file is in an unknown state: every later call says so.
    let mut failed: Option<(io::ErrorKind, String)> = None;
    let again = |f: &(io::ErrorKind, String)| io::Error::new(f.0, f.1.clone());
    while let Some(msg) = rx.blocking_recv() {
        match msg {
            Msg::Put {
                data,
                optional,
                reply,
            } => {
                let result = match &failed {
                    Some(f) => Err(again(f)),
                    None => {
                        let mut pending = Vec::new();
                        for slab in data.chunks(SLAB) {
                            pending.extend_from_slice(&member(slab));
                        }
                        if optional && len + pending.len() as u64 + reserve(cap) > cap {
                            Ok(Put::Skipped)
                        } else {
                            match file.write_all(&pending) {
                                Ok(()) => {
                                    len += pending.len() as u64;
                                    Ok(Put::Written { file_bytes: len })
                                }
                                Err(e) => {
                                    failed = Some((e.kind(), e.to_string()));
                                    Err(e)
                                }
                            }
                        }
                    }
                };
                // The caller may have gone away.
                let _ = reply.send(result);
            }
            Msg::Finish { reply } => {
                let result = match &failed {
                    Some(f) => Err(again(f)),
                    None => finish(&mut file, &part, &done, len),
                };
                if result.is_err() {
                    remove(&part);
                }
                let _ = reply.send(result);
                return;
            }
            Msg::Abort { reply } => {
                drop(file);
                remove(&part);
                let _ = reply.send(());
                return;
            }
        }
    }
    // The caller dropped the file without finishing it: nothing of it stays.
    drop(file);
    remove(&part);
}

fn finish(file: &mut File, part: &Path, done: &Path, len: u64) -> io::Result<u64> {
    file.flush()?;
    file.sync_all()?;
    std::fs::rename(part, done)?;
    Ok(len)
}

/// Deletes a file that is not wanted; one that is already gone is fine.
fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            // Nothing can be done about it here; the next export removes stale `.part` files.
            tracing::debug!(error = %e, "cannot remove a partial export");
        }
    }
}

/// Reads back what this module writes: a sequence of gzip members (no header fields, as
/// [`member`] writes them), each checked against its CRC-32 and length. For tests and for tools
/// that read an export without `zcat`.
pub fn gunzip(mut bytes: &[u8]) -> io::Result<Vec<u8>> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let mut out = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 18 || bytes[..4] != [0x1f, 0x8b, 8, 0] {
            return Err(bad("not a gzip member as this module writes them"));
        }
        let mut input = &bytes[10..];
        let mut state = InflateState::new_boxed(DataFormat::Raw);
        let mut buf = vec![0u8; 1 << 16];
        let mut member = Vec::new();
        loop {
            let r = inflate(&mut state, input, &mut buf, MZFlush::None);
            member.extend_from_slice(&buf[..r.bytes_written]);
            input = &input[r.bytes_consumed..];
            match r.status {
                Ok(MZStatus::StreamEnd) => break,
                Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
                Ok(_) => return Err(bad("a gzip member ends early")),
                Err(_) => return Err(bad("a gzip member does not inflate")),
            }
        }
        if input.len() < 8 {
            return Err(bad("a gzip member has no trailer"));
        }
        let crc = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
        let len = u32::from_le_bytes([input[4], input[5], input[6], input[7]]);
        if crc != crc32(&member) || len as usize != member.len() {
            return Err(bad("a gzip member fails its checksum"));
        }
        out.extend_from_slice(&member);
        bytes = &input[8..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_is_the_ieee_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_member_is_a_gzip_file() {
        let raw = b"{\"a\":1}\n".repeat(1000);
        let gz = member(&raw);
        assert!(gz.len() < raw.len() / 10);
        assert_eq!(gunzip(&gz).unwrap(), raw);
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mrs-gz-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn an_optional_put_that_does_not_fit_writes_nothing_and_the_cap_holds() {
        let d = dir("cap");
        let (part, done) = (d.join("x.part"), d.join("x.gz"));
        let part2 = part.clone();
        let done2 = done.clone();
        let cap = 4096;
        let gz = tokio::task::spawn_blocking(move || GzFile::create(part2, done2, cap))
            .await
            .unwrap()
            .unwrap();
        // Incompressible data, so sizes are predictable.
        let noise = |n: usize| -> Vec<u8> {
            let mut x = 0x2545F491u32;
            (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    (x >> 7) as u8
                })
                .collect()
        };
        gz.put(b"first line\n".to_vec(), false).await.unwrap();
        let small = gz.put(b"{}\n".to_vec(), true).await.unwrap();
        assert!(matches!(small, Put::Written { .. }), "{small:?}");
        let big = gz.put(noise(20_000), true).await.unwrap();
        assert_eq!(big, Put::Skipped);
        // Mandatory data goes in whatever the cap says.
        gz.put(b"end\n".to_vec(), false).await.unwrap();
        let size = gz.finish().await.unwrap();
        assert!(!part.exists());
        let bytes = std::fs::read(&done).unwrap();
        assert_eq!(bytes.len() as u64, size);
        assert!(size < cap);
        assert_eq!(gunzip(&bytes).unwrap(), b"first line\n{}\nend\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn an_aborted_file_leaves_nothing() {
        let d = dir("abort");
        let (part, done) = (d.join("x.part"), d.join("x.gz"));
        let (p2, d2) = (part.clone(), done.clone());
        let gz = tokio::task::spawn_blocking(move || GzFile::create(p2, d2, 1 << 20))
            .await
            .unwrap()
            .unwrap();
        gz.put(b"a\n".to_vec(), false).await.unwrap();
        gz.abort().await;
        assert!(!part.exists() && !done.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn a_large_put_is_split_into_members() {
        let d = dir("slabs");
        let (part, done) = (d.join("x.part"), d.join("x.gz"));
        let (p2, d2) = (part.clone(), done.clone());
        let gz = tokio::task::spawn_blocking(move || GzFile::create(p2, d2, 100 << 20))
            .await
            .unwrap()
            .unwrap();
        let raw = b"{\"k\":\"v\"}\n".repeat(SLAB / 10 * 3);
        gz.put(raw.clone(), true).await.unwrap();
        gz.finish().await.unwrap();
        assert_eq!(gunzip(&std::fs::read(&done).unwrap()).unwrap(), raw);
        let _ = std::fs::remove_dir_all(&d);
    }
}
