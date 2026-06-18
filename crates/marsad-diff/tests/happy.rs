// happy.rs — nominal paths for every public fn/variant
// Tests: cell_style for each Sig variant, split_side_by_side with a mixed diff,
//        unified row ordering, context lines appear on BOTH sides.

use marsad_diff::{cell_style, split_side_by_side, unified, Cell, CellKind, DiffLine, Sig, SideRow, UnifiedRow};

// ── helpers ─────────────────────────────────────────────────────────────────

fn ctx(ln: &str, text: &str) -> DiffLine {
    DiffLine { sig: Sig::Context, text: text.to_string(), ln: ln.to_string() }
}
fn add(ln: &str, text: &str) -> DiffLine {
    DiffLine { sig: Sig::Add, text: text.to_string(), ln: ln.to_string() }
}
fn del(ln: &str, text: &str) -> DiffLine {
    DiffLine { sig: Sig::Del, text: text.to_string(), ln: ln.to_string() }
}

// ── cell_style: each Sig variant ────────────────────────────────────────────

#[test]
fn cell_style_context() {
    let line = ctx("1", "unchanged");
    let cell = cell_style(Some(&line));
    assert_eq!(cell.ln, "1");
    assert_eq!(cell.sig, ' ');
    assert_eq!(cell.text, "unchanged");
    assert_eq!(cell.kind, CellKind::Context);
}

#[test]
fn cell_style_add() {
    let line = add("3", "new line");
    let cell = cell_style(Some(&line));
    assert_eq!(cell.ln, "3");
    assert_eq!(cell.sig, '+');
    assert_eq!(cell.text, "new line");
    assert_eq!(cell.kind, CellKind::Add);
}

#[test]
fn cell_style_del() {
    let line = del("2", "old line");
    let cell = cell_style(Some(&line));
    assert_eq!(cell.ln, "2");
    assert_eq!(cell.sig, '-');
    assert_eq!(cell.text, "old line");
    assert_eq!(cell.kind, CellKind::Del);
}

// ── cell_style(None) → Empty padding cell ───────────────────────────────────

#[test]
fn cell_style_none_returns_empty_padding() {
    let cell = cell_style(None);
    assert_eq!(cell.ln, "");
    assert_eq!(cell.sig, ' ');
    assert_eq!(cell.text, "");
    assert_eq!(cell.kind, CellKind::Empty);
}

// ── Context lines appear on BOTH sides of a SideRow ─────────────────────────

#[test]
fn context_line_appears_on_both_sides() {
    let lines = vec![ctx("5", "same on both")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.left.kind, CellKind::Context);
    assert_eq!(row.right.kind, CellKind::Context);
    assert_eq!(row.left.text, "same on both");
    assert_eq!(row.right.text, "same on both");
    assert_eq!(row.left.ln, "5");
    assert_eq!(row.right.ln, "5");
}

// ── Mixed diff: del+add pair plus context, check complete row sequence ───────

#[test]
fn split_side_by_side_mixed_diff() {
    // context / del / add / context
    let lines = vec![
        ctx("1", "header"),
        del("2", "removed"),
        add("3", "inserted"),
        ctx("4", "footer"),
    ];
    let rows = split_side_by_side(&lines);
    // expect: [ctx/ctx], [del/add], [ctx/ctx]
    assert_eq!(rows.len(), 3);

    // row 0: context both sides
    assert_eq!(rows[0].left.kind, CellKind::Context);
    assert_eq!(rows[0].right.kind, CellKind::Context);
    assert_eq!(rows[0].left.text, "header");

    // row 1: del on left, add on right
    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].right.kind, CellKind::Add);
    assert_eq!(rows[1].left.text, "removed");
    assert_eq!(rows[1].right.text, "inserted");

    // row 2: context both sides
    assert_eq!(rows[2].left.kind, CellKind::Context);
    assert_eq!(rows[2].right.kind, CellKind::Context);
    assert_eq!(rows[2].left.text, "footer");
}

// ── Equal-length del/add run paired row-by-row ──────────────────────────────

#[test]
fn equal_del_add_run_paired_row_by_row() {
    let lines = vec![
        del("1", "line A"),
        del("2", "line B"),
        add("3", "line X"),
        add("4", "line Y"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].left.text, "line A");
    assert_eq!(rows[0].right.text, "line X");
    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].right.kind, CellKind::Add);

    assert_eq!(rows[1].left.text, "line B");
    assert_eq!(rows[1].right.text, "line Y");
    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].right.kind, CellKind::Add);
}

// ── unified preserves order, one row per line ───────────────────────────────

#[test]
fn unified_preserves_order() {
    let lines = vec![
        ctx("1", "ctx1"),
        del("2", "del1"),
        add("3", "add1"),
        ctx("4", "ctx2"),
    ];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].cell.kind, CellKind::Context);
    assert_eq!(rows[0].cell.text, "ctx1");
    assert_eq!(rows[1].cell.kind, CellKind::Del);
    assert_eq!(rows[1].cell.text, "del1");
    assert_eq!(rows[2].cell.kind, CellKind::Add);
    assert_eq!(rows[2].cell.text, "add1");
    assert_eq!(rows[3].cell.kind, CellKind::Context);
    assert_eq!(rows[3].cell.text, "ctx2");
}

#[test]
fn unified_single_row_has_correct_cell() {
    let lines = vec![add("7", "only add")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cell.kind, CellKind::Add);
    assert_eq!(rows[0].cell.text, "only add");
    assert_eq!(rows[0].cell.ln, "7");
    assert_eq!(rows[0].cell.sig, '+');
}

// ── Clone + PartialEq on public types ───────────────────────────────────────

#[test]
fn types_derive_clone_and_partial_eq() {
    let line = ctx("1", "text");
    let cell = cell_style(Some(&line));
    assert_eq!(cell.clone(), cell);

    let row = SideRow { left: cell.clone(), right: cell.clone() };
    assert_eq!(row.clone(), row);

    let urow = UnifiedRow { cell: cell.clone() };
    assert_eq!(urow.clone(), urow);
}
