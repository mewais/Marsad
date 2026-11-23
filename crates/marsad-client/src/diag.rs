//! The typed, content-free diagnostics of the client of the local daemon: the runtime directory
//! it cannot prepare and the daemon it starts, in place of free-form log lines.
//!
//! Fields are `wasit-diag`'s value types: the daemon's build id is an [`Ident`], and an I/O
//! error is recorded by its kind and OS error number ([`ErrorInfo::io`]), never its message.

use std::io;

use wasit_diag::{ErrorInfo, Ident};

wasit_diag::diag_events! {
    /// Events of the client of the local Marsad daemon.
    pub enum ClientEvent {
        /// The runtime directory could not be prepared; the socket path named is where the daemon
        /// would listen, and the daemon refuses to start in the same state.
        #[diag(ev = "client.runtime_dir_failed", lvl = error)]
        RuntimeDirFailed { error: ErrorInfo },
        /// The client started the daemon, which reported itself ready.
        #[diag(ev = "client.daemon_started", lvl = info)]
        DaemonStarted { pid: u32, build: Ident },
    }
}

/// Records `event` in this process's sink (never waits).
fn record(event: ClientEvent) {
    wasit_diag::emit(&event);
}

/// The runtime directory could not be prepared.
pub fn runtime_dir_failed(e: &io::Error) {
    record(ClientEvent::RuntimeDirFailed {
        error: ErrorInfo::io(e),
    });
}

/// The daemon with process id `pid` and build `build` reported itself ready.
pub fn daemon_started(pid: u32, build: &str) {
    record(ClientEvent::DaemonStarted {
        pid,
        build: Ident::new(build),
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use wasit_diag::{Code, Sink, SinkConfig};

    use super::*;

    const HEADER: [&str; 7] = ["b", "seq", "t", "m", "src", "lvl", "ev"];

    /// Every variant, once, with its expected `ev`, `lvl` and declared fields. Adding a variant
    /// means adding its case here.
    fn cases() -> Vec<(
        ClientEvent,
        &'static str,
        &'static str,
        &'static [&'static str],
    )> {
        vec![
            (
                ClientEvent::RuntimeDirFailed {
                    error: ErrorInfo::io(&io::Error::from(io::ErrorKind::PermissionDenied)),
                },
                "client.runtime_dir_failed",
                "error",
                &["error"],
            ),
            (
                ClientEvent::DaemonStarted {
                    pid: 4242,
                    build: Ident::st("build-1.2.3"),
                },
                "client.daemon_started",
                "info",
                &["pid", "build"],
            ),
        ]
    }

    #[test]
    fn every_event_writes_the_header_and_its_declared_fields_only() {
        let cases = cases();
        let dir = std::env::temp_dir().join(format!("marsad-client-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = "events.ndjson";
        let _ = std::fs::remove_file(dir.join(file));
        let cfg = SinkConfig::new(Code::st("client"), Ident::st("b1"), dir.clone(), file);
        let sink = Sink::start(cfg).unwrap();
        for (e, _, _, _) in &cases {
            sink.emit(e);
        }
        assert!(sink.shutdown(Duration::from_secs(30)));
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines.len(),
            cases.len() + 1,
            "header and one record each: {text}"
        );
        for (rec, (_, ev, lvl, fields)) in lines[1..].iter().zip(&cases) {
            let keys: BTreeSet<&str> = rec
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let want: BTreeSet<&str> = HEADER.iter().chain(fields.iter()).copied().collect();
            assert_eq!(keys, want, "{rec}");
            assert_eq!(
                (rec["ev"].as_str(), rec["lvl"].as_str()),
                (Some(*ev), Some(*lvl)),
                "{rec}"
            );
        }
    }
}
