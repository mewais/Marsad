//! The agent's diagnostics wiring (design S4, §6.1, §6.3): which file each process role writes,
//! the panic hook, the `tracing` bridge, and the one place where free error text is made.
//!
//! * `serve` / `serve --hub` ([`Role::Daemon`], [`Role::Hub`]) install a sink on the shared
//!   `<data>/diag/aux.ndjson` and **promote** it to `daemon.ndjson` once the single-instance
//!   lock is held ([`promote_to_daemon`]); a daemon that loses the race leaves its record in
//!   the shared file and never touches `daemon.ndjson`. The daemon also keeps `aux.ndjson`
//!   within bounds (rotates it when over [`AUX_ROTATE`]).
//! * `bridge`, `stop` and the askpass helper ([`Role::Bridge`], [`Role::Stop`],
//!   [`Role::Askpass`]) are short-lived: they append to `aux.ndjson` and end with
//!   [`finish`], which waits (bounded) for their last record.
//! * `shim` installs its own sink inside `wasit_shim` (its `shim.log`); `main` does not touch
//!   diagnostics before handing over to it.
//! * `mcp` / `ask-mcp` run inside an agent CLI and know no data directory: they get no sink,
//!   only [`content_free_panic_hook`].
//!
//! **stderr.** No process writes formatted `tracing` text to a file by default: `tracing` is
//! bridged to per-minute counts ([`init_tracing`]). `MARSAD_LOG=<level>` is the developer
//! override: plain (no colour) `tracing` text on stderr and the default panic message. stderr
//! of the daemon is `marsad-agent.log` (raw crash output; see `Layout::open_log`).
//!
//! **Free text.** The only `ErrorInfo::ours` call is in [`ours`] and the only `DiagText::new`
//! calls are here too (all pinned by `tests/diag_sites.rs`): callers pass the `Display` of
//! Marsad's or Wasit's own error types (an OS reason, a path of Marsad's own files, a fixed
//! message), never anything a session, a peer or a CLI wrote. Errors that might quote stored
//! or journaled data (the Wasl node, the hub's store) go through [`foreign`] and keep only a
//! code, a length and a fingerprint. What a host printed before its report (a banner) is the
//! one other text that is kept, bounded and scrubbed of every registered secret ([`shape`]);
//! what a transport command said when a connect failed is wasit-control's record
//! (`wasit_control::ConnectDiag`).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use marsad_proto::diag::MarsadEvent;
use marsad_proto::layout::{DIAG_AUX, DIAG_DAEMON};
use tracing_subscriber::prelude::*;
use wasit_diag::{Code, DiagText, ErrorInfo, Ident, Ms, Origin, RotateShared, SinkConfig};

use crate::version;

/// The developer override: plain `tracing` text on stderr, and the default panic message.
pub const DEV_ENV: &str = "MARSAD_LOG";

/// A writer of the shared `aux.ndjson` skips (and counts) records once the file holds this much.
pub const AUX_CAP: u64 = 4 << 20;
/// The daemon rotates `aux.ndjson` when it holds more than this (one older generation is kept).
pub const AUX_ROTATE: u64 = 1 << 20;
/// `daemon.ndjson` is rotated before a record would take it past this size.
pub const DAEMON_MAX_BYTES: u64 = 8 << 20;
/// Older generations of `daemon.ndjson` kept (`.1` … `.3`: four files in all).
pub const DAEMON_GENERATIONS: u32 = 3;
/// The daemon's queue between emitters and its writer thread.
const DAEMON_QUEUE: usize = 8192;

/// The process roles that write diagnostics; the record's `src`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `serve --hub`: the GUI machine's daemon.
    Hub,
    /// `serve`: a host daemon.
    Daemon,
    /// `bridge`: the remote end of a hub link.
    Bridge,
    /// `stop`.
    Stop,
    /// The `SSH_ASKPASS` helper.
    Askpass,
}

impl Role {
    /// The `src` of this role's records.
    pub fn src(self) -> &'static str {
        match self {
            Role::Hub => "hub",
            Role::Daemon => "daemon",
            Role::Bridge => "bridge",
            Role::Stop => "stop",
            Role::Askpass => "askpass",
        }
    }

    fn is_daemon(self) -> bool {
        matches!(self, Role::Hub | Role::Daemon)
    }
}

