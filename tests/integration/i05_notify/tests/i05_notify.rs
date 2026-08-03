//! Integration scenario i05_notify — B02 events × B10 notify cross-block test.
//!
//! Proves: real folded stream state → NotifEvent → build_notification → CaptureNotifier,
//! plus DND suppression, per-kind toggle suppression, sound gating, and burst grouping.

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_notify::{
    build_notification, group, Action, CaptureNotifier, NotifEvent, NotifKind, NotifPrefs,
    Notification, Notifier, Priority,
};
use wasit_claude::LineParser;

// ── helpers ────────────────────────────────────────────────────────────────────

/// The qa.ndjson fixture path, resolved at compile time relative to the workspace root.
const QA_NDJSON: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/streams/qa.ndjson"
);

/// Fold every line in the given NDJSON text into a SessionState via `wasit_claude::LineParser`.
///
/// Wave 1 moved raw stream-json parsing into the wasit drivers; `marsad-events`
/// now folds `AgentEvent`s. `LineParser::on_line` is the per-line equivalent of
/// the deleted `parse_stream_line`.
fn fold_ndjson(ndjson: &str) -> SessionState {
    let mut state = SessionState::initial();
    let mut parser = LineParser::new();
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        for event in parser.on_line(line) {
            state = reduce(state, Event::Stream(event));
        }
    }
    state
}

/// Build a minimal NotifEvent for a given kind.
fn notif_event(kind: NotifKind, session_id: &str) -> NotifEvent {
    let (title, body) = match kind {
        NotifKind::Needs => (
            "Session needs approval".to_string(),
            "A tool-use gate is waiting".to_string(),
        ),
        NotifKind::Error => (
            "Session errored".to_string(),
            "The session hit an error".to_string(),
        ),
        NotifKind::Done => (
            "Session finished".to_string(),
            "Session completed successfully".to_string(),
        ),
        NotifKind::Started => (
            "Session started".to_string(),
            "A new session is starting".to_string(),
        ),
    };
    NotifEvent {
        kind,
        session_id: session_id.to_string(),
        host: "localhost".to_string(),
        title,
        body,
    }
}

// ── A. Real stream fold → Done phase → NotifEvent ─────────────────────────────

/// Fold qa.ndjson and verify it reaches Phase::Done, then build a Done notification.
#[test]
fn fold_qa_stream_to_done_and_notify() {
    let ndjson =
        std::fs::read_to_string(QA_NDJSON).unwrap_or_else(|e| panic!("cannot read qa.ndjson: {e}"));

    let state = fold_ndjson(&ndjson);

    // The fixture is init → assistant → result/success: must reach Done.
    assert_eq!(
        state.phase,
        Phase::Done,
        "qa.ndjson must fold to Phase::Done"
    );
    assert!(
        state.session_id.is_some(),
        "session_id must be set after init"
    );
    assert!(state.tokens > 0, "token count must accumulate");
    assert!(state.cost_usd > 0.0, "cost must accumulate");

    let sid = state.session_id.as_deref().unwrap_or("unknown");

    // Cross-block: turn the folded Done phase into a NotifEvent.
    let ev = NotifEvent {
        kind: NotifKind::Done,
        session_id: sid.to_string(),
        host: "localhost".to_string(),
        title: "Session finished".to_string(),
        body: format!("Cost ${:.6}", state.cost_usd),
    };

    let prefs = NotifPrefs::default(); // done=true, dnd=false
    let note = build_notification(&ev, &prefs)
        .expect("Done notification must be built with default prefs");

    assert_eq!(note.kind, NotifKind::Done);
    assert_eq!(note.session_id, sid);
    assert_eq!(note.priority, Priority::Low);
    assert!(!note.play_sound, "Done is Low priority; no sound");
    // Actions: Open (not primary) + Dismiss (not primary).
    assert_eq!(note.actions.len(), 2);
    assert_eq!(
        note.actions[0],
        Action {
            label: "Open".to_string(),
            primary: false
        }
    );
    assert_eq!(
        note.actions[1],
        Action {
            label: "Dismiss".to_string(),
            primary: false
        }
    );

    // Deliver via CaptureNotifier.
    let mut cap = CaptureNotifier::new();
    cap.deliver(&note).expect("CaptureNotifier must not fail");
    assert_eq!(cap.delivered().len(), 1);
    assert_eq!(cap.delivered()[0], note);
}

// ── B. build_notification: all four kinds with default prefs ──────────────────

