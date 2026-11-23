//! The export job: one run of the exporter, from the fleet's hosts to a finished file
//! (design §6.5, §6.6).
//!
//! ```text
//! 1. facts       every host, 2 at a time: link, clock, build, environment, agents, sessions
//! 2. manifest    the first line of the file: the window, the hub, every host's facts
//! 3. unavailable a `source_unavailable` line for every host that could not be read
//! 4. sections    in priority order, 2 reads at a time, written in that order:
//!                  P0  the newest generation of every source's own logs (app, daemon, aux)
//!                  P1  per active session: its trail, then its shim log
//!                  P2  older generations, newest first, one depth after the other
//! 5. end         counts, completeness, what came of every source
//! ```
//!
//! **Bounds.** A call has `call_deadline` and a host's calls add up to `host_deadline`
//! ([`HostClock`]); a host contributes at most `host_budget` raw bytes (a read stops at the
//! first chunk that reaches it: the section is cut and says so); the compressed file stays
//! within `cap` (a section is in the file whole or not at all, see [`GzFile`]). When a section
//! does not fit, it and everything after it (lower priority: older generations come last) is
//! left out and listed in `skipped` lines, not silently truncated.
//!
//! **Nothing blocks.** The job is an async task; reads are control-protocol calls under
//! deadlines or run on the blocking pool; validating and compressing what was read run on the
//! blocking pool and the writer thread. Progress is published through a callback that never
//! waits.
//!
//! **Content.** Records are copied verbatim (they are content-free by construction); a line
//! that is not a JSON object is left out and counted (`dropped`), so a host cannot put a line of
//! its own invention between two sections. The journal, `session.json`, `store/`, `wasl/`,
//! `marsad-agent.log` and `shim.stderr` have no source name and are never read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use marsad_proto::export::{
    ExportLine, ExportSource, ExportSourceState, SourceResult, EXPORTS_KEPT, EXPORT_FILE_PREFIX,
    EXPORT_FILE_SUFFIX, EXPORT_FORMAT, EXPORT_REDACTION,
};
use marsad_proto::HostId;
use wasit_diag::{Bytes, Code, ErrorInfo, Ident, Ms, TimeMs};
use wasit_runtime::wire::{DiagGen, DiagSource, SessionId};
use wasit_runtime::RuntimeClient;

use super::facts::{gather, HostRun, HostSpec, LinkInfo};
use super::gzip::{GzFile, Put};
use super::source::{Fail, HostClock, Logs};
use crate::diag;
use crate::util::now_ms;

/// The bounds of an export (design §16 item 4; the defaults are the owner's decisions).
#[derive(Debug, Clone)]
pub struct Limits {
    /// The compressed file stays within this.
    pub cap: u64,
    /// Raw bytes one source contributes (logs and trails).
    pub host_budget: u64,
    pub call_deadline: Duration,
    pub host_deadline: Duration,
    /// Bytes per `diag.read`.
    pub chunk: u32,
    /// Entries per `session.trail.page`.
    pub trail_page: u32,
    /// Entries of one session's trail.
    pub trail_entries: usize,
    /// Sessions per host whose trail and shim log are read.
    pub sessions_per_host: usize,
    /// Hosts asked their facts, and reads in flight, at a time.
    pub parallel: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            cap: 20 << 20,
            host_budget: 16 << 20,
            call_deadline: Duration::from_secs(30),
            host_deadline: Duration::from_secs(180),
            chunk: wasit_runtime::wire::DIAG_READ_MAX,
            trail_page: 2000,
            trail_entries: 20_000,
            sessions_per_host: 100,
            parallel: 2,
        }
    }
}

/// What the export is of.
pub enum Mode {
    /// The hub's fleet.
    Hub,
    /// No hub was serving: this machine's files only (`note` says why).
    LocalFiles { note: &'static str },
}

/// The hub, as the manifest names it.
pub struct HubFacts {
    pub host_id: String,
    pub build: String,
    pub target: String,
    pub machine: String,
    pub pid: u32,
}

/// What changed in an export, for whoever shows it.
pub enum Update {
    /// The sources, all waiting (first).
    Sources(Vec<ExportSource>),
    /// One source's state.
    Source(ExportSource),
}

/// Receives progress; never waits.
pub type Progress = Arc<dyn Fn(Update) + Send + Sync>;

pub struct Job {
    pub id: String,
    pub mode: Mode,
    pub since_ms: u64,
    pub include_trails: bool,
    pub hub: HubFacts,
    pub hosts: Vec<HostSpec>,
    /// The desktop app's log (`app`), when this machine has the app.
    pub app: Option<Arc<dyn Logs>>,
    /// This machine's files (`local`), for a `LocalFiles` export.
    pub local: Option<Arc<dyn Logs>>,
    /// `<data>/diag/exports`.
    pub dir: PathBuf,
    pub limits: Limits,
    pub progress: Progress,
}

/// A finished export.
#[derive(Debug, Clone)]
pub struct Finished {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub complete: bool,
    pub skipped: u32,
}

/// Why no file was produced: in words for the status, and as a record field.
#[derive(Debug)]
pub struct ExportError {
    pub words: String,
    pub info: ErrorInfo,
}

impl ExportError {
    fn io(what: &str, e: &std::io::Error) -> ExportError {
        ExportError {
            words: format!("{what}: {:?}", e.kind()),
            info: diag::io(e),
        }
    }
}

/// `marsad-diagnostics-<hub host>-<id>.ndjson.gz`; the host id reduced to file-name characters.
pub fn file_name(hub_host: &str, id: &str) -> String {
    let host: String = hub_host
        .chars()
        .take(40)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{EXPORT_FILE_PREFIX}{host}-{id}{EXPORT_FILE_SUFFIX}")
}

/// `YYYYMMDD-HHMMSSZ-xxxx`: the start time (UTC) and four random hex digits (two exports of one
/// second stay apart).
pub fn new_id(now_ms: u64) -> String {
    let secs = now_ms / 1000;
    let (y, m, d) = civil(secs / 86_400);
    let t = secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}Z-{:04x}",
        t / 3600,
        t % 3600 / 60,
        t % 60,
        fastrand::u16(..)
    )
}

/// The civil date of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

// ── the plan ─────────────────────────────────────────────────────────────────

