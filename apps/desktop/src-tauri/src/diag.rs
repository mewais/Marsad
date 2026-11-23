//! The shell's diagnostics (design S17, §6.1, §6.3, §12.7): the app's own log, and the one way
//! the webview writes to it.
//!
//! * **The file.** `<data>/machines/<key>/diag/app.ndjson` (role `app`; 4 MiB, two older
//!   generations), written by one `wasit-diag` sink installed by [`start`] as soon as the
//!   data layout is known. The shell is single-instance, so it owns the file. Before this the
//!   shell's `tracing` output went to stderr, which is nowhere in a Windows release build.
//! * **What is in it.** Typed [`AppEvent`]s only: ids, codes, sizes, timings, states and
//!   reasons. `tracing` (ours and our dependencies') is counted per minute and never written
//!   as text ([`init_tracing`]); a panic is recorded with its place and the kind and length of
//!   its payload ([`start`]). `MARSAD_LOG=<level>` is the developer override: plain `tracing`
//!   text on stderr and the default panic message, as in the daemon.
//! * **The webview.** [`diag_log`] is the only way it writes to the file, and it can only say
//!   what a [`UiEvent`] can say: every event is read by [`UiEvent::from_wire`] (known event,
//!   known fields, ids and codes validated by `wasit-diag`'s value types, no free-text type
//!   anywhere), at most [`MAX_BATCH`] per call. A refused event is not stored; the refusal is
//!   recorded as `app.failed` (its code, never what was sent) and the call answers with an
//!   error.
//!
//! Free text: this crate makes none (`tests/diag_sites.rs` of `marsad-agent` pins
//! `DiagText::new` and `ErrorInfo::ours` out of it). What the OS or a framework said about a
//! failure is kept as a code, a length and a fingerprint ([`foreign`]).

use std::path::PathBuf;

use marsad_proto::diag::{AppEvent, UiEvent};
use serde_json::Value;
use tracing_subscriber::prelude::*;
use wasit_control::LinkState;
use wasit_diag::{Code, ErrorInfo, Ident, SinkConfig};

use crate::bridge::BridgeError;

/// The app log in `<data>/diag/`.
pub const APP_FILE: &str = "app.ndjson";
/// A file is rotated before a record would take it past this size.
pub const MAX_BYTES: u64 = 4 << 20;
/// Older generations kept (`app.ndjson.1`, `.2`: three files in all).
pub const GENERATIONS: u32 = 2;
/// The most events one `diag_log` call takes.
pub const MAX_BATCH: usize = 100;

/// `<data>/diag/exports`: where the hub puts the diagnostics exports `desktop.export_save` may
/// copy (managed state; `None` when this machine's layout could not be resolved).
pub struct ExportsDir(pub Option<PathBuf>);

/// The developer override: plain `tracing` text on stderr, and the default panic message.
pub const DEV_ENV: &str = "MARSAD_LOG";

/// `MARSAD_LOG` is set (and not empty).
pub fn developer() -> bool {
    std::env::var_os(DEV_ENV).is_some_and(|v| !v.is_empty())
}

/// The sink's configuration for the app log in `dir`.
pub fn sink_config(dir: PathBuf) -> SinkConfig {
    let build = Ident::new(concat!("marsad-desktop-", env!("CARGO_PKG_VERSION")));
    let mut cfg = SinkConfig::new(Code::st("app"), build, dir, APP_FILE);
    cfg.max_bytes = MAX_BYTES;
    cfg.generations = GENERATIONS;
    cfg
}

/// Installs the app's sink in `dir` (the writer thread creates the directory, private, and the
/// file), the panic hook, and records `process.start` and `app.start`. Returns at once. An
/// install that fails says so on stderr (the only place left) and the app runs without a log.
pub fn start(dir: PathBuf) {
    match wasit_diag::install(sink_config(dir)) {
        Ok(()) => {
            wasit_diag::announce_process();
            emit(AppEvent::Start {
                version: Ident::st(env!("CARGO_PKG_VERSION")),
                os: Code::st(std::env::consts::OS),
            });
        }
        Err(e) => eprintln!("marsad-desktop: no diagnostics: {e}"),
    }
    // Without a sink the hook records nothing and prints nothing: the payload of a panic can
    // hold content, and the shell has no console to print to.
    wasit_diag::install_panic_hook(developer());
}