#[test]
fn build_notification_all_kinds_default_prefs() {
    let prefs = NotifPrefs::default();
    // Default: needs/error/done=true, started=false, sound=true, group=true, dnd=false.

    // Needs → High + [Approve(primary), Reject, Open] + sound=true.
    {
        let ev = notif_event(NotifKind::Needs, "sess-needs");
        let note = build_notification(&ev, &prefs).expect("Needs must build");
        assert_eq!(note.priority, Priority::High);
        assert!(note.play_sound);
        assert_eq!(note.actions.len(), 3);
        assert_eq!(
            note.actions[0],
            Action {
                label: "Approve".to_string(),
                primary: true
            }
        );
        assert_eq!(
            note.actions[1],
            Action {
                label: "Reject".to_string(),
                primary: false
            }
        );
        assert_eq!(
            note.actions[2],
            Action {
                label: "Open".to_string(),
                primary: false
            }
        );
    }

    // Error → High + [Open(primary), Retry] + sound=true.
    {
        let ev = notif_event(NotifKind::Error, "sess-err");
        let note = build_notification(&ev, &prefs).expect("Error must build");
        assert_eq!(note.priority, Priority::High);
        assert!(note.play_sound);
        assert_eq!(note.actions.len(), 2);
        assert_eq!(
            note.actions[0],
            Action {
                label: "Open".to_string(),
                primary: true
            }
        );
        assert_eq!(
            note.actions[1],
            Action {
                label: "Retry".to_string(),
                primary: false
            }
        );
    }

    // Done → Low + [Open, Dismiss] + sound=false.
    {
        let ev = notif_event(NotifKind::Done, "sess-done");
        let note = build_notification(&ev, &prefs).expect("Done must build");
        assert_eq!(note.priority, Priority::Low);
        assert!(!note.play_sound);
        assert_eq!(note.actions.len(), 2);
        assert_eq!(
            note.actions[0],
            Action {
                label: "Open".to_string(),
                primary: false
            }
        );
        assert_eq!(
            note.actions[1],
            Action {
                label: "Dismiss".to_string(),
                primary: false
            }
        );
    }

    // Started → default prefs has started=false → None.
    {
        let ev = notif_event(NotifKind::Started, "sess-start");
        let result = build_notification(&ev, &prefs);
        assert!(result.is_none(), "Started is off by default → None");
    }
}

// ── C. DND suppresses everything ──────────────────────────────────────────────

#[test]
fn dnd_suppresses_all_kinds() {
    let prefs = NotifPrefs {
        dnd: true,
        ..NotifPrefs::default()
    };

    for kind in [NotifKind::Needs, NotifKind::Error, NotifKind::Done] {
        let ev = notif_event(kind, "sess-dnd");
        assert!(
            build_notification(&ev, &prefs).is_none(),
            "DND must suppress all kinds"
        );
    }
}

// ── D. Per-kind toggle off → None ─────────────────────────────────────────────

#[test]
fn kind_toggle_off_suppresses() {
    // needs=false
    {
        let prefs = NotifPrefs {
            needs: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Needs, "s1");
        assert!(
            build_notification(&ev, &prefs).is_none(),
            "needs=false → None"
        );
    }
    // error=false
    {
        let prefs = NotifPrefs {
            error: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Error, "s2");
        assert!(
            build_notification(&ev, &prefs).is_none(),
            "error=false → None"
        );
    }
    // done=false
    {
        let prefs = NotifPrefs {
            done: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Done, "s3");
        assert!(
            build_notification(&ev, &prefs).is_none(),
            "done=false → None"
        );
    }
    // started=true (explicit enable)
    {
        let prefs = NotifPrefs {
            started: true,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Started, "s4");
        let note = build_notification(&ev, &prefs).expect("started=true → Some");
        assert_eq!(note.priority, Priority::Low);
        assert!(!note.play_sound);
        assert_eq!(note.actions.len(), 1);
        assert_eq!(
            note.actions[0],
            Action {
                label: "Open".to_string(),
                primary: false
            }
        );
    }
}

// ── E. play_sound gating ──────────────────────────────────────────────────────

#[test]
fn play_sound_gating() {
    // sound=true, High priority, dnd=false → play_sound=true.
    {
        let prefs = NotifPrefs {
            sound: true,
            dnd: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Needs, "s-snd");
        let note = build_notification(&ev, &prefs).unwrap();
        assert!(note.play_sound);
    }
    // sound=false → no sound even for High.
    {
        let prefs = NotifPrefs {
            sound: false,
            dnd: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Needs, "s-nosnd");
        let note = build_notification(&ev, &prefs).unwrap();
        assert!(!note.play_sound);
    }
    // Low priority (Done) → no sound even with sound=true.
    {
        let prefs = NotifPrefs {
            sound: true,
            dnd: false,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Done, "s-low");
        let note = build_notification(&ev, &prefs).unwrap();
        assert!(!note.play_sound);
    }
    // DND → build returns None; this also means no sound path (already covered by C).
    {
        let prefs = NotifPrefs {
            sound: true,
            dnd: true,
            ..NotifPrefs::default()
        };
        let ev = notif_event(NotifKind::Needs, "s-dnd");
        assert!(build_notification(&ev, &prefs).is_none());
    }
}

// ── F. group collapses same-kind bursts ───────────────────────────────────────