/// One source of the export (`app`, `host:<id>`, `local`) and what is needed to read it.
struct Src {
    name: String,
    /// The word of the `export.source` event: `app`, `host` or `local`.
    word: &'static str,
    host: Option<HostId>,
    logs: Option<Arc<dyn Logs>>,
    client: Option<RuntimeClient>,
    clock: Option<Arc<HostClock>>,
    /// What the source offers, and in which order its files are planned.
    catalog: Vec<DiagSource>,
    core: Vec<String>,
    shims: Vec<String>,
    sessions: Vec<SessionId>,
    /// Raw bytes this source may still contribute.
    budget: Mutex<u64>,
    /// Raw bytes read so far (live progress).
    read: AtomicU64,
}

/// One unit of reading.
struct Work {
    src: usize,
    what: What,
}

enum What {
    Log { file: String, gen: DiagGen },
    Trail { session: SessionId },
}

/// The generations of `file` that may hold records of the window, newest first.
fn window_gens(catalog: &[DiagSource], file: &str, since: u64) -> Vec<DiagGen> {
    let mut gens: Vec<DiagGen> = catalog
        .iter()
        .find(|s| s.name == file)
        .map(|s| s.gens.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|g| g.last_t.is_some_and(|t| t >= since))
        .collect();
    gens.sort_by_key(|g| std::cmp::Reverse(g.gen));
    gens
}

/// The work of every source in priority order (see the module documentation).
fn plan(srcs: &[Src], since: u64, include_trails: bool) -> Vec<Work> {
    let mut work = Vec::new();
    let log = |work: &mut Vec<Work>, i: usize, file: &str, depth: usize| {
        if let Some(gen) = window_gens(&srcs[i].catalog, file, since)
            .into_iter()
            .nth(depth)
        {
            work.push(Work {
                src: i,
                what: What::Log {
                    file: file.to_owned(),
                    gen,
                },
            });
        }
    };
    // P0: what every source's own process wrote, newest first.
    for (i, s) in srcs.iter().enumerate() {
        for f in &s.core {
            log(&mut work, i, f, 0);
        }
    }
    // P1: the sessions, newest activity first: trail, then shim log.
    for (i, s) in srcs.iter().enumerate() {
        for id in &s.sessions {
            if include_trails && s.client.is_some() {
                work.push(Work {
                    src: i,
                    what: What::Trail {
                        session: id.clone(),
                    },
                });
            }
            let shim = format!("shim:{id}");
            if s.shims.contains(&shim) {
                log(&mut work, i, &shim, 0);
            }
        }
        if s.sessions.is_empty() {
            // `local` (no hub): every shim log there is, in the catalog's order.
            for f in &s.shims {
                log(&mut work, i, f, 0);
            }
        }
    }
    // P2: older generations, the newer of them first, one depth after the other.
    let deepest = srcs
        .iter()
        .flat_map(|s| s.catalog.iter().map(|c| c.gens.len()))
        .max()
        .unwrap_or(0);
    for depth in 1..deepest {
        for (i, s) in srcs.iter().enumerate() {
            let files: Vec<&String> = s.core.iter().chain(&s.shims).collect();
            for f in files {
                log(&mut work, i, f, depth);
            }
        }
    }
    work
}

// ── the run ──────────────────────────────────────────────────────────────────

