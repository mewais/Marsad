/// branches.rs — LineFramer coverage: every branch in the framer.
///
/// LineFramer is the line-framing heart of the crate.  These tests drive 100%
/// line+region coverage of its logic by exercising all splits and edge cases.
use marsad_transport::LineFramer;

// ---------------------------------------------------------------------------
// 1. Empty push — returns no lines; pending remains empty.
// ---------------------------------------------------------------------------

#[test]
fn empty_push_returns_no_lines() {
    let mut f = LineFramer::new();
    let lines = f.push(b"");
    assert!(lines.is_empty(), "empty push must yield no lines");
    assert_eq!(f.pending(), "", "pending must be empty after empty push");
}

// ---------------------------------------------------------------------------
// 2. Single complete line — returns exactly that line; pending is empty.
// ---------------------------------------------------------------------------

#[test]
fn single_complete_line() {
    let mut f = LineFramer::new();
    let lines = f.push(b"hello\n");
    assert_eq!(lines, vec!["hello"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 3. Partial line, then its completion in a later push.
// ---------------------------------------------------------------------------

#[test]
fn partial_then_completion() {
    let mut f = LineFramer::new();
    // First push: only the prefix, no newline.
    let partial = f.push(b"hel");
    assert!(partial.is_empty(), "no complete line yet");
    assert_eq!(f.pending(), "hel");

    // Second push: rest of the line including the newline.
    let complete = f.push(b"lo\n");
    assert_eq!(complete, vec!["hello"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 4. Several complete lines in one push.
// ---------------------------------------------------------------------------

#[test]
fn several_lines_in_one_push() {
    let mut f = LineFramer::new();
    let lines = f.push(b"alpha\nbeta\ngamma\n");
    assert_eq!(lines, vec!["alpha", "beta", "gamma"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 5. Trailing partial left in pending() after a push with complete lines.
// ---------------------------------------------------------------------------

#[test]
fn trailing_partial_in_pending() {
    let mut f = LineFramer::new();
    let lines = f.push(b"first\nsecond\nincomplete");
    assert_eq!(lines, vec!["first", "second"]);
    assert_eq!(f.pending(), "incomplete");
}

// ---------------------------------------------------------------------------
// 6. Blank lines (just '\n') are preserved as empty strings.
// ---------------------------------------------------------------------------

#[test]
fn blank_line_is_preserved() {
    let mut f = LineFramer::new();
    let lines = f.push(b"\n");
    assert_eq!(lines, vec![""]);
    assert_eq!(f.pending(), "");
}

#[test]
fn multiple_blank_lines() {
    let mut f = LineFramer::new();
    let lines = f.push(b"\n\n\n");
    assert_eq!(lines, vec!["", "", ""]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 7. Blank lines mixed with content.
// ---------------------------------------------------------------------------

#[test]
fn blank_lines_mixed_with_content() {
    let mut f = LineFramer::new();
    let lines = f.push(b"first\n\nsecond\n");
    assert_eq!(lines, vec!["first", "", "second"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 8. Bytes split at arbitrary boundaries reassemble to the correct lines.
//    This covers: newline exactly at boundary, newline not at boundary, etc.
// ---------------------------------------------------------------------------

#[test]
fn split_before_newline() {
    let mut f = LineFramer::new();
    // "data\n" split so the '\n' is delivered in the second push.
    assert!(f.push(b"data").is_empty());
    let lines = f.push(b"\n");
    assert_eq!(lines, vec!["data"]);
    assert_eq!(f.pending(), "");
}

#[test]
fn split_after_newline() {
    let mut f = LineFramer::new();
    // "line\npart" — newline and next partial in same push.
    let lines = f.push(b"line\npart");
    assert_eq!(lines, vec!["line"]);
    assert_eq!(f.pending(), "part");
    let lines2 = f.push(b"ial\n");
    assert_eq!(lines2, vec!["partial"]);
    assert_eq!(f.pending(), "");
}

#[test]
fn split_at_every_byte() {
    // Feed b"abc\ndef\n" one byte at a time; must reassemble correctly.
    let input = b"abc\ndef\n";
    let mut f = LineFramer::new();
    let mut all_lines: Vec<String> = Vec::new();
    for byte in input {
        let lines = f.push(std::slice::from_ref(byte));
        all_lines.extend(lines);
    }
    assert_eq!(all_lines, vec!["abc", "def"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 9. Many partial pushes followed by a newline — reassemble to one long line.
// ---------------------------------------------------------------------------

#[test]
fn many_partials_then_newline() {
    let mut f = LineFramer::new();
    let chunks: &[&[u8]] = &[b"fo", b"ob", b"ar", b"ba", b"z"];
    for chunk in chunks {
        let lines = f.push(chunk);
        assert!(lines.is_empty(), "no complete line yet");
    }
    assert_eq!(f.pending(), "foobarbaz");
    let lines = f.push(b"\n");
    assert_eq!(lines, vec!["foobarbaz"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 10. Mixed: partial, several complete, trailing partial.
// ---------------------------------------------------------------------------

#[test]
fn partial_then_multiple_complete_then_partial() {
    let mut f = LineFramer::new();
    // Build up a prefix.
    assert!(f.push(b"sta").is_empty());
    assert_eq!(f.pending(), "sta");

    // Now push the completion of the first line + two more + a trailing partial.
    let lines = f.push(b"rt\nmiddle\nend\nnext");
    assert_eq!(lines, vec!["start", "middle", "end"]);
    assert_eq!(f.pending(), "next");

    // Finish the last one.
    let final_lines = f.push(b"\n");
    assert_eq!(final_lines, vec!["next"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 11. Long line (to exercise any internal buffer growth).
// ---------------------------------------------------------------------------

#[test]
fn long_line_reassembles() {
    let payload: String = "x".repeat(65_536);
    let mut input = payload.clone();
    input.push('\n');

    let mut f = LineFramer::new();
    // Push in 1024-byte chunks.
    let mut all_lines: Vec<String> = Vec::new();
    for chunk in input.as_bytes().chunks(1024) {
        all_lines.extend(f.push(chunk));
    }
    assert_eq!(all_lines, vec![payload]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 12. Push after a framer that already has trailing partial from previous push.
// ---------------------------------------------------------------------------

#[test]
fn accumulate_pending_across_multiple_calls() {
    let mut f = LineFramer::new();
    assert!(f.push(b"a").is_empty());
    assert_eq!(f.pending(), "a");
    assert!(f.push(b"b").is_empty());
    assert_eq!(f.pending(), "ab");
    assert!(f.push(b"c").is_empty());
    assert_eq!(f.pending(), "abc");
    let lines = f.push(b"\n");
    assert_eq!(lines, vec!["abc"]);
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 13. new() starts with empty pending.
// ---------------------------------------------------------------------------

#[test]
fn new_starts_empty() {
    let f = LineFramer::new();
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 14. Two complete lines then an empty push — pending stays empty.
// ---------------------------------------------------------------------------

#[test]
fn empty_push_after_complete_lines() {
    let mut f = LineFramer::new();
    let _ = f.push(b"one\ntwo\n");
    let lines = f.push(b"");
    assert!(lines.is_empty());
    assert_eq!(f.pending(), "");
}

// ---------------------------------------------------------------------------
// 15. Newline-only followed by content — produces blank then content.
// ---------------------------------------------------------------------------

#[test]
fn newline_then_content() {
    let mut f = LineFramer::new();
    let lines = f.push(b"\nhello\n");
    assert_eq!(lines, vec!["", "hello"]);
    assert_eq!(f.pending(), "");
}