/// `MARSAD_LOG` is set (and not empty).
pub fn developer() -> bool {
    std::env::var_os(DEV_ENV).is_some_and(|v| !v.is_empty())
}

/// The sink config of `role` writing in `dir` (every role starts on the shared file).
pub fn sink_config(role: Role, dir: PathBuf) -> SinkConfig {
    let build = Ident::new(&version::build_id());
    let mut cfg = SinkConfig::shared(Code::st(role.src()), build, dir, DIAG_AUX, AUX_CAP);
    if role.is_daemon() {
        cfg.max_bytes = DAEMON_MAX_BYTES;
        cfg.generations = DAEMON_GENERATIONS;
        cfg.queue = DAEMON_QUEUE;
        cfg.rotate_shared = Some(RotateShared {
            file: DIAG_AUX.to_owned(),
            max_bytes: AUX_ROTATE,
            generations: 1,
        });
    }
    cfg
}

/// Installs this process's sink in `dir` (created, private, by the sink's writer thread), the
/// panic hook, and, for a short-lived role, records `process.start`. A daemon announces itself
/// after it holds its lock ([`announce`]). Returns at once; an install that fails says so on
/// stderr (the only place left) and the process runs without diagnostics.
pub fn start(role: Role, dir: PathBuf) {
    match wasit_diag::install(sink_config(role, dir)) {
        Ok(()) => {
            if !role.is_daemon() {
                wasit_diag::announce_process();
            }
        }
        Err(e) => eprintln!("marsad-agent: no diagnostics: {e}"),
    }
    // Without a sink the hook records nothing and prints nothing: the payload of a panic can
    // hold content and stderr is a file.
    wasit_diag::install_panic_hook(developer());
}

/// A daemon that holds its lock: from the next record on, its records go to `daemon.ndjson`.
pub fn promote_to_daemon() {
    if let Err(e) = wasit_diag::promote(DIAG_DAEMON) {
        eprintln!("marsad-agent: diagnostics stay in the shared file: {e}");
    }
}

/// Records `process.start` (a daemon calls it after [`promote_to_daemon`], or when it lost the
/// lock race).
pub fn announce() {
    wasit_diag::announce_process();
}

/// Records `event` (never waits).
pub fn emit(event: MarsadEvent) {
    wasit_diag::emit(&event);
}

/// Waits (bounded) until this process's records are written: the last thing a process does.
pub fn finish() {
    // A timeout or a writer that cannot write changes nothing for a process that is ending.
    let _finished = wasit_diag::shutdown();
}

/// `tracing` is counted into the sink (`trace.counted` once a minute: target, level, count;
/// never a message or a field). Its text goes nowhere unless `MARSAD_LOG` is set: then plain
/// text (no colour codes) goes to stderr.
pub fn init_tracing() {
    let dev = developer();
    let text = dev.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_filter(
                tracing_subscriber::EnvFilter::try_from_env(DEV_ENV)
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
    });
    // Only fails when a subscriber is already set.
    let installed = tracing_subscriber::registry()
        .with(wasit_diag::tracing_layer())
        .with(text)
        .try_init()
        .is_ok();
    if dev && installed {
        tracing::info!("MARSAD_LOG is set: developer log text goes to stderr");
    }
}

/// The panic hook of a process that has no sink (`mcp`, `ask-mcp`: they run inside an agent CLI
/// and know no data directory): one line on stderr with where it happened and the kind and
/// length of the payload, never the payload (it can hold a tool's arguments); nothing is
/// written to any file. With `MARSAD_LOG` set the default message follows.
pub fn content_free_panic_hook(what: &'static str) {
    let previous = std::panic::take_hook();
    let dev = developer();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let (kind, len) = if let Some(s) = payload.downcast_ref::<&'static str>() {
            ("static str", s.chars().count())
        } else if let Some(s) = payload.downcast_ref::<String>() {
            ("string", s.chars().count())
        } else {
            ("other", 0)
        };
        let at = info.location().map_or_else(
            || "an unknown place".to_owned(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        // Not `eprintln!`: it panics when stderr is closed, and a panic inside the hook aborts.
        let _printed = writeln!(
            std::io::stderr(),
            "marsad-agent {what}: panicked at {at} ({kind}, {len} chars)"
        );
        if dev {
            previous(info);
        }
    }));
}

/// Milliseconds since `since`.
pub fn ms(since: Instant) -> Ms {
    Ms::of(since.elapsed())
}

