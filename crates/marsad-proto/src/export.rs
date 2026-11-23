//! `diagnostics.export.start` and the topic `diagnostics.exports` (hub), and the lines of the
//! export file itself (design §6.5–§6.7).
//!
//! # The wire
//!
//! [`methods::DIAGNOSTICS_EXPORT_START`](crate::methods::DIAGNOSTICS_EXPORT_START) takes an
//! [`ExportRequest`] and answers at once with an [`ExportStarted`]; the work is a task of the
//! hub, never part of the request. Its progress arrives on
//! [`topics::DIAGNOSTICS_EXPORTS`](crate::topics::DIAGNOSTICS_EXPORTS) as [`ExportsEvent`]s: the
//! status of the exports the hub knows (a running one and the last finished ones), `Reset` first
//! and an `Upsert` for every change after it. One export runs at a time: a second
//! `diagnostics.export.start` while one runs is refused with the `conflict` error naming the
//! running export.
//!
//! # The file
//!
//! `<data>/diag/exports/marsad-diagnostics-<hub host id>-<id>.ndjson.gz`: gzip (a sequence of
//! members, so a cut-off file is readable as far as it goes) of NDJSON. Every line the exporter
//! writes itself is an [`ExportLine`] (`{"line": "<kind>", …}`); between a `section` or `trail`
//! line and the next [`ExportLine`] the lines are the source's own records, verbatim (log
//! records of `wasit-diag`, or `{cursor, t, ev, …}` trail entries). In order:
//!
//! 1. `manifest`: what the export is and what it is about: format, window, the hub, and for
//!    every host its [`HostFacts`] (link state and last error, clock offset, build, platform,
//!    environment, agents);
//! 2. per source, in priority order (newest generations of every source first, then the trails
//!    and shim logs of the sessions active in the window, then older generations): `section`
//!    (one generation of one log) or `trail` (one session's trail), each followed by its lines;
//!    `source_unavailable` for a host that could not be read; `skipped` for a section left out
//!    (the size cap or a host's budget) with why;
//! 3. `end`: counts, whether everything planned is in, and the result of every source.
//!
//! Every field of every line is a `wasit-diag` value type (declared with `diag_data!`), so the
//! lines the exporter writes cannot hold a prompt, a response, a tool's input or output, a
//! file's contents or a secret; the records that follow a `section` are content-free because
//! their writers are.

use serde::{Deserialize, Serialize};
use wasit_control::HostId;
use wasit_diag::{Bytes, Code, DiagText, ErrorInfo, Ident, Ms, TimeMs};

/// The export file's format version (the `format` of its manifest).
pub const EXPORT_FORMAT: u32 = 1;
/// The version of the redaction policy the records were written under (design §4.4).
pub const EXPORT_REDACTION: u32 = 1;
/// The export window when a request names none: 24 hours.
pub const DEFAULT_WINDOW_MS: u64 = 24 * 3600 * 1000;
/// The longest window a request may name: 31 days.
pub const MAX_WINDOW_MS: u64 = 31 * 24 * 3600 * 1000;
/// Exports are files of this directory of `<data>/diag/`.
pub const EXPORT_DIR: &str = "exports";
/// A file name starts with this …
pub const EXPORT_FILE_PREFIX: &str = "marsad-diagnostics-";
/// … and ends with this.
pub const EXPORT_FILE_SUFFIX: &str = ".ndjson.gz";
/// How many finished exports are kept (the newest).
pub const EXPORTS_KEPT: usize = 3;

fn yes() -> bool {
    true
}

/// `diagnostics.export.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportRequest {
    /// The start of the window the export covers, as hub wall-clock milliseconds since the Unix
    /// epoch (the end is when the export starts). Absent: 24 hours before now. Not in the
    /// future and not more than 31 days back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    /// Include the trails of the sessions active in the window (default: yes).
    #[serde(default = "yes")]
    pub include_trails: bool,
}

impl Default for ExportRequest {
    fn default() -> Self {
        ExportRequest {
            since_ms: None,
            include_trails: true,
        }
    }
}

/// Result of `diagnostics.export.start`: the export began; follow it on `diagnostics.exports`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportStarted {
    pub id: String,
}

/// Where an export is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    Running,
    /// The file is written (`file`); `complete` says whether everything planned is in it.
    Done,
    /// No file was produced (`error` says why).
    Failed,
}

/// Where one source of an export is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExportSourceState {
    Waiting,
    /// Being read; `bytes` is what has been read so far.
    Reading,
    /// The host's link is down: nothing could be read (`reason` is the link state).
    NotConnected,
    /// Everything planned is in the file.
    Done,
    /// Some of it is: `reason` says what is missing and why.
    Partial,
    /// Nothing of it is (`reason` says why).
    Failed,
}

