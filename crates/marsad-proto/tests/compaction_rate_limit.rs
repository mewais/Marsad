/// compaction_rate_limit.rs — U3b (marsad-full-experience "nothing silently
/// dropped"): `system/compact_boundary` and top-level `rate_limit_event`
/// stream lines graduate from `StreamMsg::Unknown` to typed variants.
///
/// Evidence for the wire shapes: `fixtures/streams/compaction.ndjson`, a REAL
/// capture (claude 2.1.198) that contains BOTH a `rate_limit_event` line
/// (line 3, before any assistant activity) and a `system/compact_boundary`
/// line (line 18, after a real `/compact`). Every literal JSON string in this
/// file is copied verbatim from that fixture — nothing here is invented.
///
/// These tests are RED against `marsad-proto` as of this commit: `StreamMsg`
/// has no `CompactBoundary`/`RateLimitEvent` variant yet, so `parse_stream_line`
/// routes both shapes to `StreamMsg::Unknown` and every `matches!`/field-access
/// assertion below fails to compile or fails at runtime.
use marsad_proto::{parse_stream_line, ProtoError, StreamMsg};

fn fixture_path(filename: &str) -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../fixtures/streams")
        .join(filename)
}

// ---------------------------------------------------------------------------
// compact_boundary — happy path, literal JSON copied from the real fixture
// ---------------------------------------------------------------------------

const COMPACT_BOUNDARY_LINE: &str = r#"{"type":"system","subtype":"compact_boundary","session_id":"26999d41-e8e2-4fbe-87d1-baf25122e1be","uuid":"bfb3644f-c8ea-4b2a-bb94-a081f64bf67e","compact_metadata":{"trigger":"manual","pre_tokens":21755,"post_tokens":1586,"cumulative_dropped_tokens":20169,"duration_ms":19756,"preserved_segment":{"head_uuid":"a738beb6-1418-49bd-81c8-e90db2370ba2","anchor_uuid":"20229789-b3e8-41b3-8724-65d060d331e1","tail_uuid":"a738beb6-1418-49bd-81c8-e90db2370ba2"},"preserved_messages":{"anchor_uuid":"20229789-b3e8-41b3-8724-65d060d331e1","uuids":["a738beb6-1418-49bd-81c8-e90db2370ba2"],"all_uuids":["a738beb6-1418-49bd-81c8-e90db2370ba2"]}}}"#;

#[test]
fn compact_boundary_parses_as_typed_variant_not_unknown() {
    let msg = parse_stream_line(COMPACT_BOUNDARY_LINE).expect("must parse without error");
    assert!(
        matches!(msg, StreamMsg::CompactBoundary(_)),
        "compact_boundary must be typed, not StreamMsg::Unknown; got {msg:?}"
    );
}

#[test]
fn compact_boundary_carries_session_id_and_metadata() {
    let msg = parse_stream_line(COMPACT_BOUNDARY_LINE).expect("must parse");
    match msg {
        StreamMsg::CompactBoundary(cb) => {
            assert_eq!(cb.session_id, "26999d41-e8e2-4fbe-87d1-baf25122e1be");
            assert_eq!(cb.compact_metadata.trigger, "manual");
            assert_eq!(cb.compact_metadata.pre_tokens, 21755);
            assert_eq!(cb.compact_metadata.post_tokens, 1586);
            assert_eq!(cb.compact_metadata.cumulative_dropped_tokens, 20169);
            assert_eq!(cb.compact_metadata.duration_ms, 19756);
        }
        other => panic!("expected CompactBoundary, got {other:?}"),
    }
}

/// `preserved_segment`/`preserved_messages` are real fields on the wire but
/// carry no UI-facing signal (internal compaction bookkeeping) — the typed
/// struct is allowed to ignore them (serde's default "unknown fields OK"
/// behavior for a non-`deny_unknown_fields` struct), so a real capture with
/// them present must still parse cleanly rather than erroring.
#[test]
fn compact_boundary_tolerates_extra_preserved_segment_fields() {
    parse_stream_line(COMPACT_BOUNDARY_LINE).expect("extra fields must not cause a parse error");
}

// ---------------------------------------------------------------------------
// compact_boundary — schema mismatch (still a genuine error, not Unknown)
// ---------------------------------------------------------------------------

