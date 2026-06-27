//! marsad-diff — side-by-side and unified diff row model + cell styling.
//!
//! Direct port of `splitDiff` + `cellStyle` from the Marsad design handoff.
//! See docs/specs/diff.md for the authoritative contract.

use serde::{Deserialize, Serialize};

// ── Public types ─────────────────────────────────────────────────────────────

/// Sigil indicating the role of a diff line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Sig {
    /// Context line (unchanged, `' '` in unified diff output).
    Context,
    /// Added line (`'+'`).
    Add,
    /// Deleted line (`'-'`).
    Del,
}

/// A single logical line from a unified diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    /// Line number label (may be left-side or right-side depending on caller).
    pub ln: String,
    /// Sigil: Context / Add / Del.
    pub sig: Sig,
    /// The text content of the line (without the sigil prefix).
    pub text: String,
}

/// Kind of cell, driving visual styling in the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CellKind {
    /// Unchanged context.
    Context,
    /// Added line.
    Add,
    /// Deleted line.
    Del,
    /// Empty padding cell in a ragged del/add pair.
    Empty,
}

/// A styled cell in a side-by-side or unified row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// Line number label (`""` for Empty cells).
    pub ln: String,
    /// Single-character sigil: `' '`, `'+'`, or `'-'`.
    pub sig: char,
    /// Text content (`""` for Empty cells).
    pub text: String,
    /// Cell kind for styling.
    pub kind: CellKind,
}

/// One row of a side-by-side diff view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideRow {
    pub left: Cell,
    pub right: Cell,
}

/// One row of a unified (mobile stacked) diff view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedRow {
    pub cell: Cell,
}

// ── Public functions ─────────────────────────────────────────────────────────

/// Convert an optional `DiffLine` reference into a styled `Cell`.
///
/// - `Some(line)` → cell whose kind/sig/ln/text mirror the line.
/// - `None` → Empty padding cell: `ln:""`, `sig:' '`, `text:""`, `kind:Empty`.
pub fn cell_style(line: Option<&DiffLine>) -> Cell {
    match line {
        Some(dl) => {
            let (sig, kind) = match dl.sig {
                Sig::Context => (' ', CellKind::Context),
                Sig::Add => ('+', CellKind::Add),
                Sig::Del => ('-', CellKind::Del),
            };
            Cell {
                ln: dl.ln.clone(),
                sig,
                text: dl.text.clone(),
                kind,
            }
        }
        None => Cell {
            ln: String::new(),
            sig: ' ',
            text: String::new(),
            kind: CellKind::Empty,
        },
    }
}

/// Build side-by-side rows from a slice of `DiffLine`s.
///
/// Algorithm (port of `splitDiff`):
/// ```text
/// while i < lines.len():
///   if sig == Context: push {l: cell_style(line), r: cell_style(line)}; i++
///   collect dels[] while sig == Del
///   collect adds[] while sig == Add
///   n = max(dels.len(), adds.len())
///   for k in 0..n: push {l: cell_style(dels.get(k)), r: cell_style(adds.get(k))}
/// ```
pub fn split_side_by_side(lines: &[DiffLine]) -> Vec<SideRow> {
    let mut rows: Vec<SideRow> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if lines[i].sig == Sig::Context {
            let cell = cell_style(Some(&lines[i]));
            rows.push(SideRow {
                left: cell.clone(),
                right: cell,
            });
            i += 1;
            continue;
        }

        // Collect a run of deletions.
        let mut dels: Vec<&DiffLine> = Vec::new();
        while i < lines.len() && lines[i].sig == Sig::Del {
            dels.push(&lines[i]);
            i += 1;
        }

        // Collect the immediately following run of additions.
        let mut adds: Vec<&DiffLine> = Vec::new();
        while i < lines.len() && lines[i].sig == Sig::Add {
            adds.push(&lines[i]);
            i += 1;
        }

        // Pair them row-by-row, padding the shorter side with Empty cells.
        let n = dels.len().max(adds.len());
        for k in 0..n {
            rows.push(SideRow {
                left: cell_style(dels.get(k).copied()),
                right: cell_style(adds.get(k).copied()),
            });
        }
    }

    rows
}

/// Build unified rows from a slice of `DiffLine`s.
///
/// Each line maps to exactly one `UnifiedRow` in input order.
pub fn unified(lines: &[DiffLine]) -> Vec<UnifiedRow> {
    lines
        .iter()
        .map(|dl| UnifiedRow {
            cell: cell_style(Some(dl)),
        })
        .collect()
}