/// One source of an export: this app (`app`), or a host (`host:<id>`; the hub's own computer is
/// the host `local` of the fleet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportSource {
    pub source: String,
    /// The host, for a host source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostId>,
    pub state: ExportSourceState,
    /// Raw bytes read so far (logs and trails).
    pub bytes: u64,
    /// Why the source is `not_connected`, `partial` or `failed`, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One export as the topic reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportStatus {
    pub id: String,
    pub state: ExportState,
    /// The window: its start (ms since the Unix epoch).
    pub since_ms: u64,
    /// When the export began (ms since the Unix epoch).
    pub started_ms: u64,
    pub sources: Vec<ExportSource>,
    /// The file's name, in `<data>/diag/exports/` of the hub's machine (a plain name: no
    /// directory part), once `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The file's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// `done` only: nothing planned is missing (no source partial, unavailable or failed, no
    /// section skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
    /// `failed` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Payload of the topic `diagnostics.exports`. Apply in order; `reset` replaces everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportsEvent {
    /// Every export the hub knows (first event of a fresh subscription or after an expired
    /// cursor).
    Reset { exports: Vec<ExportStatus> },
    /// One export was started or changed.
    Upsert { export: ExportStatus },
}

// ── the lines of the file ────────────────────────────────────────────────────

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// What one discovery probe of an agent found.
    pub enum ProbeFacts {
        #[diag(ev = "probe")]
        Probe { kind: Code, ok: bool, ms: Ms ; error: ErrorInfo },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// One agent CLI on a host: whether it is installed and which version answered. What the CLI
    /// wrote when it failed is a length and a fingerprint, never the text.
    pub enum DriverFacts {
        #[diag(ev = "driver")]
        Driver {
            driver: Ident,
            installed: bool,
            /// The host sets an agent command of its own for this driver.
            command_set: bool,
            /// Probes were still running when it was asked.
            refreshing: bool,
            probes: Vec<ProbeFacts>
            ; version: Ident,
            warning: ErrorInfo,
            error: ErrorInfo,
            refreshed_at: TimeMs
        },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// How a host's setup commands went (never what they printed).
    pub enum SetupFacts {
        #[diag(ev = "setup")]
        Setup { ok: bool, applied: bool, ms: Ms ; exit_code: i32, error: DiagText },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// How a host's environment was captured (design §6.5): where it came from, how long it
    /// took, why attempts failed (first lines), `PATH` as the processes of the host get it. The
    /// values of other variables are never read.
    pub enum EnvFacts {
        #[diag(ev = "env")]
        Env {
            /// `login_interactive_shell`, `login_shell`, `registry` or `process`.
            source: Code,
            ms: Ms,
            captured_at: TimeMs,
            notes: Vec<DiagText>,
            path: Vec<DiagText>,
            /// How many variables the host's configuration sets (their names are not listed).
            variables: u32
            ; shell: Ident,
            revision: Ident,
            setup: SetupFacts
        },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// A question to a host that failed.
    pub enum FactFailure {
        /// `call` is the method asked (`host.info`, `diag.sources`, …).
        #[diag(ev = "failure")]
        Failure { call: Code, error: ErrorInfo },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// A host as the export saw it (design §6.6). `role` is `hub` for the host that is the hub's
    /// own computer, else `host`. `link` is the link state's word (`local` for the hub's own
    /// computer). The ssh target, a distro's name and any path of a user are never here.
    pub enum HostFacts {
        #[diag(ev = "host")]
        Host {
            id: Ident,
            /// `local`, `ssh` or `wsl`.
            kind: Code,
            role: Code,
            link: Code,
            drivers: Vec<DriverFacts>,
            /// Questions that failed (the rest of the facts are what could be had).
            failures: Vec<FactFailure>
            ; machine: Ident,
            os: Ident,
            arch: Ident,
            target: Ident,
            build: Ident,
            started_at: TimeMs,
            pid: u32,
            /// The link's last error (the transport's words, scrubbed and bounded).
            last_error: DiagText,
            /// The host's clock minus the hub's (ms), from one `diag.sources` round trip:
            /// `remote_now - (sent + received) / 2`. Times in the logs are never rewritten.
            clock_offset_ms: i64,
            rtt_ms: Ms,
            /// Sessions the host has, and how many were active in the window.
            sessions: u32,
            sessions_in_window: u32,
            env: EnvFacts
        },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "fact")]
    /// What came of one source (the summary in the `end` line).
    pub enum SourceResult {
        /// `outcome`: `done`, `partial`, `not_connected` or `failed` (the words of
        /// [`ExportSourceState`]).
        #[diag(ev = "result")]
        Result {
            source: Ident,
            outcome: Code,
            /// Raw bytes of records in the file.
            bytes: Bytes,
            sections: u32,
            skipped: u32
            ; reason: Code
        },
    }
}

wasit_diag::diag_data! {
    #[diag(tag = "line")]
    /// A line the exporter writes itself.
    pub enum ExportLine {
        /// The first line. `mode`: `hub` (everything) or `local_files` (no hub was serving: only
        /// this machine's files, no trails, no host facts; `note` says why). The window is
        /// `since`..`until`; `cap_bytes` is the cap of the compressed file and
        /// `host_budget_bytes` how much raw data one host contributes.
        #[diag(ev = "manifest")]
        Manifest {
            format: u32,
            redaction: u32,
            id: Ident,
            created: TimeMs,
            since: TimeMs,
            until: TimeMs,
            mode: Code,
            include_trails: bool,
            cap_bytes: Bytes,
            host_budget_bytes: Bytes,
            hub_host: Ident,
            hub_build: Ident,
            hub_target: Ident,
            hub_os: Ident,
            hub_arch: Ident,
            hosts: Vec<HostFacts>
            ; hub_machine: Ident,
            hub_pid: u32,
            note: Code
        },
        /// One generation of one log follows (`source`: `app`, `host:<id>` or `local`; `file`:
        /// `app`, `daemon`, `aux` or `shim:<session>`). `bytes`, `lines`: what follows.
        /// `complete`: the whole generation is in; else `reason` says why not (`host_budget`,
        /// `host_deadline`, `call_deadline`, `rotated_away`, `read_failed` with `error`).
        /// `dropped`: lines that were not a JSON object and were left out.
        #[diag(ev = "section")]
        Section {
            source: Ident,
            file: Ident,
            gen: u64,
            bytes: Bytes,
            lines: u64,
            complete: bool
            ; reason: Code,
            error: ErrorInfo,
            dropped: u64
        },
        /// One session's trail follows, oldest entry first (`entries`).
        #[diag(ev = "trail")]
        Trail {
            source: Ident,
            session: Ident,
            bytes: Bytes,
            entries: u64,
            complete: bool
            ; reason: Code,
            error: ErrorInfo
        },
        /// A host that could not be read. `link_state` is the link's word; `last_error` the
        /// link's last error; `error` the failure of a call to a host that was connected.
        #[diag(ev = "source_unavailable")]
        SourceUnavailable { source: Ident, link_state: Code ; last_error: DiagText, error: ErrorInfo },
        /// A section left out: `file` is the section's name (`daemon`, `aux`, `app`,
        /// `shim:<session>`, `trail:<session>`), `why`: `size_cap` (it did not fit the cap, or a
        /// section before it did not and everything after that is left out), `host_budget` (the
        /// host's raw budget was spent), `host_deadline` (its time was) or `read_failed` (the
        /// source could not be read at all). `bytes`: its raw size when known.
        #[diag(ev = "skipped")]
        Skipped { source: Ident, file: Ident, why: Code ; gen: u64, bytes: Bytes },
        /// The last line. `complete`: nothing planned is missing. `bytes`: raw bytes of records
        /// in the file. `skipped`: sections left out; `partial`: sections cut short.
        #[diag(ev = "end")]
        End {
            sections: u32,
            skipped: u32,
            partial: u32,
            complete: bool,
            bytes: Bytes,
            ms: Ms,
            sources: Vec<SourceResult>
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_defaults_to_the_last_day_with_trails() {
        let r: ExportRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(r, ExportRequest::default());
        assert!(r.include_trails);
        let r: ExportRequest =
            serde_json::from_str(r#"{"since_ms":5,"include_trails":false}"#).unwrap();
        assert_eq!((r.since_ms, r.include_trails), (Some(5), false));
    }

    #[test]
    fn a_line_is_tagged_and_leaves_absent_fields_out() {
        let l = ExportLine::Skipped {
            source: Ident::new("host:gpu"),
            file: Ident::new("daemon"),
            why: Code::st("size_cap"),
            gen: Some(2),
            bytes: None,
        };
        assert_eq!(
            serde_json::to_string(&l).unwrap(),
            r#"{"line":"skipped","source":"host:gpu","file":"daemon","why":"size_cap","gen":2}"#
        );
        let back: ExportLine = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn a_manifest_names_its_hosts() {
        let host = HostFacts::Host {
            id: Ident::new("gpu"),
            kind: Code::st("ssh"),
            role: Code::st("host"),
            link: Code::st("reconnecting"),
            drivers: vec![],
            failures: vec![],
            machine: None,
            os: None,
            arch: None,
            target: None,
            build: None,
            started_at: None,
            pid: None,
            last_error: Some(
                serde_json::from_str::<DiagText>("\"ssh: Connection refused\"").unwrap(),
            ),
            clock_offset_ms: None,
            rtt_ms: None,
            sessions: None,
            sessions_in_window: None,
            env: None,
        };
        let v = serde_json::to_value(&host).unwrap();
        assert_eq!(v["fact"], "host");
        assert_eq!(v["link"], "reconnecting");
        assert_eq!(v["last_error"], "ssh: Connection refused");
    }
}
