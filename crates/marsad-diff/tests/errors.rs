// errors.rs — degenerate/edge inputs (no error type in marsad-diff; these are boundary cases)
// Tests: empty slice → empty output, single lines of each kind, all-context/all-add/all-del diffs.

use marsad_diff::{cell_style, split_side_by_side, unified, CellKind, DiffLine, Sig};

// ── helpers ─────────────────────────────────────────────────────────────────

fn ctx(ln: &str, text: &str) -> DiffLine {
    DiffLine {
        sig: Sig::Context,
        text: text.to_string(),
        ln: ln.to_string(),
    }
}
fn add(ln: &str, text: &str) -> DiffLine {
    DiffLine {
        sig: Sig::Add,
        text: text.to_string(),
        ln: ln.to_string(),
    }
}
fn del(ln: &str, text: &str) -> DiffLine {
    DiffLine {
        sig: Sig::Del,
        text: text.to_string(),
        ln: ln.to_string(),
    }
}

// ── empty input → empty output ───────────────────────────────────────────────

#[test]
fn split_empty_input_returns_empty() {
    let rows = split_side_by_side(&[]);
    assert!(rows.is_empty());
}

#[test]
fn unified_empty_input_returns_empty() {
    let rows = unified(&[]);
    assert!(rows.is_empty());
}

// ── single line of each kind ─────────────────────────────────────────────────

#[test]
fn split_single_context_line() {
    let lines = vec![ctx("1", "only ctx")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].left.kind, CellKind::Context);
    assert_eq!(rows[0].right.kind, CellKind::Context);
}

#[test]
fn split_single_add_line() {
    // A lone add with no preceding del: left side gets Empty padding, right gets Add.
    let lines = vec![add("1", "lone add")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].left.kind, CellKind::Empty);
    assert_eq!(rows[0].left.ln, "");
    assert_eq!(rows[0].left.sig, ' ');
    assert_eq!(rows[0].left.text, "");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "lone add");
}

#[test]
fn split_single_del_line() {
    // A lone del with no following add: left gets Del, right gets Empty padding.
    let lines = vec![del("1", "lone del")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "lone del");
    assert_eq!(rows[0].right.kind, CellKind::Empty);
    assert_eq!(rows[0].right.ln, "");
    assert_eq!(rows[0].right.sig, ' ');
    assert_eq!(rows[0].right.text, "");
}

#[test]
fn unified_single_context_line() {
    let lines = vec![ctx("1", "ctx only")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cell.kind, CellKind::Context);
}

#[test]
fn unified_single_add_line() {
    let lines = vec![add("2", "add only")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cell.kind, CellKind::Add);
}

#[test]
fn unified_single_del_line() {
    let lines = vec![del("3", "del only")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cell.kind, CellKind::Del);
}

// ── all-context diff ─────────────────────────────────────────────────────────

#[test]
fn split_all_context() {
    let lines = vec![ctx("1", "a"), ctx("2", "b"), ctx("3", "c")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.left.kind, CellKind::Context);
        assert_eq!(row.right.kind, CellKind::Context);
        // text is the same on both sides for context lines
        assert_eq!(row.left.text, row.right.text);
        assert_eq!(row.left.ln, row.right.ln);
    }
}

#[test]
fn unified_all_context() {
    let lines = vec![ctx("1", "a"), ctx("2", "b"), ctx("3", "c")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.cell.kind, CellKind::Context);
    }
}

// ── all-add diff ─────────────────────────────────────────────────────────────

#[test]
fn split_all_add() {
    let lines = vec![add("1", "a"), add("2", "b"), add("3", "c")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.left.kind, CellKind::Empty);
        assert_eq!(row.right.kind, CellKind::Add);
    }
}

#[test]
fn unified_all_add() {
    let lines = vec![add("1", "x"), add("2", "y")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.cell.kind == CellKind::Add));
}

// ── all-del diff ─────────────────────────────────────────────────────────────

#[test]
fn split_all_del() {
    let lines = vec![del("1", "a"), del("2", "b"), del("3", "c")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.left.kind, CellKind::Del);
        assert_eq!(row.right.kind, CellKind::Empty);
    }
}

#[test]
fn unified_all_del() {
    let lines = vec![del("1", "x"), del("2", "y")];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.cell.kind == CellKind::Del));
}

// ── cell_style fields are accessible (struct is not opaque) ──────────────────

#[test]
fn cell_fields_are_public() {
    let line = add("99", "field check");
    let c = cell_style(Some(&line));
    let _ = &c.ln;
    let _ = &c.sig;
    let _ = &c.text;
    let _ = &c.kind;
}

// ── DiffLine fields are public ───────────────────────────────────────────────

#[test]
fn diff_line_fields_are_public() {
    let line = del("5", "check");
    let _ = &line.sig;
    let _ = &line.text;
    let _ = &line.ln;
}