/// An I/O error as a record field (its kind and OS error number, never the message).
pub fn io(e: &std::io::Error) -> ErrorInfo {
    ErrorInfo::io(e)
}

/// The `Display` of one of our own error types, as a record field (scrubbed, bounded). The one
/// `ErrorInfo::ours` call of this crate: callers pass errors whose text names an operation and
/// the OS's reason or a path of Marsad's own files, never what a session, a peer or a CLI
/// wrote (see the module documentation).
pub fn ours(code: &'static str, e: &impl std::fmt::Display) -> ErrorInfo {
    ErrorInfo::ours(Code::st(code), e)
}

/// A text that may quote data (stored documents, journals, a peer): only a code, its length and
/// a fingerprint are kept.
pub fn foreign(code: &str, text: &str) -> ErrorInfo {
    ErrorInfo::foreign(Code::new(code), text)
}

/// Transport text, bounded and scrubbed of every registered secret and project path: what a
/// host printed before its report (`shape`), and a link's last error as the link layer states
/// it, for the manifest of a diagnostics export. The one `DiagText::new(Origin::Transport, …)`
/// call of this crate; what a transport command said when a connect failed is recorded by
/// wasit-control (`wasit_control::ConnectDiag`), not here.
pub(crate) fn transport(text: &str) -> DiagText {
    DiagText::new(Origin::Transport, text)
}

/// A short look at what a host printed before its report (a banner): scrubbed first, then cut
/// to [`SHAPE_CHARS`] characters, so a registered secret is never cut in half before it is
/// masked.
pub fn shape(text: &str) -> DiagText {
    let scrubbed = transport(text);
    let head: String = scrubbed.as_str().chars().take(SHAPE_CHARS).collect();
    transport(&head)
}

/// What a host's environment capture wrote about itself (a note on why an attempt failed, how its
/// setup commands ended): scrubbed and cut to its first line (`Origin::HostShell`). Never the
/// output of the setup commands. For the export's manifest (`hub/export/facts.rs`).
pub fn host_note(text: &str) -> DiagText {
    DiagText::new(Origin::HostShell, text)
}

/// One entry of a host's `PATH`, as the manifest of an export shows it (design §6.5: the
/// host's `PATH` is included, scrubbed and bounded). Scrubbing replaces what is registered with
/// the hub (secrets, this machine's home directory); the host's own home directory is not known
/// here and shows as it is.
pub fn host_path(entry: &str) -> DiagText {
    DiagText::new(Origin::Own, entry)
}

/// Characters of a host's preamble that [`shape`] keeps.
pub const SHAPE_CHARS: usize = 120;

/// Why a startup step failed, twice: as the record's [`ErrorInfo`] and as the words `serve`
/// prints on stderr (the daemon's `marsad-agent.log`) unless `MARSAD_LOG` is set. Both carry the
/// same content-free information: the code, and the OS's reason or the path of Marsad's own
/// files where the error is ours; for an error that may quote stored data only a length.
pub struct Reason {
    pub info: ErrorInfo,
    words: String,
}

impl Reason {
    /// An error of our own making (see [`ours`]).
    pub fn ours(code: &'static str, e: &impl std::fmt::Display) -> Reason {
        let text = DiagText::new(Origin::Own, &e.to_string());
        Reason {
            info: ours(code, e),
            words: format!("{code}: {}", text.as_str()),
        }
    }

    /// An I/O error while doing `what` (a fixed phrase, optionally naming one of Marsad's own
    /// paths): its kind and OS error number, never the message.
    pub fn io(what: impl std::fmt::Display, e: &std::io::Error) -> Reason {
        let text = DiagText::new(Origin::Own, &what.to_string());
        let os = e
            .raw_os_error()
            .map_or_else(String::new, |n| format!(", os error {n}"));
        Reason {
            info: io(e),
            words: format!("{}: {:?}{os}", text.as_str(), e.kind()),
        }
    }

    /// A text that may quote data (see [`foreign`]): the code and its length.
    pub fn foreign(code: &str, text: &str) -> Reason {
        Reason {
            info: foreign(code, text),
            words: format!(
                "{code} (details withheld, {} characters; see the diagnostics file)",
                text.chars().count()
            ),
        }
    }

    /// The words for stderr.
    pub fn words(&self) -> &str {
        &self.words
    }
}