/// What a read produced for the writer.
enum Out {
    /// All of it.
    Full,
    /// Part of it: why (a reason word).
    Cut(&'static str),
    /// None of it: why.
    Failed(&'static str),
    /// Not read: why.
    Left(&'static str),
}

struct Piece {
    src: usize,
    /// The section's name for a `skipped` line (`daemon`, `trail:<id>`, …) and its generation.
    file: String,
    gen: Option<u64>,
    line: ExportLine,
    body: Vec<u8>,
    out: Out,
    /// What the host said when the read stopped, when it said something.
    error: Option<ErrorInfo>,
}

/// What came of the planned work of one source, counted as the writer takes it in.
#[derive(Default)]
struct Acct {
    planned: usize,
    seen: usize,
    sections: u32,
    skipped: u32,
    partial: u32,
    failed: u32,
    bytes: u64,
    /// Reason word → how many sections.
    reasons: BTreeMap<&'static str, u32>,
    /// What a host first said when a read of this source stopped.
    error: Option<ErrorInfo>,
    first: Option<Instant>,
}

impl Acct {
    fn reason(&mut self, word: &'static str, error: Option<ErrorInfo>) {
        *self.reasons.entry(word).or_default() += 1;
        if self.error.is_none() {
            self.error = error;
        }
    }

    /// `host_budget (2), size_cap (1)`.
    fn reasons(&self) -> String {
        self.reasons
            .iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

struct Run<'a> {
    job: &'a Job,
    srcs: Vec<Src>,
    /// The cap was hit: everything after is left out without reading it.
    cap_hit: AtomicBool,
}

/// Why a read stopped before the end of what it was reading, and what the host said about it.
struct Stop {
    word: &'static str,
    error: Option<ErrorInfo>,
}

impl Stop {
    fn of(e: &Fail) -> Stop {
        Stop {
            word: e.word(),
            error: e.info(),
        }
    }

    fn word(word: &'static str) -> Stop {
        Stop { word, error: None }
    }
}

impl Run<'_> {
    fn publish(&self, i: usize) {
        let s = &self.srcs[i];
        (self.job.progress)(Update::Source(ExportSource {
            source: s.name.clone(),
            host: s.host.clone(),
            state: ExportSourceState::Reading,
            bytes: s.read.load(Ordering::Relaxed),
            reason: None,
        }));
    }

    /// Takes up to `want` raw bytes from the source's budget.
    fn take_budget(&self, i: usize, want: u64) -> u64 {
        let mut b = self.srcs[i]
            .budget
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let t = want.min(*b);
        *b -= t;
        t
    }

    /// Settles a reservation of `reserved` bytes against the `used` ones.
    fn settle_budget(&self, i: usize, reserved: u64, used: u64) {
        let mut b = self.srcs[i]
            .budget
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *b = if used <= reserved {
            b.saturating_add(reserved - used)
        } else {
            b.saturating_sub(used - reserved)
        };
    }

    fn budget_left(&self, i: usize) -> u64 {
        *self.srcs[i]
            .budget
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    async fn read(&self, w: &Work) -> Piece {
        match &w.what {
            What::Log { file, gen } => self.read_log(w.src, file, gen).await,
            What::Trail { session } => self.read_trail(w.src, session).await,
        }
    }

    /// A section that was not read.
    fn left(
        &self,
        src: usize,
        file: &str,
        gen: Option<u64>,
        bytes: u64,
        why: &'static str,
    ) -> Piece {
        Piece {
            src,
            file: file.to_owned(),
            gen,
            line: ExportLine::Skipped {
                source: Ident::new(&self.srcs[src].name),
                file: Ident::new(file),
                why: Code::st(why),
                gen,
                bytes: (bytes > 0).then_some(Bytes(bytes)),
            },
            body: Vec::new(),
            out: Out::Left(why),
            error: None,
        }
    }

    async fn read_log(&self, i: usize, file: &str, gen: &DiagGen) -> Piece {
        let src = &self.srcs[i];
        if self.cap_hit.load(Ordering::Acquire) {
            return self.left(i, file, Some(gen.gen), gen.bytes, "size_cap");
        }
        let Some(logs) = src.logs.clone() else {
            return self.left(i, file, Some(gen.gen), gen.bytes, "read_failed");
        };
        if src.clock.as_ref().is_some_and(|c| c.exhausted()) {
            return self.left(i, file, Some(gen.gen), gen.bytes, "host_deadline");
        }
        let take = self.take_budget(i, gen.bytes);
        if take == 0 && gen.bytes > 0 {
            return self.left(i, file, Some(gen.gen), gen.bytes, "host_budget");
        }
        let mut body: Vec<u8> = Vec::new();
        let (mut got, mut offset) = (0u64, 0u64);
        let (mut lines, mut dropped) = (0u64, 0u64);
        let mut stop: Option<Stop> = None;
        loop {
            let chunk = match logs
                .read(file, gen.gen, offset, self.job.limits.chunk)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    stop = Some(Stop::of(&e));
                    break;
                }
            };
            if chunk.gone {
                stop = Some(Stop::word("rotated_away"));
                break;
            }
            let raw = chunk.data.len() as u64;
            src.read.fetch_add(raw, Ordering::Relaxed);
            got += raw;
            offset = chunk.next;
            let (eof, empty) = (chunk.eof, raw == 0);
            // Parsing megabytes of JSON is not for a runtime thread.
            let kept = tokio::task::spawn_blocking(move || keep_records(&chunk.data))
                .await
                .unwrap_or_default();
            body.extend_from_slice(&kept.bytes);
            lines += kept.lines;
            dropped += kept.dropped;
            self.publish(i);
            if eof || empty {
                break;
            }
            if got >= take {
                stop = Some(Stop::word("host_budget"));
                break;
            }
        }
        self.settle_budget(i, take, got);
        let out = match &stop {
            None => Out::Full,
            Some(s) if body.is_empty() && s.word != "host_budget" => Out::Failed(s.word),
            Some(s) => Out::Cut(s.word),
        };
        Piece {
            src: i,
            file: file.to_owned(),
            gen: Some(gen.gen),
            line: ExportLine::Section {
                source: Ident::new(&src.name),
                file: Ident::new(file),
                gen: gen.gen,
                bytes: Bytes(body.len() as u64),
                lines,
                complete: stop.is_none(),
                reason: stop.as_ref().map(|s| Code::st(s.word)),
                error: stop.as_ref().and_then(|s| s.error.clone()),
                dropped: (dropped > 0).then_some(dropped),
            },
            body,
            out,
            error: stop.and_then(|s| s.error),
        }
    }

    async fn read_trail(&self, i: usize, session: &SessionId) -> Piece {
        let src = &self.srcs[i];
        let label = format!("trail:{session}");
        if self.cap_hit.load(Ordering::Acquire) {
            return self.left(i, &label, None, 0, "size_cap");
        }
        let (Some(client), Some(clock)) = (src.client.clone(), src.clock.clone()) else {
            return self.left(i, &label, None, 0, "read_failed");
        };
        if clock.exhausted() {
            return self.left(i, &label, None, 0, "host_deadline");
        }
        if self.budget_left(i) == 0 {
            return self.left(i, &label, None, 0, "host_budget");
        }
        // Pages come newest first; the entries of the file go oldest first.
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let (mut before, mut entries, mut raw) = (None, 0u64, 0u64);
        let mut stop: Option<Stop> = None;
        loop {
            let page = match clock
                .within(client.trail_page(session, before, self.job.limits.trail_page))
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    stop = Some(Stop::of(&e));
                    break;
                }
            };
            let mut bytes = Vec::new();
            for item in &page.entries {
                // A trail entry is made of value types: it always serializes.
                if serde_json::to_writer(&mut bytes, item).is_ok() {
                    bytes.push(b'\n');
                }
            }
            let n = bytes.len() as u64;
            raw += n;
            entries += page.entries.len() as u64;
            src.read.fetch_add(n, Ordering::Relaxed);
            self.settle_budget(i, 0, n);
            pages.push(bytes);
            self.publish(i);
            if !page.has_more {
                break;
            }
            let Some(first) = page.entries.first() else {
                // More was promised but nothing came: stop rather than ask for ever.
                stop = Some(Stop::word("read_failed"));
                break;
            };
            before = Some(first.cursor);
            if entries as usize >= self.job.limits.trail_entries {
                stop = Some(Stop::word("entries_cap"));
                break;
            }
            if self.budget_left(i) == 0 {
                stop = Some(Stop::word("host_budget"));
                break;
            }
        }
        pages.reverse();
        let body: Vec<u8> = pages.into_iter().flatten().collect();
        let out = match &stop {
            None => Out::Full,
            Some(s) if body.is_empty() => Out::Failed(s.word),
            Some(s) => Out::Cut(s.word),
        };
        Piece {
            src: i,
            file: label,
            gen: None,
            line: ExportLine::Trail {
                source: Ident::new(&src.name),
                session: Ident::new(session.as_str()),
                bytes: Bytes(raw),
                entries,
                complete: stop.is_none(),
                reason: stop.as_ref().map(|s| Code::st(s.word)),
                error: stop.as_ref().and_then(|s| s.error.clone()),
            },
            body,
            out,
            error: stop.and_then(|s| s.error),
        }
    }
}

/// The records of a chunk of lines: those that are a JSON object, whole.
#[derive(Default)]
struct Kept {
    bytes: Vec<u8>,
    lines: u64,
    dropped: u64,
}

