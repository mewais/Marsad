// B05 marsad-format — degenerate / edge-input tests
// Covers: 0 tokens, 0 secs, exactly 1000 tokens, exactly 3600s.

use marsad_format::{fmt_elapsed, fmt_tokens};

// ---------- fmt_tokens edge inputs ----------

#[test]
fn fmt_tokens_zero() {
    // 0 is below 1000 → plain integer string
    assert_eq!(fmt_tokens(0), "0");
}

#[test]
fn fmt_tokens_exactly_1000_strips_trailing_dot_zero() {
    // 1000 / 1000 = 1.0 → strip trailing ".0" → "1k"
    assert_eq!(fmt_tokens(1000), "1k");
}

#[test]
fn fmt_tokens_1() {
    assert_eq!(fmt_tokens(1), "1");
}

#[test]
fn fmt_tokens_999() {
    // still below threshold
    assert_eq!(fmt_tokens(999), "999");
}

// ---------- fmt_elapsed edge inputs ----------

#[test]
fn fmt_elapsed_zero_seconds() {
    // 0s = 0m 00s
    assert_eq!(fmt_elapsed(0), "0m 00s");
}

#[test]
fn fmt_elapsed_exactly_3600_switches_to_hh_mm() {
    // 3600s = 60m = 1h 0m  → hours mode: "1h 0m"
    assert_eq!(fmt_elapsed(3600), "1h 0m");
}

#[test]
fn fmt_elapsed_3601_seconds() {
    // 3601s = 60m 1s → hours mode: "1h 0m"
    // NOTE: spec says ">= 60 minutes" → hours mode; 3601/60 = 60m remainder, h=1, m%60=0
    assert_eq!(fmt_elapsed(3601), "1h 0m");
}

#[test]
fn fmt_elapsed_59_seconds() {
    // 59s = 0m 59s
    assert_eq!(fmt_elapsed(59), "0m 59s");
}

#[test]
fn fmt_elapsed_60_seconds() {
    // 60s = 1m 00s (still under 60 minutes → minutes+seconds mode)
    assert_eq!(fmt_elapsed(60), "1m 00s");
}