/// Metadata / header prefixes in a `git diff` unified patch that carry no body
/// content and must be skipped when extracting `DiffLine`s.
const SKIP_PREFIXES: &[&str] = &[
    "diff ",
    "index ",
    "--- ",
    "+++ ",
    "@@",
    "new file",
    "deleted file",
    "old mode",
    "new mode",
    "similarity",
    "rename ",
    "copy ",
    "Binary files",
    r"\ No newline",
];

/// Parse a `git diff` unified patch into a flat list of `DiffLine`s.
///
/// Header / metadata lines (see [`SKIP_PREFIXES`]) are dropped. Body lines are
/// classified by their leading character — `'+'` → [`Sig::Add`], `'-'` →
/// [`Sig::Del`], `' '` → [`Sig::Context`] — and the leading character is
/// stripped from the stored text. Lines that match none of the above (e.g. a
/// truly empty line in the patch stream) are treated as context with empty text.
///
/// The `ln` label is left empty: unified patches do not carry a per-line number
/// without reconstructing hunk ranges, which is out of scope here.
///
/// Empty input yields an empty `Vec`.
pub fn parse_unified(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();

    for raw in diff.lines() {
        if SKIP_PREFIXES.iter().any(|p| raw.starts_with(p)) {
            continue;
        }

        let (sig, text) = match raw.chars().next() {
            Some('+') => (Sig::Add, &raw[1..]),
            Some('-') => (Sig::Del, &raw[1..]),
            Some(' ') => (Sig::Context, &raw[1..]),
            // A bare empty line (no leading sigil) → context with empty text.
            _ => (Sig::Context, raw),
        };

        lines.push(DiffLine {
            ln: String::new(),
            sig,
            text: text.to_string(),
        });
    }

    lines
}

#[cfg(test)]
mod parse_unified_tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_vec() {
        assert!(parse_unified("").is_empty());
    }

    #[test]
    fn modified_file_patch_adds_dels_context() {
        let diff = "\
diff --git a/foo.txt b/foo.txt
index e69de29..4b825dc 100644
--- a/foo.txt
+++ b/foo.txt
@@ -1,3 +1,3 @@
 context line
-removed line
+added line
 trailing context";
        let lines = parse_unified(diff);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].sig, Sig::Context);
        assert_eq!(lines[0].text, "context line");
        assert_eq!(lines[1].sig, Sig::Del);
        assert_eq!(lines[1].text, "removed line");
        assert_eq!(lines[2].sig, Sig::Add);
        assert_eq!(lines[2].text, "added line");
        assert_eq!(lines[3].sig, Sig::Context);
        assert_eq!(lines[3].text, "trailing context");
    }

    #[test]
    fn all_additions_patch() {
        let diff = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+first
+second";
        let lines = parse_unified(diff);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.sig == Sig::Add));
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].text, "second");
    }

    #[test]
    fn skips_all_header_and_metadata_prefixes() {
        // Every skippable prefix appears at least once; only the body line stays.
        let diff = "\
diff --git a/x b/x
index aaa..bbb 100644
old mode 100644
new mode 100755
similarity index 90%
rename from a
copy from b
new file mode 100644
deleted file mode 100644
--- a/x
+++ b/x
@@ -1 +1 @@
 only body line";
        let lines = parse_unified(diff);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].sig, Sig::Context);
        assert_eq!(lines[0].text, "only body line");
    }

    #[test]
    fn skips_no_newline_marker() {
        let diff = "\
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file";
        let lines = parse_unified(diff);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].sig, Sig::Del);
        assert_eq!(lines[0].text, "old");
        assert_eq!(lines[1].sig, Sig::Add);
        assert_eq!(lines[1].text, "new");
    }

    #[test]
    fn skips_binary_files_line() {
        let diff = "\
diff --git a/img.png b/img.png
index 1111..2222 100644
Binary files a/img.png and b/img.png differ";
        assert!(parse_unified(diff).is_empty());
    }

    #[test]
    fn bare_empty_line_becomes_empty_context() {
        // A blank line in the patch stream (a context line whose content is "").
        // git emits these as " " but a stripped/edge stream may carry just "".
        let diff = "@@ -1,2 +1,2 @@\n context\n\n+added";
        let lines = parse_unified(diff);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].sig, Sig::Context);
        assert_eq!(lines[0].text, "context");
        assert_eq!(lines[1].sig, Sig::Context);
        assert_eq!(lines[1].text, "");
        assert_eq!(lines[2].sig, Sig::Add);
        assert_eq!(lines[2].text, "added");
    }
}