fn keep_records(data: &str) -> Kept {
    let mut k = Kept::default();
    for line in data.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        let object =
            text.starts_with('{') && serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok();
        if object {
            k.bytes.extend_from_slice(text.as_bytes());
            k.bytes.push(b'\n');
            k.lines += 1;
        } else {
            k.dropped += 1;
        }
    }
    k
}

// ── the job ──────────────────────────────────────────────────────────────────

/// Runs the job; the file is in `job.dir` when this returns `Ok`.
pub async fn run(job: Job) -> Result<Finished, ExportError> {
    let started = Instant::now();
    let dir = job.dir.clone();
    let name = file_name(&job.hub.host_id, &job.id);
    let (part, done) = (dir.join(format!("{name}.part")), dir.join(&name));
    let cap = job.limits.cap;
    let (p2, d2) = (part, done.clone());
    let opened = tokio::task::spawn_blocking(move || {
        prepare(&dir)?;
        GzFile::create(p2, d2, cap)
    })
    .await
    .map_err(|e| ExportError::io("cannot start the export", &std::io::Error::other(e)))?
    .map_err(|e| ExportError::io("cannot create the export file", &e))?;
    match write_all(&job, &opened, started).await {
        Ok(summary) => {
            let bytes = opened
                .finish()
                .await
                .map_err(|e| ExportError::io("cannot finish the export file", &e))?;
            let keep = job.dir.clone();
            // Old exports go; one that cannot be removed is not this export's failure, and the
            // next export tries again.
            let _pruned = tokio::task::spawn_blocking(move || prune(&keep)).await;
            Ok(Finished {
                name,
                path: done,
                bytes,
                complete: summary.complete,
                skipped: summary.skipped,
            })
        }
        Err(e) => {
            opened.abort().await;
            Err(e)
        }
    }
}

/// Creates the directory (private) and removes what an earlier export left half-written.
fn prepare(dir: &Path) -> std::io::Result<()> {
    wasit_diag::private::create_dir_all(dir)?;
    for e in std::fs::read_dir(dir)?.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(EXPORT_FILE_PREFIX) && n.ends_with(".part") {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(())
}

/// Keeps the newest [`EXPORTS_KEPT`] finished exports of `dir`.
fn prune(dir: &Path) -> std::io::Result<()> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with(EXPORT_FILE_PREFIX) && n.ends_with(EXPORT_FILE_SUFFIX)
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    for (_, p) in files.into_iter().skip(EXPORTS_KEPT) {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

struct Summary {
    complete: bool,
    skipped: u32,
}

/// Writes one of the exporter's own lines (they always fit: see [`GzFile`]).
async fn line(gz: &GzFile, l: &ExportLine) -> Result<(), ExportError> {
    let mut bytes = serde_json::to_vec(l).map_err(|e| ExportError {
        words: "cannot encode an export line".into(),
        info: diag::ours("encode", &e),
    })?;
    bytes.push(b'\n');
    gz.put(bytes, false)
        .await
        .map_err(|e| ExportError::io("cannot write the export file", &e))
        .map(|_| ())
}

fn outcome_word(s: ExportSourceState) -> &'static str {
    match s {
        ExportSourceState::Waiting => "waiting",
        ExportSourceState::Reading => "reading",
        ExportSourceState::NotConnected => "not_connected",
        ExportSourceState::Done => "done",
        ExportSourceState::Partial => "partial",
        ExportSourceState::Failed => "failed",
    }
}

/// The sources of the export, in file order, and the lines for the hosts that cannot be read.
async fn sources_of(
    job: &Job,
    runs: &[HostRun],
) -> Result<(Vec<Src>, Vec<ExportLine>), ExportError> {
    let lim = &job.limits;
    let budget = || Mutex::new(lim.host_budget);
    let mut srcs: Vec<Src> = Vec::new();
    let mut unavailable: Vec<ExportLine> = Vec::new();
    let listed = |what: &'static str, e: Fail| ExportError {
        words: format!("cannot list {what}: {}", e.words()),
        info: e.info().unwrap_or_else(|| diag::foreign("list", what)),
    };
    if let Some(app) = &job.app {
        let catalog = app
            .sources()
            .await
            .map_err(|e| listed("the app's log", e))?;
        srcs.push(Src {
            name: "app".into(),
            word: "app",
            host: None,
            logs: Some(app.clone()),
            client: None,
            clock: None,
            core: vec!["app".into()],
            shims: Vec::new(),
            sessions: Vec::new(),
            catalog: catalog.sources,
            budget: budget(),
            read: AtomicU64::new(0),
        });
    }
    if let Some(local) = &job.local {
        let catalog = local
            .sources()
            .await
            .map_err(|e| listed("this machine's logs", e))?
            .sources;
        let (shims, mut core): (Vec<String>, Vec<String>) = catalog
            .iter()
            .map(|c| c.name.clone())
            .partition(|n| n.starts_with("shim:"));
        // The daemon's own log first, then the short-lived processes', then the app's.
        core.sort_by_key(|n| ["daemon", "aux", "app"].iter().position(|k| k == n));
        srcs.push(Src {
            name: "local".into(),
            word: "local",
            host: None,
            logs: Some(local.clone()),
            client: None,
            clock: None,
            core,
            shims,
            sessions: Vec::new(),
            catalog,
            budget: budget(),
            read: AtomicU64::new(0),
        });
    }
    for r in runs {
        let name = format!("host:{}", r.spec.id);
        let readable = r.logs.is_some() && r.unreadable.is_none();
        let has = |f: &str| r.sources.iter().any(|s| s.name == f);
        let core: Vec<String> = ["daemon", "aux"]
            .into_iter()
            .filter(|f| has(f))
            .map(str::to_owned)
            .collect();
        let shims: Vec<String> = r
            .sessions
            .iter()
            .map(|id| format!("shim:{id}"))
            .filter(|n| has(n))
            .collect();
        match (&r.spec.link, &r.unreadable) {
            (LinkInfo::Down { word, last_error }, _) => {
                unavailable.push(ExportLine::SourceUnavailable {
                    source: Ident::new(&name),
                    link_state: Code::new(word),
                    last_error: last_error.as_deref().map(diag::transport),
                    error: None,
                })
            }
            (LinkInfo::Connected { word }, Some(e)) => {
                unavailable.push(ExportLine::SourceUnavailable {
                    source: Ident::new(&name),
                    link_state: Code::new(word),
                    last_error: None,
                    error: e.info(),
                })
            }
            _ => {}
        }
        srcs.push(Src {
            name,
            word: "host",
            host: Some(r.spec.id.clone()),
            logs: readable.then(|| r.logs.clone()).flatten(),
            client: readable.then(|| r.spec.client.clone()).flatten(),
            clock: Some(r.clock.clone()),
            catalog: r.sources.clone(),
            core,
            shims,
            sessions: r.sessions.clone(),
            budget: budget(),
            read: AtomicU64::new(0),
        });
    }
    Ok((srcs, unavailable))
}