/// Helper: build a Notification directly for grouping tests (bypass prefs toggle for Started).
fn make_note(kind: NotifKind, session_id: &str) -> Notification {
    let prefs = NotifPrefs {
        started: true,
        ..NotifPrefs::default()
    };
    let ev = notif_event(kind, session_id);
    build_notification(&ev, &prefs).expect("must build")
}

#[test]
fn group_collapses_same_kind_burst_when_on() {
    let notes: Vec<Notification> = (0..5)
        .map(|i| make_note(NotifKind::Done, &format!("sess-done-{i}")))
        .collect();

    let prefs = NotifPrefs::default(); // group=true

    let result = group(&notes, &prefs);
    assert_eq!(result.len(), 1, "5 Done notes must collapse to 1 summary");
    let summary = &result[0];
    assert_eq!(summary.kind, NotifKind::Done);
    assert!(
        summary.title.contains('5'),
        "summary title must mention count: got {:?}",
        summary.title
    );
    assert!(
        summary.body.contains('5'),
        "summary body must mention count: got {:?}",
        summary.body
    );
}

#[test]
fn group_unchanged_when_off() {
    let notes: Vec<Notification> = (0..3)
        .map(|i| make_note(NotifKind::Done, &format!("sess-{i}")))
        .collect();

    let prefs = NotifPrefs {
        group: false,
        ..NotifPrefs::default()
    };
    let result = group(&notes, &prefs);
    assert_eq!(result.len(), 3, "group=false must return input unchanged");
}

#[test]
fn group_mixed_kinds_collapses_per_kind() {
    let mut notes = Vec::new();
    // 2 Done
    for i in 0..2 {
        notes.push(make_note(NotifKind::Done, &format!("d-{i}")));
    }
    // 3 Error
    for i in 0..3 {
        notes.push(make_note(NotifKind::Error, &format!("e-{i}")));
    }
    // 1 Needs
    notes.push(make_note(NotifKind::Needs, "n-0"));

    let prefs = NotifPrefs::default(); // group=true
    let result = group(&notes, &prefs);

    // 3 distinct kinds → 3 entries (each collapsed within its kind).
    assert_eq!(result.len(), 3);
    // First kind encountered is Done.
    assert_eq!(result[0].kind, NotifKind::Done);
    assert!(result[0].title.contains('2'));
    // Second kind is Error.
    assert_eq!(result[1].kind, NotifKind::Error);
    assert!(result[1].title.contains('3'));
    // Third kind is Needs — only 1, so no summary text needed (stays as-is).
    assert_eq!(result[2].kind, NotifKind::Needs);
}

#[test]
fn group_single_note_stays_unchanged() {
    let notes = vec![make_note(NotifKind::Error, "solo")];
    let prefs = NotifPrefs::default();
    let result = group(&notes, &prefs);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], notes[0]);
}

// ── G. CaptureNotifier delivers multiple in order ─────────────────────────────

#[test]
fn capture_notifier_records_all_delivered_in_order() {
    let prefs = NotifPrefs::default();
    let mut cap = CaptureNotifier::new();

    let events = [
        notif_event(NotifKind::Needs, "s1"),
        notif_event(NotifKind::Error, "s2"),
        notif_event(NotifKind::Done, "s3"),
    ];

    let mut built = Vec::new();
    for ev in &events {
        let note = build_notification(ev, &prefs).expect("must build");
        built.push(note.clone());
        cap.deliver(&note).expect("CaptureNotifier never fails");
    }

    let delivered = cap.delivered();
    assert_eq!(delivered.len(), 3);
    for (i, expected) in built.iter().enumerate() {
        assert_eq!(&delivered[i], expected, "delivery[{i}] must match");
    }
}

// ── H. Full cross-block pipeline: stream fold → notif → group → capture ───────

#[test]
fn full_pipeline_stream_fold_to_group_to_capture() {
    let ndjson =
        std::fs::read_to_string(QA_NDJSON).unwrap_or_else(|e| panic!("cannot read qa.ndjson: {e}"));

    // Fold the fixture (represents one finished session).
    let state = fold_ndjson(&ndjson);
    assert_eq!(state.phase, Phase::Done);

    let sid = state.session_id.as_deref().unwrap_or("unknown");

    // Simulate 3 sessions finishing (same-kind burst).
    let burst: Vec<Notification> = (0..3)
        .map(|i| {
            let ev = NotifEvent {
                kind: NotifKind::Done,
                session_id: format!("{sid}-{i}"),
                host: "localhost".to_string(),
                title: "Session finished".to_string(),
                body: format!("session {i} done"),
            };
            build_notification(&ev, &NotifPrefs::default())
                .expect("Done must build with default prefs")
        })
        .collect();

    let prefs = NotifPrefs::default(); // group=true
    let grouped = group(&burst, &prefs);
    assert_eq!(grouped.len(), 1, "3 Done notes must collapse to 1");

    let mut cap = CaptureNotifier::new();
    for note in &grouped {
        cap.deliver(note).expect("deliver must succeed");
    }

    let delivered = cap.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].kind, NotifKind::Done);
    assert!(delivered[0].title.contains('3'));
}