/// `tracing` is counted into the sink (`trace.counted` once a minute: target, level, count;
/// never a message or a field). Its text goes nowhere unless `MARSAD_LOG` is set: then plain
/// text (no colour codes) goes to stderr.
pub fn init_tracing() {
    let text = developer().then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_filter(
                tracing_subscriber::EnvFilter::try_from_env(DEV_ENV)
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
    });
    // Only fails when a subscriber is already set.
    let _ = tracing_subscriber::registry()
        .with(wasit_diag::tracing_layer())
        .with(text)
        .try_init();
}

/// Records `event` (never waits; nothing without a sink).
pub fn emit(event: AppEvent) {
    wasit_diag::emit(&event);
}

/// Waits (bounded) until this process's records are written: the last thing the app does.
pub fn finish() {
    // A timeout or a writer that cannot write changes nothing for a process that is ending.
    let _finished = wasit_diag::shutdown();
}

/// A text from the OS, a framework or the daemon link: its length and fingerprint, never the
/// text (it can name paths, users and hosts).
pub fn foreign(text: &str) -> ErrorInfo {
    ErrorInfo::foreign(Code::st("error"), text)
}

/// Records that operation `op` failed and the shell carried on (`app.failed`).
pub fn failed(op: &'static str, error: &impl std::fmt::Display) {
    emit(AppEvent::Failed {
        op: Code::st(op),
        error: foreign(&error.to_string()),
    });
}

/// The app's link to the daemon changed to `state` (`app.hub_link`).
pub fn hub_link(state: &LinkState) -> AppEvent {
    AppEvent::HubLink {
        state: Code::st(state.code()),
        attempt: match state {
            LinkState::Reconnecting { attempt, .. } => Some(*attempt),
            _ => None,
        },
        error: crate::connection::link_detail(state).1.map(|t| foreign(&t)),
    }
}

/// Reads and records the webview's events (the core of [`diag_log`]). Every valid event of the
/// batch is recorded; a refused one is not, and the batch then answers with an error.
pub fn log_ui(events: Vec<Value>) -> Result<(), BridgeError> {
    if events.len() > MAX_BATCH {
        emit(AppEvent::Failed {
            op: Code::st("diag_log"),
            error: foreign("too many events in one call"),
        });
        return Err(BridgeError::new(
            "invalid_params",
            format!("at most {MAX_BATCH} diagnostics events per call"),
        ));
    }
    let total = events.len();
    let mut refused = 0usize;
    let mut first = None;
    for value in events {
        match UiEvent::from_wire(value) {
            Ok(event) => emit(event.into()),
            Err(why) => {
                refused += 1;
                first.get_or_insert(why);
            }
        }
    }
    let Some(why) = first else { return Ok(()) };
    emit(AppEvent::Failed {
        op: Code::st("diag_log"),
        error: ErrorInfo::foreign(Code::st(why.code()), &why.to_string()),
    });
    Err(BridgeError::new(
        "invalid_params",
        format!("{refused} of {total} diagnostics events refused: {why}"),
    ))
}

/// `diag_log(events)`: the webview's way to write to the app log. Each element is a [`UiEvent`]
/// as JSON (`{"ev": "ui.prompt", …}`); the shell reads them with [`UiEvent::from_wire`] rather
/// than letting the IPC layer do it, so that a refusal can be recorded (the app log then shows
/// that the webview sent something wrong, instead of nothing at all).
#[tauri::command]
pub async fn diag_log(events: Vec<Value>) -> Result<(), BridgeError> {
    log_ui(events)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn the_link_is_recorded_by_state_and_attempt_with_the_text_kept_out() {
        let up = hub_link(&LinkState::Connecting);
        assert!(matches!(
            &up,
            AppEvent::HubLink {
                attempt: None,
                error: None,
                ..
            }
        ));
        let down = hub_link(&LinkState::Reconnecting {
            attempt: 3,
            next_at: SystemTime::now() + Duration::from_secs(5),
            last_error: "cannot reach /home/someone/secret-project".into(),
        });
        let AppEvent::HubLink {
            state,
            attempt,
            error,
        } = &down
        else {
            panic!("{down:?}")
        };
        assert_eq!(state.as_str(), Some("reconnecting"));
        assert_eq!(*attempt, Some(3));
        assert!(error.is_some());
        assert!(!format!("{down:?}").contains("secret-project"), "{down:?}");
    }

    #[test]
    fn the_sink_is_the_documented_one() {
        let c = sink_config(PathBuf::from("d"));
        assert_eq!(c.role.as_str(), Some("app"));
        assert_eq!(
            (c.file.as_str(), c.max_bytes, c.generations),
            ("app.ndjson", 4 << 20, 2)
        );
    }
}
