//! The bootstrap's line protocol, both ends (the hub's connector in `hub::provision`, the
//! host's `marsad-agent boot` in [`crate::boot`] and the scripts in `hub::transport`).
//!
//! Every line is a tag and space-separated `key=value` fields; values are escaped so they never
//! hold a space, `=`, `%` or a line break ([`escape`]); unknown keys are ignored, so either end
//! may add fields without breaking the other. One connection:
//!
//! 1. `MARSAD-SH` (a POSIX host's `sh` asks for its script; the hub sends it as one line), then
//! 2. `MARSAD-BOOT2 os= arch= helper_build= …` ([`Report`]), from the bootstrap script when no
//!    helper of the hub's build is installed, else from that helper;
//! 3. the hub's single answer ([`Decision`]): `RUN`, `RESTART reason=<why>` or
//!    `UPLOAD <bytes> <sha256>` followed by the bytes. After an upload the freshly installed
//!    helper reports again (step 2, `install=ok`) and gets `RUN` or `RESTART`;
//! 4. `MARSAD-OK stop= install= daemon=` once the daemon is ready (its readiness line was read),
//!    or `MARSAD-FAIL stage= reason=`; after `MARSAD-OK` the connection is the bridge's.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// A POSIX host's `sh` is waiting for its script (one line on stdin).
pub const SH_READY: &str = "MARSAD-SH";
/// The report tag.
pub const REPORT: &str = "MARSAD-BOOT2";
/// Success, after the decision was carried out and the daemon is ready.
pub const OK: &str = "MARSAD-OK";
/// Failure, with its stage and reason.
pub const FAIL: &str = "MARSAD-FAIL";

/// `value` with `%`, space, `=`, CR and LF percent-encoded.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '=' => out.push_str("%3D"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            c => out.push(c),
        }
    }
    out
}

/// The inverse of [`escape`] (any `%XX` is decoded; a malformed one is kept as is).
pub fn unescape(value: &str) -> String {
    let b = value.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A line's tag and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub tag: String,
    /// Words without `=` (the `UPLOAD` arguments), in order.
    pub words: Vec<String>,
    fields: Vec<(String, String)>,
}

impl Line {
    pub fn parse(text: &str) -> Line {
        let mut parts = text.split_whitespace();
        let tag = parts.next().unwrap_or("").to_owned();
        let mut words = Vec::new();
        let mut fields = Vec::new();
        for p in parts {
            match p.split_once('=') {
                Some((k, v)) => fields.push((k.to_owned(), unescape(v))),
                None => words.push(p.to_owned()),
            }
        }
        Line { tag, words, fields }
    }

    /// The value of `key` (`none` and the empty string are no value).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty() && *v != "none")
    }
}

/// `tag k=v …` with the values escaped (`None` is written `none`).
pub fn format(tag: &str, fields: &[(&str, Option<&str>)]) -> String {
    let mut s = tag.to_owned();
    for (k, v) in fields {
        s.push(' ');
        s.push_str(k);
        s.push('=');
        s.push_str(&v.map_or_else(|| "none".to_owned(), escape));
    }
    s
}

/// The daemon of the host's account, as the helper found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    /// It answered.
    Running,
    /// Nothing serves its endpoint.
    Absent,
    /// Something is there but did not answer as a daemon.
    Unreachable,
    /// Not looked at (the bootstrap script reports before any helper runs).
    Unknown,
}

impl DaemonState {
    pub fn word(self) -> &'static str {
        match self {
            DaemonState::Running => "running",
            DaemonState::Absent => "absent",
            DaemonState::Unreachable => "unreachable",
            DaemonState::Unknown => "unknown",
        }
    }

    fn parse(w: Option<&str>) -> DaemonState {
        match w {
            Some("running") => DaemonState::Running,
            Some("absent") => DaemonState::Absent,
            Some("unreachable") => DaemonState::Unreachable,
            _ => DaemonState::Unknown,
        }
    }
}

/// `MARSAD-BOOT2`: what the host has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// `uname -s` / `Windows`.
    pub os: String,
    /// `uname -m` / `PROCESSOR_ARCHITECTURE`.
    pub arch: String,
    /// The build of the helper reporting (`None`: the script reports, none is installed).
    pub helper_build: Option<String>,
    /// SHA-256 (hex) of the reporting helper's executable.
    pub helper_hash: Option<String>,
    /// The checksum tool the script would verify an upload with (`None`: there is none, and
    /// nothing can be installed). Only reported by the scripts.
    pub hasher: Option<String>,
    pub daemon: DaemonState,
    pub daemon_build: Option<String>,
    pub daemon_host: Option<String>,
    pub daemon_pid: Option<u32>,
    /// The hub the running daemon serves (`None`: it does not say: an earlier build).
    pub daemon_hub: Option<String>,
    /// The host's machine key.
    pub machine: Option<String>,
    /// This helper was installed by this connection's upload.
    pub installed: bool,
}

