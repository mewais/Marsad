//! The typed, content-free diagnostics of the hub's fleet aggregator (`src` `hub`): a host's
//! session feed that outlived its host, and the aggregator's task that has ended, in place of
//! free-form log lines.
//!
//! Fields are `wasit-diag`'s value types: a host id is an [`Ident`]. No event carries session
//! content.

use std::fmt::Display;

use wasit_diag::Ident;

wasit_diag::diag_events! {
    /// Events of the hub's fleet aggregator.
    pub enum FleetDiagEvent {
        /// A host's session feed delivered an event after the host was removed from the fleet;
        /// the event is dropped.
        #[diag(ev = "fleet.feed_unknown_host", lvl = debug)]
        FeedUnknownHost { host: Ident },
        /// The fleet aggregator's task has ended: fleet updates are no longer applied.
        #[diag(ev = "fleet.aggregator_gone", lvl = error)]
        AggregatorGone {},
    }
}

/// Records `event` in this process's sink (never waits).
fn record(event: FleetDiagEvent) {
    wasit_diag::emit(&event);
}

/// An id as a record field (validated: anything that is not an identifier shows as its length).
fn ident(id: &impl Display) -> Ident {
    Ident::new(&id.to_string())
}

/// A session feed of `host` delivered an event after the host was removed.
pub fn feed_unknown_host(host: &impl Display) {
    record(FleetDiagEvent::FeedUnknownHost { host: ident(host) });
}

/// The fleet aggregator's task has ended.
pub fn aggregator_gone() {
    record(FleetDiagEvent::AggregatorGone {});
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
        FleetDiagEvent,
        &'static str,
        &'static str,
        &'static [&'static str],
    )> {
        vec![
            (
                FleetDiagEvent::FeedUnknownHost {
                    host: Ident::st("host-1"),
                },
                "fleet.feed_unknown_host",
                "debug",
                &["host"],
            ),
            (
                FleetDiagEvent::AggregatorGone {},
                "fleet.aggregator_gone",
                "error",
                &[],
            ),
        ]
    }

    #[test]
    fn every_event_writes_the_header_and_its_declared_fields_only() {
        let cases = cases();
        let dir = std::env::temp_dir().join(format!("marsad-fleet-events-{}", std::process::id()));
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
        }
    }
}