async fn write_all(job: &Job, gz: &GzFile, started: Instant) -> Result<Summary, ExportError> {
    let lim = &job.limits;
    let created = now_ms();
    diag::emit(marsad_proto::diag::MarsadEvent::ExportStart {
        id: Ident::new(&job.id),
        since_ms: TimeMs(job.since_ms),
        hosts: u32::try_from(job.hosts.len()).unwrap_or(u32::MAX),
        include_trails: job.include_trails,
        mode: Code::st(match job.mode {
            Mode::Hub => "hub",
            Mode::LocalFiles { .. } => "local_files",
        }),
    });

    // 1. facts
    let runs: Vec<HostRun> = stream::iter(job.hosts.iter().cloned())
        .map(|h| {
            gather(
                h,
                job.since_ms,
                lim.sessions_per_host,
                lim.call_deadline,
                lim.host_deadline,
            )
        })
        .buffered(lim.parallel.max(1))
        .collect()
        .await;
    let (srcs, unavailable) = sources_of(job, &runs).await?;
    let status = |s: &Src, state, reason: Option<String>| ExportSource {
        source: s.name.clone(),
        host: s.host.clone(),
        state,
        bytes: s.read.load(Ordering::Relaxed),
        reason,
    };
    (job.progress)(Update::Sources(
        srcs.iter()
            .map(|s| status(s, ExportSourceState::Waiting, None))
            .collect(),
    ));

    // 2. the manifest
    let (mode, note) = match &job.mode {
        Mode::Hub => (Code::st("hub"), None),
        Mode::LocalFiles { note } => (Code::st("local_files"), Some(Code::st(note))),
    };
    line(
        gz,
        &ExportLine::Manifest {
            format: EXPORT_FORMAT,
            redaction: EXPORT_REDACTION,
            id: Ident::new(&job.id),
            created: TimeMs(created),
            since: TimeMs(job.since_ms),
            until: TimeMs(created),
            mode,
            include_trails: job.include_trails,
            cap_bytes: Bytes(lim.cap),
            host_budget_bytes: Bytes(lim.host_budget),
            hub_host: Ident::new(&job.hub.host_id),
            hub_build: Ident::new(&job.hub.build),
            hub_target: Ident::new(&job.hub.target),
            hub_os: Ident::new(std::env::consts::OS),
            hub_arch: Ident::new(std::env::consts::ARCH),
            hosts: runs.iter().map(|r| r.facts.clone()).collect(),
            hub_machine: Some(Ident::new(&job.hub.machine)),
            hub_pid: Some(job.hub.pid),
            note,
        },
    )
    .await?;

    // 3. hosts that could not be read
    for l in &unavailable {
        line(gz, l).await?;
    }

    // 4. the sections
    let work = plan(&srcs, job.since_ms, job.include_trails);
    let mut accts: Vec<Acct> = srcs.iter().map(|_| Acct::default()).collect();
    for w in &work {
        accts[w.src].planned += 1;
    }
    let mut verdicts: Vec<Option<(ExportSourceState, Option<String>)>> =
        srcs.iter().map(|_| None).collect();
    // A source is decided when all its planned work has been written (or was left out).
    let decide = |i: usize,
                  srcs: &[Src],
                  accts: &[Acct],
                  verdicts: &mut [Option<(ExportSourceState, Option<String>)>]| {
        let (s, a) = (&srcs[i], &accts[i]);
        let (state, reason) = verdict(s, a, &runs);
        verdicts[i] = Some((state, reason.clone()));
        (job.progress)(Update::Source(status(s, state, reason)));
        diag::emit(marsad_proto::diag::MarsadEvent::ExportSource {
            id: Ident::new(&job.id),
            source: Code::st(s.word),
            host: s.host.as_ref().map(|h| Ident::new(h.as_str())),
            outcome: Code::st(outcome_word(state)),
            bytes: Bytes(a.bytes),
            sections: a.sections,
            skipped: a.skipped,
            ms: Ms::of(a.first.map_or(Duration::ZERO, |t| t.elapsed())),
            error: a
                .error
                .clone()
                .or_else(|| run_of(s, &runs).and_then(|r| r.unreadable.as_ref()?.info())),
        });
    };
    // Sources with nothing to read (or that cannot be read) are decided now.
    for i in 0..srcs.len() {
        if accts[i].planned == 0 {
            decide(i, &srcs, &accts, &mut verdicts);
        }
    }

    let run = Run {
        job,
        srcs,
        cap_hit: AtomicBool::new(false),
    };
    // Sections, skipped, cut short, raw bytes.
    let mut total = (0u32, 0u32, 0u32, 0u64);
    let work = &work;
    let run_ref = &run;
    let mut pieces = stream::iter(0..work.len())
        .map(move |k| run_ref.read(&work[k]))
        .buffered(lim.parallel.max(1));
    while let Some(p) = pieces.next().await {
        let a = &mut accts[p.src];
        a.first.get_or_insert_with(Instant::now);
        match p.out {
            // Not read: the line says why.
            Out::Left(why) => {
                a.skipped += 1;
                total.1 += 1;
                a.reason(why, None);
                line(gz, &p.line).await?;
            }
            _ => {
                let mut buf = serde_json::to_vec(&p.line).map_err(|e| ExportError {
                    words: "cannot encode an export line".into(),
                    info: diag::ours("encode", &e),
                })?;
                buf.push(b'\n');
                buf.extend_from_slice(&p.body);
                let raw = p.body.len() as u64;
                // What follows a section that did not fit is lower priority: it is left out
                // too, also when it was read before the cap was hit (reads run ahead).
                let put = if run.cap_hit.load(Ordering::Acquire) {
                    Put::Skipped
                } else {
                    gz.put(buf, true)
                        .await
                        .map_err(|e| ExportError::io("cannot write the export file", &e))?
                };
                match put {
                    Put::Written { .. } => {
                        a.sections += 1;
                        total.0 += 1;
                        a.bytes += raw;
                        total.3 += raw;
                        match p.out {
                            Out::Cut(why) => {
                                a.partial += 1;
                                total.2 += 1;
                                a.reason(why, p.error);
                            }
                            Out::Failed(why) => {
                                a.failed += 1;
                                a.partial += 1;
                                total.2 += 1;
                                a.reason(why, p.error);
                            }
                            Out::Full | Out::Left(_) => {}
                        }
                    }
                    Put::Skipped => {
                        run.cap_hit.store(true, Ordering::Release);
                        a.skipped += 1;
                        total.1 += 1;
                        a.reason("size_cap", None);
                        line(
                            gz,
                            &ExportLine::Skipped {
                                source: Ident::new(&run.srcs[p.src].name),
                                file: Ident::new(&p.file),
                                why: Code::st("size_cap"),
                                gen: p.gen,
                                bytes: (raw > 0).then_some(Bytes(raw)),
                            },
                        )
                        .await?;
                    }
                }
            }
        }
        let a = &mut accts[p.src];
        a.seen += 1;
        if a.seen == a.planned {
            decide(p.src, &run.srcs, &accts, &mut verdicts);
        }
    }
    drop(pieces);

    // 5. the end
    let mut complete = true;
    let mut sources = Vec::new();
    for (i, s) in run.srcs.iter().enumerate() {
        let (state, _) = verdicts[i]
            .clone()
            .unwrap_or((ExportSourceState::Failed, None));
        complete &= state == ExportSourceState::Done;
        sources.push(SourceResult::Result {
            source: Ident::new(&s.name),
            outcome: Code::st(outcome_word(state)),
            bytes: Bytes(accts[i].bytes),
            sections: accts[i].sections,
            skipped: accts[i].skipped,
            reason: end_reason(state, &run.srcs[i], &accts[i], &runs),
        });
    }
    line(
        gz,
        &ExportLine::End {
            sections: total.0,
            skipped: total.1,
            partial: total.2,
            complete,
            bytes: Bytes(total.3),
            ms: Ms::of(started.elapsed()),
            sources,
        },
    )
    .await?;
    diag::emit(marsad_proto::diag::MarsadEvent::ExportDone {
        id: Ident::new(&job.id),
        bytes: Bytes(total.3),
        complete,
        skipped: total.1,
        ms: Ms::of(started.elapsed()),
        error: None,
    });
    Ok(Summary {
        complete,
        skipped: total.1,
    })
}