impl Report {
    pub fn parse(line: &Line) -> Report {
        let s = |k: &str| line.get(k).map(str::to_owned);
        Report {
            os: s("os").unwrap_or_default(),
            arch: s("arch").unwrap_or_default(),
            helper_build: s("helper_build"),
            helper_hash: s("helper_hash"),
            hasher: s("hasher"),
            daemon: DaemonState::parse(line.get("daemon")),
            daemon_build: s("daemon_build"),
            daemon_host: s("daemon_host"),
            daemon_pid: line.get("daemon_pid").and_then(|p| p.parse().ok()),
            daemon_hub: s("daemon_hub"),
            machine: s("machine"),
            installed: line.get("install") == Some("ok"),
        }
    }

    pub fn line(&self) -> String {
        let pid = self.daemon_pid.map(|p| p.to_string());
        format(
            REPORT,
            &[
                ("os", Some(&self.os)),
                ("arch", Some(&self.arch)),
                ("helper_build", self.helper_build.as_deref()),
                ("helper_hash", self.helper_hash.as_deref()),
                ("daemon", Some(self.daemon.word())),
                ("daemon_build", self.daemon_build.as_deref()),
                ("daemon_host", self.daemon_host.as_deref()),
                ("daemon_pid", pid.as_deref()),
                ("daemon_hub", self.daemon_hub.as_deref()),
                ("machine", self.machine.as_deref()),
                ("install", Some(if self.installed { "ok" } else { "none" })),
            ],
        )
    }
}

/// Why the hub has the running daemon restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartWhy {
    /// It is another build.
    Build,
    /// It knows itself by another host id.
    Host,
    /// Something holds its endpoint but does not answer as a daemon.
    Unreachable,
}

impl RestartWhy {
    pub fn word(self) -> &'static str {
        match self {
            RestartWhy::Build => "build",
            RestartWhy::Host => "host",
            RestartWhy::Unreachable => "unreachable",
        }
    }
}

/// The hub's one answer to a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Run,
    Restart(RestartWhy),
    Upload { bytes: u64, sha256: String },
}

impl Decision {
    pub fn line(&self) -> String {
        match self {
            Decision::Run => "RUN".to_owned(),
            Decision::Restart(why) => format!("RESTART reason={}", why.word()),
            Decision::Upload { bytes, sha256 } => format!("UPLOAD {bytes} {sha256}"),
        }
    }

    /// The decision a helper received (`None`: not one).
    pub fn parse(text: &str) -> Option<Decision> {
        let l = Line::parse(text);
        match l.tag.as_str() {
            "RUN" => Some(Decision::Run),
            "RESTART" => Some(Decision::Restart(match l.get("reason") {
                Some("host") => RestartWhy::Host,
                Some("unreachable") => RestartWhy::Unreachable,
                _ => RestartWhy::Build,
            })),
            "UPLOAD" => Some(Decision::Upload {
                bytes: l.words.first()?.parse().ok()?,
                sha256: l.words.get(1)?.clone(),
            }),
            _ => None,
        }
    }
}

/// `MARSAD-FAIL stage=<stage> reason=<reason>`.
pub fn fail_line(stage: &str, reason: &str) -> String {
    format(FAIL, &[("stage", Some(stage)), ("reason", Some(reason))])
}

/// SHA-256 of a file, lowercase hex (blocking).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_whatever_they_hold() {
        for v in ["plain", "a b=c%d\r\n", "0.2.0+abc.dirty", "%zz", "%4", ""] {
            assert_eq!(unescape(&escape(v)), v, "{v:?}");
            assert!(!escape(v).contains([' ', '=', '\n']));
        }
    }

    #[test]
    fn a_report_round_trips_and_ignores_unknown_keys() {
        let r = Report {
            os: "Linux".into(),
            arch: "x86_64".into(),
            helper_build: Some("0.1 +x".into()),
            helper_hash: Some("ab".into()),
            hasher: None,
            daemon: DaemonState::Running,
            daemon_build: Some("old".into()),
            daemon_host: Some("box".into()),
            daemon_pid: Some(42),
            daemon_hub: None,
            machine: Some("m1".into()),
            installed: true,
        };
        let text = format!("{} future=thing", r.line());
        assert_eq!(Report::parse(&Line::parse(&text)), r);
        // The script's own report.
        let s = Report::parse(&Line::parse(
            "MARSAD-BOOT2 os=Darwin arch=arm64 helper_build=none daemon=unknown hasher=shasum",
        ));
        assert_eq!(s.helper_build, None);
        assert_eq!(s.hasher.as_deref(), Some("shasum"));
        assert_eq!(s.daemon, DaemonState::Unknown);
        assert!(!s.installed);
    }

    #[test]
    fn decisions_round_trip() {
        for d in [
            Decision::Run,
            Decision::Restart(RestartWhy::Build),
            Decision::Restart(RestartWhy::Host),
            Decision::Restart(RestartWhy::Unreachable),
            Decision::Upload {
                bytes: 12,
                sha256: "ff".into(),
            },
        ] {
            assert_eq!(Decision::parse(&d.line()), Some(d));
        }
        assert_eq!(Decision::parse("UPLOAD 12"), None);
        assert_eq!(Decision::parse("QUIT"), None);
    }
}