#[test]
fn compact_boundary_missing_session_id_is_schema_mismatch() {
    let line =
        r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual"}}"#;
    let err = parse_stream_line(line).expect_err("missing session_id must be a schema error");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// `compact_metadata`'s own fields are individually defaulted (forward
/// compat with a claude version that trims the payload down), but a
/// `compact_boundary` missing the `compact_metadata` key entirely is not a
/// real compaction event worth modeling — SchemaMismatch, not a silently
/// empty struct.
#[test]
fn compact_boundary_missing_compact_metadata_is_schema_mismatch() {
    let line = r#"{"type":"system","subtype":"compact_boundary","session_id":"s1"}"#;
    let err = parse_stream_line(line).expect_err("missing compact_metadata must be a schema error");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// `compact_metadata`'s numeric/string sub-fields default when absent —
/// forward-compat with a claude version that trims the payload, mirroring
/// `Usage`'s all-fields-default convention.
#[test]
fn compact_boundary_metadata_subfields_default_when_absent() {
    let line =
        r#"{"type":"system","subtype":"compact_boundary","session_id":"s1","compact_metadata":{}}"#;
    let msg = parse_stream_line(line).expect("empty compact_metadata object must still parse");
    match msg {
        StreamMsg::CompactBoundary(cb) => {
            assert_eq!(cb.compact_metadata.trigger, "");
            assert_eq!(cb.compact_metadata.pre_tokens, 0);
            assert_eq!(cb.compact_metadata.post_tokens, 0);
            assert_eq!(cb.compact_metadata.cumulative_dropped_tokens, 0);
            assert_eq!(cb.compact_metadata.duration_ms, 0);
        }
        other => panic!("expected CompactBoundary, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// system/* non-compact_boundary subtypes stay Unknown — don't over-fit
// ---------------------------------------------------------------------------

/// A `system/status` line (the "compacting" progress notice that precedes a
/// real `compact_boundary` — see the fixture) is NOT `compact_boundary` and
/// must stay `StreamMsg::Unknown`, same as before this change. Only
/// `compact_boundary` graduates out of the forward-compat bucket.
#[test]
fn system_status_subtype_still_unknown() {
    let line = r#"{"type":"system","subtype":"status","status":"compacting","session_id":"s1","uuid":"u1"}"#;
    let msg = parse_stream_line(line).expect("must parse");
    assert!(
        matches!(msg, StreamMsg::Unknown(_)),
        "system/status must remain Unknown (not over-fit); got {msg:?}"
    );
}

// ---------------------------------------------------------------------------
// rate_limit_event — happy path, literal JSON copied from the real fixture
// ---------------------------------------------------------------------------

const RATE_LIMIT_EVENT_LINE: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1783014000,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled_until","isUsingOverage":false},"uuid":"e627c42d-a415-47ea-8ad6-ed7f42021fc6","session_id":"26999d41-e8e2-4fbe-87d1-baf25122e1be"}"#;

#[test]
fn rate_limit_event_parses_as_typed_variant_not_unknown() {
    let msg = parse_stream_line(RATE_LIMIT_EVENT_LINE).expect("must parse without error");
    assert!(
        matches!(msg, StreamMsg::RateLimitEvent(_)),
        "rate_limit_event must be typed, not StreamMsg::Unknown; got {msg:?}"
    );
}

#[test]
fn rate_limit_event_carries_session_id_and_info() {
    let msg = parse_stream_line(RATE_LIMIT_EVENT_LINE).expect("must parse");
    match msg {
        StreamMsg::RateLimitEvent(rl) => {
            assert_eq!(rl.session_id, "26999d41-e8e2-4fbe-87d1-baf25122e1be");
            assert_eq!(rl.rate_limit_info.status, "allowed");
            assert_eq!(rl.rate_limit_info.resets_at, 1783014000);
            assert_eq!(rl.rate_limit_info.rate_limit_type, "five_hour");
            assert_eq!(rl.rate_limit_info.overage_status, "rejected");
            assert!(!rl.rate_limit_info.is_using_overage);
        }
        other => panic!("expected RateLimitEvent, got {other:?}"),
    }
}

#[test]
fn rate_limit_event_missing_rate_limit_info_is_schema_mismatch() {
    let line = r#"{"type":"rate_limit_event","session_id":"s1"}"#;
    let err = parse_stream_line(line).expect_err("missing rate_limit_info must be a schema error");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// `session_id` is absent-tolerant (defaults to "") — the rate-limit signal
/// itself (`rate_limit_info`) is the load-bearing part of this event.
#[test]
fn rate_limit_event_session_id_defaults_when_absent() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
    let msg = parse_stream_line(line).expect("must parse without session_id");
    match msg {
        StreamMsg::RateLimitEvent(rl) => {
            assert_eq!(rl.session_id, "");
            assert_eq!(rl.rate_limit_info.status, "allowed");
            // Fields absent from this minimal payload all default.
            assert_eq!(rl.rate_limit_info.resets_at, 0);
            assert_eq!(rl.rate_limit_info.rate_limit_type, "");
            assert_eq!(rl.rate_limit_info.overage_status, "");
            assert!(!rl.rate_limit_info.is_using_overage);
        }
        other => panic!("expected RateLimitEvent, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// fixture — fold the real compaction.ndjson capture end to end
// ---------------------------------------------------------------------------

/// The real capture contains exactly one `rate_limit_event` (line 3) and
/// exactly one `system/compact_boundary` (line 18, after the real `/compact`)
/// — both must come back typed, not `Unknown`, and every line must still
/// parse without a `ProtoError` (this is the same fixture
/// `compaction_fixture_parses_all_lines` in `fixtures.rs` already exercises
/// for "no errors"; this test additionally pins the variant identities so a
/// regression back to `Unknown` is caught).
#[test]
fn compaction_fixture_yields_typed_compact_boundary_and_rate_limit_event() {
    let path = fixture_path("compaction.ndjson");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));

    let (mut compact_boundaries, mut rate_limit_events) = (0u32, 0u32);
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let msg =
            parse_stream_line(line).unwrap_or_else(|e| panic!("line {} failed: {e:?}", i + 1));
        match msg {
            StreamMsg::CompactBoundary(cb) => {
                compact_boundaries += 1;
                assert_eq!(cb.compact_metadata.trigger, "manual");
                assert_eq!(cb.compact_metadata.cumulative_dropped_tokens, 20169);
            }
            StreamMsg::RateLimitEvent(rl) => {
                rate_limit_events += 1;
                assert_eq!(rl.rate_limit_info.status, "allowed");
            }
            _ => {}
        }
    }
    assert_eq!(
        compact_boundaries, 1,
        "compaction.ndjson: exactly 1 compact_boundary"
    );
    assert_eq!(
        rate_limit_events, 1,
        "compaction.ndjson: exactly 1 rate_limit_event"
    );
}