/// The host (if any) that `s` is the source of.
fn run_of<'a>(s: &Src, runs: &'a [HostRun]) -> Option<&'a HostRun> {
    runs.iter().find(|r| s.host.as_ref() == Some(&r.spec.id))
}

/// How a source ended, as a state and in words (reason words and counts).
fn verdict(s: &Src, a: &Acct, runs: &[HostRun]) -> (ExportSourceState, Option<String>) {
    if let Some(r) = run_of(s, runs) {
        if !r.spec.link.connected() {
            return (
                ExportSourceState::NotConnected,
                Some(r.spec.link.word().to_owned()),
            );
        }
        if let Some(e) = &r.unreadable {
            return (ExportSourceState::Failed, Some(e.words()));
        }
    }
    if a.planned == 0 {
        return (
            ExportSourceState::Done,
            Some("nothing to read in the window".into()),
        );
    }
    // Everything that was written was a failed read: nothing of the source is in the file.
    if a.failed > 0 && a.failed == a.sections {
        return (ExportSourceState::Failed, Some(a.reasons()));
    }
    if a.partial > 0 || a.skipped > 0 {
        return (ExportSourceState::Partial, Some(a.reasons()));
    }
    (ExportSourceState::Done, None)
}

/// The reason word of a source's result in the `end` line.
fn end_reason(state: ExportSourceState, s: &Src, a: &Acct, runs: &[HostRun]) -> Option<Code> {
    match state {
        ExportSourceState::Done => None,
        ExportSourceState::NotConnected => run_of(s, runs).map(|r| Code::new(r.spec.link.word())),
        _ => a
            .reasons
            .keys()
            .next()
            .map(|w| Code::st(w))
            .or_else(|| Some(Code::st("read_failed"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_the_utc_start_time_and_four_hex_digits() {
        // 2026-11-23 10:15:30 UTC.
        let id = new_id(1_795_428_930_000);
        assert!(id.starts_with("20261123-101530Z-"), "{id}");
        assert_eq!(id.len(), "20261123-101530Z-".len() + 4);
        // The epoch, a leap day and the start of a year.
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(20_089), (2025, 1, 1));
    }

    #[test]
    fn a_file_name_keeps_only_name_characters_of_the_host() {
        assert_eq!(
            file_name("my host/x", "20261123-101530Z-00ff"),
            "marsad-diagnostics-my_host_x-20261123-101530Z-00ff.ndjson.gz"
        );
    }

    #[test]
    fn only_json_objects_are_records() {
        let k = keep_records("{\"a\":1}\nnot json\n{\"line\":\"end\"}\n[1]\n\n{\"b\":");
        assert_eq!(k.lines, 2);
        assert_eq!(k.dropped, 3);
        assert_eq!(k.bytes, b"{\"a\":1}\n{\"line\":\"end\"}\n");
    }

    fn gens(spec: &[(u64, Option<u64>)]) -> Vec<DiagGen> {
        spec.iter()
            .map(|(gen, last)| DiagGen {
                gen: *gen,
                bytes: 10,
                first_t: *last,
                last_t: *last,
            })
            .collect()
    }

    fn cat(n: &str, g: &[(u64, Option<u64>)]) -> DiagSource {
        DiagSource {
            name: n.into(),
            gens: gens(g),
        }
    }

    #[test]
    fn the_window_keeps_generations_with_records_in_it_newest_first() {
        let c = vec![cat(
            "daemon",
            &[(1, Some(50)), (2, Some(150)), (3, Some(250)), (4, None)],
        )];
        let g: Vec<u64> = window_gens(&c, "daemon", 100)
            .iter()
            .map(|g| g.gen)
            .collect();
        assert_eq!(g, [3, 2]);
        assert!(window_gens(&c, "aux", 100).is_empty());
    }

    fn src(name: &str, core: &[&str], catalog: Vec<DiagSource>) -> Src {
        Src {
            name: name.into(),
            word: "host",
            host: None,
            logs: None,
            client: None,
            clock: None,
            catalog,
            core: core.iter().map(|s| s.to_string()).collect(),
            shims: Vec::new(),
            sessions: Vec::new(),
            budget: Mutex::new(0),
            read: AtomicU64::new(0),
        }
    }

    #[test]
    fn newest_generations_come_first_and_older_ones_last() {
        let a = src(
            "host:a",
            &["daemon", "aux"],
            vec![
                cat("daemon", &[(1, Some(200)), (2, Some(300)), (3, Some(400))]),
                cat("aux", &[(1, Some(300))]),
            ],
        );
        let b = src(
            "host:b",
            &["daemon"],
            vec![cat("daemon", &[(7, Some(400))])],
        );
        let order: Vec<(usize, String, u64)> = plan(&[a, b], 100, true)
            .into_iter()
            .map(|w| match w.what {
                What::Log { file, gen } => (w.src, file, gen.gen),
                What::Trail { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            order,
            [
                (0, "daemon".into(), 3),
                (0, "aux".into(), 1),
                (1, "daemon".into(), 7),
                (0, "daemon".into(), 2),
                (0, "daemon".into(), 1),
            ]
        );
    }

    // ── whole runs over fake logs ────────────────────────────────────────────

    use std::collections::HashMap;

    use async_trait::async_trait;
    use wasit_runtime::wire::DiagChunk;

    use super::super::source::LogSources;

    /// Logs held in memory: `(source, gen)` → the file's text. Reads honour `max` (whole lines).
    struct Fake {
        files: HashMap<(String, u64), String>,
        /// Reads of this `(source, gen)` fail from the n-th on.
        fail_from: Option<((String, u64), usize)>,
        reads: Mutex<HashMap<(String, u64), usize>>,
    }

    impl Fake {
        fn new(files: &[(&str, u64, String)]) -> Fake {
            Fake {
                files: files
                    .iter()
                    .map(|(s, g, t)| ((s.to_string(), *g), t.clone()))
                    .collect(),
                fail_from: None,
                reads: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl Logs for Fake {
        async fn sources(&self) -> Result<LogSources, Fail> {
            let mut by: BTreeMap<String, Vec<DiagGen>> = BTreeMap::new();
            for ((s, g), t) in &self.files {
                by.entry(s.clone()).or_default().push(DiagGen {
                    gen: *g,
                    bytes: t.len() as u64,
                    first_t: Some(1_000 + g),
                    last_t: Some(1_000 + g),
                });
            }
            Ok(LogSources {
                now_ms: None,
                sources: by
                    .into_iter()
                    .map(|(name, mut gens)| {
                        gens.sort_by_key(|g| g.gen);
                        DiagSource { name, gens }
                    })
                    .collect(),
            })
        }

        async fn read(
            &self,
            source: &str,
            gen: u64,
            offset: u64,
            max: u32,
        ) -> Result<DiagChunk, Fail> {
            let key = (source.to_owned(), gen);
            let n = {
                let mut r = self.reads.lock().unwrap();
                let n = r.entry(key.clone()).or_default();
                *n += 1;
                *n
            };
            if let Some((k, from)) = &self.fail_from {
                if *k == key && n >= *from {
                    return Err(Fail::Io(std::io::Error::other("disk on fire")));
                }
            }
            let Some(text) = self.files.get(&key) else {
                return Ok(DiagChunk {
                    data: String::new(),
                    next: offset,
                    eof: true,
                    gone: true,
                });
            };
            let rest = &text[offset as usize..];
            let mut end = 0;
            for l in rest.split_inclusive('\n') {
                if end + l.len() > max as usize {
                    break;
                }
                end += l.len();
            }
            Ok(DiagChunk {
                data: rest[..end].to_owned(),
                next: offset + end as u64,
                eof: offset + end as u64 >= text.len() as u64,
                gone: false,
            })
        }
    }

    /// `n` lines of about 100 bytes: a JSON object each.
    fn records(tag: &str, n: usize) -> String {
        (0..n)
            .map(|i| format!("{{\"seq\":{i},\"ev\":\"{tag}\",\"pad\":\"{:060}\"}}\n", i))
            .collect()
    }

    /// Incompressible records (a pseudo-random base64-ish text in each).
    fn noise(n: usize) -> String {
        let mut x = 0x9E37_79B9u32;
        let mut line = |i: usize| {
            let pad: String = (0..90)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                        [(x >> 9) as usize % 64] as char
                })
                .collect();
            format!("{{\"seq\":{i},\"pad\":\"{pad}\"}}\n")
        };
        (0..n).map(&mut line).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mrs-job-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn job(dir: &Path, logs: Fake, limits: Limits) -> Job {
        Job {
            id: "20261123-000000Z-0001".into(),
            mode: Mode::LocalFiles { note: "no_daemon" },
            since_ms: 500,
            include_trails: false,
            hub: HubFacts {
                host_id: "hub".into(),
                build: "b".into(),
                target: "t".into(),
                machine: "m".into(),
                pid: 1,
            },
            hosts: Vec::new(),
            app: None,
            local: Some(Arc::new(logs)),
            dir: dir.to_path_buf(),
            limits,
            progress: Arc::new(|_| {}),
        }
    }

    /// The export's lines: the exporter's own (`line` key) and the records between them.
    fn read_back(f: &Finished) -> Vec<serde_json::Value> {
        let text = super::super::gzip::gunzip(&std::fs::read(&f.path).unwrap()).unwrap();
        String::from_utf8(text)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn own<'a>(lines: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
        lines.iter().filter(|l| l["line"] == kind).collect()
    }

    #[tokio::test]
    async fn a_whole_export_is_manifest_sections_newest_first_and_an_end() {
        let dir = scratch("whole");
        let logs = Fake::new(&[
            ("daemon", 1, records("old", 3)),
            ("daemon", 2, records("new", 5)),
            ("aux", 1, records("aux", 2)),
            ("shim:abcdefghijkl", 1, records("shim", 4)),
        ]);
        let f = run(job(&dir, logs, Limits::default())).await.unwrap();
        assert!(f.complete && f.skipped == 0, "{f:?}");
        let lines = read_back(&f);
        assert_eq!(lines[0]["line"], "manifest");
        assert_eq!(lines[0]["mode"], "local_files");
        assert_eq!(lines[0]["note"], "no_daemon");
        assert_eq!(lines[0]["hub_host"], "hub");
        let sections: Vec<(String, u64)> = own(&lines, "section")
            .iter()
            .map(|s| {
                (
                    s["file"].as_str().unwrap().to_owned(),
                    s["gen"].as_u64().unwrap(),
                )
            })
            .collect();
        // Own logs newest first, then the shim, then the older generation.
        assert_eq!(
            sections,
            [
                ("daemon".to_owned(), 2),
                ("aux".to_owned(), 1),
                ("shim:abcdefghijkl".to_owned(), 1),
                ("daemon".to_owned(), 1)
            ]
        );
        // The records follow their section verbatim.
        let i = lines.iter().position(|l| l["line"] == "section").unwrap();
        assert_eq!(lines[i + 1]["ev"], "new");
        assert_eq!(lines[i + 5]["seq"], 4);
        let end = own(&lines, "end")[0];
        assert_eq!(
            (end["sections"].as_u64(), end["skipped"].as_u64()),
            (Some(4), Some(0))
        );
        assert_eq!(end["complete"], true);
        assert_eq!(end["sources"][0]["source"], "local");
        assert_eq!(end["sources"][0]["outcome"], "done");
        assert_eq!(lines.last().unwrap()["line"], "end");
        // Only the file: no part is left, and its name follows the pattern.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, std::slice::from_ref(&f.name));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_section_that_does_not_fit_the_cap_is_listed_with_everything_after_it() {
        let dir = scratch("cap");
        let logs = Fake::new(&[
            ("daemon", 2, records("fits", 2)),
            ("aux", 1, noise(400)),
            ("daemon", 1, records("later", 2)),
        ]);
        let limits = Limits {
            cap: 8000,
            ..Limits::default()
        };
        let f = run(job(&dir, logs, limits)).await.unwrap();
        assert!(!f.complete);
        assert_eq!(f.skipped, 2);
        assert!(f.bytes < 8000, "{} bytes", f.bytes);
        let lines = read_back(&f);
        let sections = own(&lines, "section");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0]["file"], "daemon");
        // The big one did not fit; the small older one is left out with it, unread.
        let skipped = own(&lines, "skipped");
        let left: Vec<(&str, &str)> = skipped
            .iter()
            .map(|s| (s["file"].as_str().unwrap(), s["why"].as_str().unwrap()))
            .collect();
        assert_eq!(left, [("aux", "size_cap"), ("daemon", "size_cap")]);
        assert_eq!(skipped[0]["gen"], 1);
        assert!(skipped[0]["bytes"].as_u64().unwrap() > 10_000);
        let end = own(&lines, "end")[0];
        assert_eq!(end["complete"], false);
        assert_eq!(end["skipped"], 2);
        assert_eq!(end["sources"][0]["outcome"], "partial");
        assert_eq!(end["sources"][0]["reason"], "size_cap");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_read_that_reaches_the_budget_is_cut_and_says_so() {
        let dir = scratch("budget");
        let logs = Fake::new(&[("daemon", 1, records("many", 30))]);
        let limits = Limits {
            chunk: 400,
            host_budget: 900,
            ..Limits::default()
        };
        let f = run(job(&dir, logs, limits)).await.unwrap();
        assert!(!f.complete);
        let lines = read_back(&f);
        let s = own(&lines, "section")[0];
        assert_eq!(s["complete"], false);
        assert_eq!(s["reason"], "host_budget");
        // Whole lines only, at most the budget plus the chunk that crossed it.
        let n = s["lines"].as_u64().unwrap();
        assert!((8..=12).contains(&n), "{n} lines");
        let end = own(&lines, "end")[0];
        assert_eq!(end["partial"], 1);
        assert_eq!(end["sources"][0]["outcome"], "partial");
        assert_eq!(end["sources"][0]["reason"], "host_budget");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_read_that_fails_midway_keeps_what_it_read_and_a_source_that_only_fails_is_failed() {
        let dir = scratch("fail");
        let mut logs = Fake::new(&[
            ("daemon", 1, records("half", 30)),
            ("aux", 1, records("never", 3)),
        ]);
        logs.fail_from = Some((("daemon".into(), 1), 3));
        let limits = Limits {
            chunk: 400,
            ..Limits::default()
        };
        let f = run(job(&dir, logs, limits)).await.unwrap();
        let lines = read_back(&f);
        let s = own(&lines, "section")
            .into_iter()
            .find(|s| s["file"] == "daemon")
            .unwrap();
        assert_eq!(s["complete"], false);
        assert_eq!(s["reason"], "read_failed");
        assert_eq!(s["error"]["code"], "other");
        assert!(s["lines"].as_u64().unwrap() > 0);
        let end = own(&lines, "end")[0];
        // One section was cut, the other read whole: partial, not failed.
        assert_eq!(end["sources"][0]["outcome"], "partial");

        // Nothing readable at all: failed, each section saying why.
        let dir2 = scratch("fail2");
        let mut logs = Fake::new(&[("daemon", 1, records("x", 3))]);
        logs.fail_from = Some((("daemon".into(), 1), 1));
        let f = run(job(&dir2, logs, Limits::default())).await.unwrap();
        let lines = read_back(&f);
        let s = own(&lines, "section")[0];
        assert_eq!(
            (s["lines"].as_u64(), s["reason"].as_str()),
            (Some(0), Some("read_failed"))
        );
        assert_eq!(own(&lines, "end")[0]["sources"][0]["outcome"], "failed");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[tokio::test]
    async fn lines_that_are_not_json_objects_never_enter_the_file() {
        let dir = scratch("dropped");
        let text = format!(
            "{}not json at all\n{{\"line\":\"end\",\"forged\":true}}\n[1,2]\n",
            records("ok", 2)
        );
        let logs = Fake::new(&[("daemon", 1, text)]);
        let f = run(job(&dir, logs, Limits::default())).await.unwrap();
        let lines = read_back(&f);
        let s = own(&lines, "section")[0];
        assert_eq!(
            (s["lines"].as_u64(), s["dropped"].as_u64()),
            (Some(3), Some(2))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn only_the_newest_exports_are_kept() {
        let dir = scratch("keep");
        for n in 0..5 {
            let mut j = job(
                &dir,
                Fake::new(&[("daemon", 1, records("k", 1))]),
                Limits::default(),
            );
            j.id = format!("20261123-00000{n}Z-000{n}");
            run(j).await.unwrap();
        }
        let kept: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept.len(), EXPORTS_KEPT, "{kept:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
