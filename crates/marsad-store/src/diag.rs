//! The typed, content-free diagnostics of the store: a document that could not be made durable,
//! and the secret values' system credential store (see [`crate::secrets`]) falling back to the
//! store's file or handing its values over, in place of free-form log lines.
//!
//! Every field is one of `wasit-diag`'s value types: a fixed backend word is a [`Code`], an
//! error is either an I/O error ([`ErrorInfo::io`]) or a text from outside this crate
//! ([`ErrorInfo::foreign`]: code, length and fingerprint, never the text). No event carries a
//! secret value, a secret's name or key, an account or service string, a document's content or
//! a key a user typed.

use std::fmt::Display;
use std::io;

use wasit_diag::{Code, ErrorInfo};

wasit_diag::diag_events! {
    /// Events of the store's documents and secret values.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub enum StoreEvent {
        /// A document's directory could not be synced after its rename. The write stands; only
        /// its durability across a power loss is weaker.
        #[diag(ev = "store.dir_fsync_failed", lvl = debug)]
        DirFsyncFailed { error: ErrorInfo },
        /// A document write failed; the store keeps its previous state.
        #[diag(ev = "store.write_failed", lvl = error)]
        WriteFailed { error: ErrorInfo },
        /// The system credential store cannot be used (no secret service runs on the session
        /// bus): the secret values stay in the store's file. `store` is the fixed name of the
        /// system store.
        #[diag(ev = "secrets.service_unavailable", lvl = warn)]
        SecretsServiceUnavailable { store: Code, error: ErrorInfo },
        /// The system credential store did not pass its probe at open: the secret values stay
        /// in the store's file.
        #[diag(ev = "secrets.probe_failed", lvl = warn)]
        SecretsProbeFailed { store: Code, error: ErrorInfo },
        /// Secret values of the store's file were moved into the system credential store, and
        /// the file was removed.
        #[diag(ev = "secrets.moved_from_file", lvl = info)]
        SecretsMovedFromFile { store: Code },
    }
}

/// Records `event` in this process's sink (never waits).
fn record(event: StoreEvent) {
    wasit_diag::emit(&event);
}

/// An error whose text came from outside this crate (the credential store, a document): its
/// code, length and fingerprint only.
fn foreign(code: &'static str, e: &impl Display) -> ErrorInfo {
    ErrorInfo::foreign(Code::st(code), &e.to_string())
}

/// A system store's name as a fixed word (`Secret Service` → `secret_service`).
fn store_code(name: &str) -> Code {
    Code::new(&name.to_ascii_lowercase().replace(' ', "_"))
}

/// The durability sync of a document's directory failed.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn dir_fsync_failed(e: &io::Error) {
    record(StoreEvent::DirFsyncFailed {
        error: ErrorInfo::io(e),
    });
}

/// A document write failed.
pub fn write_failed(e: &impl Display) {
    record(StoreEvent::WriteFailed {
        error: foreign("store_write", e),
    });
}

/// The system credential store `name` cannot be used: its service is not running.
pub fn secrets_service_unavailable(name: &str, e: &impl Display) {
    record(StoreEvent::SecretsServiceUnavailable {
        store: store_code(name),
        error: foreign("keyring", e),
    });
}

/// The system credential store `name` failed its probe.
pub fn secrets_probe_failed(name: &str, e: &impl Display) {
    record(StoreEvent::SecretsProbeFailed {
        store: store_code(name),
        error: foreign("keyring", e),
    });
}

/// The values of `secrets.json` were moved into the system credential store `name`.
pub fn secrets_moved_from_file(name: &str) {
    record(StoreEvent::SecretsMovedFromFile {
        store: store_code(name),
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use wasit_diag::{Ident, Sink, SinkConfig};

    use super::*;

    const HEADER: [&str; 7] = ["b", "seq", "t", "m", "src", "lvl", "ev"];

    /// Every variant, once, with its expected `ev`, `lvl` and declared fields. Adding a variant
    /// means adding its case here. Every error text carries `SECRETVALUE`, which must never
    /// reach a record.
    fn cases() -> Vec<(
        StoreEvent,
        &'static str,
        &'static str,
        &'static [&'static str],
    )> {
        let io_err = || io::Error::from(io::ErrorKind::PermissionDenied);
        let secret_text = || foreign("keyring", &"SECRETVALUE in a credential store error");
        vec![
            (
                StoreEvent::DirFsyncFailed {
                    error: ErrorInfo::io(&io_err()),
                },
                "store.dir_fsync_failed",
                "debug",
                &["error"],
            ),
            (
                StoreEvent::WriteFailed {
                    error: foreign("store_write", &"SECRETVALUE in a document"),
                },
                "store.write_failed",
                "error",
                &["error"],
            ),
            (
                StoreEvent::SecretsServiceUnavailable {
                    store: Code::st("secret_service"),
                    error: secret_text(),
                },
                "secrets.service_unavailable",
                "warn",
                &["store", "error"],
            ),
            (
                StoreEvent::SecretsProbeFailed {
                    store: Code::st("windows_credential_manager"),
                    error: secret_text(),
                },
                "secrets.probe_failed",
                "warn",
                &["store", "error"],
            ),
            (
                StoreEvent::SecretsMovedFromFile {
                    store: Code::st("macos_keychain"),
                },
                "secrets.moved_from_file",
                "info",
                &["store"],
            ),
        ]
    }

    #[test]
    fn every_event_writes_the_header_and_its_declared_fields_only() {
        let cases = cases();
        let dir = std::env::temp_dir().join(format!("marsad-store-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = "events.ndjson";
        let _ = std::fs::remove_file(dir.join(file));
        let cfg = SinkConfig::new(Code::st("daemon"), Ident::st("b1"), dir.clone(), file);
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
            assert!(!rec.to_string().contains("SECRETVALUE"), "{rec}");
        }
    }
}
