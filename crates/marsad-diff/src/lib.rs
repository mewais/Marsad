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
