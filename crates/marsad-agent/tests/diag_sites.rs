//! What may reach a diagnostics record stays reviewable in the Marsad tree, as in Wasit
//! (`wasit-diag/tests/value_sites.rs`): nothing in Marsad implements `DiagValue`/`DiagEvent` by
//! hand or registers a field type, no code makes free text with `DiagText::new(`, and every
//! `ErrorInfo::ours(` call (free text from any `Display`) is in the allow-list below. Adding
//! one is a reviewed change here, with the reason why it is "not session content".

use std::path::Path;

use wasit_process::scan;

/// Patterns that extend what may reach a log record or an export line. Marsad registers no field
/// type (`diag_value!`) and implements none by hand: its events are made of Wasit's value types.
/// `diag_data!` (an enum served over a wire or written into a file, made only of value types) is
/// used in one file, [`PATTERN_ALLOW`].
const PATTERNS: &[&str] = &[
    "diag_value!",
    "diag_data!",
    "DiagValue for ",
    "DiagEvent for ",
    "Sealed for ",
    "SealedEvent for ",
];

/// The one file that declares `diag_data!` types: the lines of the diagnostics export file
/// (manifest, host facts, section headers, the end summary). Every field of every one of them is
/// a `wasit-diag` value type, so the exporter's own lines cannot hold content; the records between
/// them are copied from content-free logs and trails.
const PATTERN_ALLOW: &[&str] = &["marsad-proto/src/export.rs"];

const OURS: &str = "ErrorInfo::ours(";
const TEXT: &str = "DiagText::new(";

/// Files allowed to call `DiagText::new(`: `marsad-agent/src/diag.rs`, the one place free text
/// is made, with these kinds of text and no others:
/// * `shape` (`Origin::Transport`, through `transport`): the first 120 characters of what a
///   host printed before its report (a banner). Design §4.4 keeps this "bounded, scrubbed" (the
///   scrubber masks every registered secret, project path and the home directory; token-shaped
///   runs are redacted): the evidence for "the host would not connect". Called from
///   `hub/provision.rs`; and, through `transport` directly, a link's last error as the link
///   layer states it, for the export manifest (`hub/export`). What a transport command said when a connect failed (ssh and wsl
///   stderr, the reason as the user is shown it) is recorded through wasit-control's
///   `ConnectDiag`, which owns that text.
///   The shared ssh connection's records (`ssh.master_check`, `ssh.master_ended`,
///   `ssh.master_exit`, `hub/master.rs`) use the same `transport` function for what ssh said (the
///   last lines of the connection's log, the stderr of `ssh -O check|exit`): ssh's own words
///   about a connection, the same kind of text as a connect failure's stderr, scrubbed and
///   bounded the same way; they add no site to this list.
/// * `Reason` (`Origin::Own`): the same scrubbing for the words `serve` prints on stderr, so
///   that the line carries what the record carries.
/// * `host_note` (`Origin::HostShell`, first line only) and `host_path` (`Origin::Own`): what a
///   host's environment capture wrote about itself (why an attempt failed, how its setup ended;
///   never what the setup printed) and the entries of its `PATH`, for the manifest of a
///   diagnostics export (design §6.5). Called from `hub/export/facts.rs` only.
const TEXT_ALLOW: &[&str] = &["marsad-agent/src/diag.rs"];

/// Files allowed to call `ErrorInfo::ours(`, and what they pass:
/// * `marsad-agent/src/diag.rs` (`diag::ours`, the only call): the `Display` of an error of
///   Marsad's or Wasit's own making whose text names an operation and the OS's reason, or a
///   path of Marsad's own files (the data directory, the runtime directory, the socket, the
///   lock): the options error of `serve` (an unknown driver id from its own argv), the session
///   manager's error, the control crate's bind/lock/serve errors, service registration
///   conflicts, the serialization of the readiness line, and the bridge's "cannot reach the
///   daemon at <endpoint>" message. The Wasl node's and the hub's errors, whose text may quote
///   a journal or a stored document, and every error a peer wrote, use `diag::foreign`
///   (code, length and fingerprint only).
const OURS_ALLOW: &[&str] = &["marsad-agent/src/diag.rs"];

#[test]
fn field_types_and_free_text_only_in_reviewed_places() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = scan::find(&root, PATTERN_ALLOW, |line| {
        PATTERNS.iter().any(|p| scan::mentions(line, p))
    });
    assert!(
        found.is_empty(),
        "new diagnostics field types or free-text sites need review (allow-list in {}):\n{}",
        file!(),
        found.join("\n")
    );
}

#[test]
fn error_text_from_display_only_in_reviewed_places() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = scan::find(&root, OURS_ALLOW, |line| scan::mentions(line, OURS));
    assert!(
        found.is_empty(),
        "new `ErrorInfo::ours` sites need review (allow-list in {}):\n{}",
        file!(),
        found.join("\n")
    );
}

#[test]
fn free_text_from_a_transport_only_in_reviewed_places() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = scan::find(&root, TEXT_ALLOW, |line| scan::mentions(line, TEXT));
    assert!(
        found.is_empty(),
        "new `DiagText::new` sites need review (allow-list in {}):\n{}",
        file!(),
        found.join("\n")
    );
}

/// Each allow-listed file really contains its call (so an allow-list cannot go stale).
#[test]
fn the_allow_lists_are_live() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (f, call) in OURS_ALLOW
        .iter()
        .map(|f| (f, OURS))
        .chain(TEXT_ALLOW.iter().map(|f| (f, TEXT)))
    {
        let path = root.join("crates").join(f);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.lines()
                .any(|l| !l.trim_start().starts_with("//") && scan::mentions(l, call)),
            "{f} no longer calls {call}: remove it from the allow-list"
        );
    }
}

#[test]
fn the_scan_sees_each_pattern() {
    for line in [
        "wasit_diag::diag_value!(TraceId => Ident, |t| Ident::new(&t.0));",
        "wasit_diag::diag_data! {",
        "impl DiagValue for Prompt {",
        "impl wasit_diag::DiagEvent for X {",
    ] {
        assert!(PATTERNS.iter().any(|p| scan::mentions(line, p)), "{line}");
    }
    assert!(scan::mentions("ErrorInfo::ours(Code::st(\"x\"), e)", OURS));
    assert!(scan::mentions(
        "let t = DiagText::new(Origin::Own, &e.to_string());",
        TEXT
    ));
}
