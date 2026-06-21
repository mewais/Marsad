// branches.rs — ragged pairing, Empty padding cells, context interleaved between change blocks.
//
// Key algorithm from splitDiff (Marsad.dc.html lines 1260-1270):
//   while(i < L.length):
//     if sig==' ': push {l:cellStyle(L[i]), r:cellStyle(L[i])}; i++
//     collect dels[] while sig==='-'
//     collect adds[] while sig==='+'
//     n = max(dels.length, adds.length)
//     for k in 0..n: push {l:cellStyle(dels[k]||null), r:cellStyle(adds[k]||null)}
//
// NOTE: The spec says a run of `-` is paired against the "following" `+` run.
// A leading `+` run (no preceding `-`) uses dels=[], so left side is all Empty.
// A trailing `-` run (no following `+`) uses adds=[], so right side is all Empty.

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

// ── cell_style(None) → Empty padding cell ───────────────────────────────────

#[test]
fn cell_style_none_is_empty_kind() {
    let cell = cell_style(None);
    assert_eq!(cell.kind, CellKind::Empty);
    assert_eq!(cell.ln, "");
    assert_eq!(cell.text, "");
    assert_eq!(cell.sig, ' ');
}

// ── RAGGED: del > add  (more deletions than additions) ───────────────────────
//
//  Input:  del("1","A"), del("2","B"), del("3","C"), add("4","X")
//  Expected rows:
//    row 0: left=Del("A"), right=Add("X")
//    row 1: left=Del("B"), right=Empty
//    row 2: left=Del("C"), right=Empty

#[test]
fn ragged_del_gt_add_exact_rows() {
    let lines = vec![del("1", "A"), del("2", "B"), del("3", "C"), add("4", "X")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3, "3 rows: max(3 dels, 1 add)");

    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "A");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "X");

    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].left.text, "B");
    assert_eq!(rows[1].right.kind, CellKind::Empty);
    assert_eq!(rows[1].right.ln, "");
    assert_eq!(rows[1].right.sig, ' ');
    assert_eq!(rows[1].right.text, "");

    assert_eq!(rows[2].left.kind, CellKind::Del);
    assert_eq!(rows[2].left.text, "C");
    assert_eq!(rows[2].right.kind, CellKind::Empty);
    assert_eq!(rows[2].right.ln, "");
    assert_eq!(rows[2].right.sig, ' ');
    assert_eq!(rows[2].right.text, "");
}

// ── RAGGED: add > del  (more additions than deletions) ───────────────────────
//
//  Input:  del("1","X"), add("2","A"), add("3","B"), add("4","C")
//  Expected rows:
//    row 0: left=Del("X"), right=Add("A")
//    row 1: left=Empty,    right=Add("B")
//    row 2: left=Empty,    right=Add("C")

#[test]
fn ragged_add_gt_del_exact_rows() {
    let lines = vec![del("1", "X"), add("2", "A"), add("3", "B"), add("4", "C")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3, "3 rows: max(1 del, 3 adds)");

    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "X");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "A");

    assert_eq!(rows[1].left.kind, CellKind::Empty);
    assert_eq!(rows[1].left.ln, "");
    assert_eq!(rows[1].left.sig, ' ');
    assert_eq!(rows[1].left.text, "");
    assert_eq!(rows[1].right.kind, CellKind::Add);
    assert_eq!(rows[1].right.text, "B");

    assert_eq!(rows[2].left.kind, CellKind::Empty);
    assert_eq!(rows[2].left.ln, "");
    assert_eq!(rows[2].left.sig, ' ');
    assert_eq!(rows[2].left.text, "");
    assert_eq!(rows[2].right.kind, CellKind::Add);
    assert_eq!(rows[2].right.text, "C");
}

// ── RAGGED: large del run vs no adds ────────────────────────────────────────
//
//  Input:  del x 4, no following add
//  All right cells are Empty.

#[test]
fn ragged_all_del_no_add() {
    let lines = vec![
        del("1", "R1"),
        del("2", "R2"),
        del("3", "R3"),
        del("4", "R4"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 4);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.left.kind, CellKind::Del, "row {i} left must be Del");
        assert_eq!(
            row.right.kind,
            CellKind::Empty,
            "row {i} right must be Empty"
        );
    }
}

// ── RAGGED: no del run followed by large add run ─────────────────────────────
//
//  Input:  add x 4, no preceding del.
//  All left cells are Empty.

#[test]
fn ragged_all_add_no_del() {
    let lines = vec![
        add("1", "N1"),
        add("2", "N2"),
        add("3", "N3"),
        add("4", "N4"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 4);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.left.kind, CellKind::Empty, "row {i} left must be Empty");
        assert_eq!(row.right.kind, CellKind::Add, "row {i} right must be Add");
    }
}

// ── Context interleaved between two change blocks ────────────────────────────
//
//  Input:  del/add block → context → del/add block
//  Ensures the algorithm re-enters the del/add collector after context lines.

#[test]
fn context_interleaved_between_change_blocks() {
    let lines = vec![
        // block 1
        del("1", "old1"),
        add("2", "new1"),
        // context separator
        ctx("3", "unchanged"),
        // block 2 (ragged: 2 del, 1 add)
        del("4", "old2a"),
        del("5", "old2b"),
        add("6", "new2"),
    ];
    let rows = split_side_by_side(&lines);
    // block1: 1 row; ctx: 1 row; block2: max(2,1)=2 rows → total 4
    assert_eq!(rows.len(), 4);

    // block 1 row
    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "old1");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "new1");

    // context row
    assert_eq!(rows[1].left.kind, CellKind::Context);
    assert_eq!(rows[1].right.kind, CellKind::Context);
    assert_eq!(rows[1].left.text, "unchanged");
    assert_eq!(rows[1].right.text, "unchanged");

    // block 2, row 0: del("old2a") / add("new2")
    assert_eq!(rows[2].left.kind, CellKind::Del);
    assert_eq!(rows[2].left.text, "old2a");
    assert_eq!(rows[2].right.kind, CellKind::Add);
    assert_eq!(rows[2].right.text, "new2");

    // block 2, row 1: del("old2b") / Empty
    assert_eq!(rows[3].left.kind, CellKind::Del);
    assert_eq!(rows[3].left.text, "old2b");
    assert_eq!(rows[3].right.kind, CellKind::Empty);
    assert_eq!(rows[3].right.ln, "");
    assert_eq!(rows[3].right.sig, ' ');
    assert_eq!(rows[3].right.text, "");
}

// ── Multiple context lines between blocks ────────────────────────────────────

#[test]
fn multiple_context_lines_between_blocks() {
    let lines = vec![
        del("1", "d1"),
        add("2", "a1"),
        ctx("3", "c1"),
        ctx("4", "c2"),
        ctx("5", "c3"),
        del("6", "d2"),
        add("7", "a2"),
    ];
    let rows = split_side_by_side(&lines);
    // 1 + 3 + 1 = 5 rows
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[1].left.kind, CellKind::Context);
    assert_eq!(rows[2].left.kind, CellKind::Context);
    assert_eq!(rows[3].left.kind, CellKind::Context);
    assert_eq!(rows[4].left.kind, CellKind::Del);
    assert_eq!(rows[4].right.kind, CellKind::Add);
}

// ── unified preserves interleaved context and changes ────────────────────────

#[test]
fn unified_interleaved_order_preserved() {
    let lines = vec![
        ctx("1", "c1"),
        del("2", "d1"),
        del("3", "d2"),
        add("4", "a1"),
        ctx("5", "c2"),
        add("6", "a2"),
        add("7", "a3"),
        del("8", "d3"),
        ctx("9", "c3"),
    ];
    let rows = unified(&lines);
    assert_eq!(rows.len(), 9);
    let expected = [
        CellKind::Context,
        CellKind::Del,
        CellKind::Del,
        CellKind::Add,
        CellKind::Context,
        CellKind::Add,
        CellKind::Add,
        CellKind::Del,
        CellKind::Context,
    ];
    for (i, (row, expected_kind)) in rows.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            row.cell.kind, *expected_kind,
            "unified row {i} kind mismatch"
        );
    }
}

// ── Ragged 3-del / 2-add: assert all 3 rows precisely ────────────────────────

#[test]
fn ragged_three_del_two_add_exact_rows() {
    let lines = vec![
        del("1", "D1"),
        del("2", "D2"),
        del("3", "D3"),
        add("4", "A1"),
        add("5", "A2"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);

    assert_eq!(rows[0].left.text, "D1");
    assert_eq!(rows[0].right.text, "A1");
    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].right.kind, CellKind::Add);

    assert_eq!(rows[1].left.text, "D2");
    assert_eq!(rows[1].right.text, "A2");
    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].right.kind, CellKind::Add);

    assert_eq!(rows[2].left.text, "D3");
    assert_eq!(rows[2].right.kind, CellKind::Empty);
    assert_eq!(rows[2].right.ln, "");
    assert_eq!(rows[2].right.sig, ' ');
    assert_eq!(rows[2].right.text, "");
}

// ── Ragged 2-del / 4-add: assert all 4 rows precisely ────────────────────────

#[test]
fn ragged_two_del_four_add_exact_rows() {
    let lines = vec![
        del("1", "D1"),
        del("2", "D2"),
        add("3", "A1"),
        add("4", "A2"),
        add("5", "A3"),
        add("6", "A4"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 4);

    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "D1");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "A1");

    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].left.text, "D2");
    assert_eq!(rows[1].right.kind, CellKind::Add);
    assert_eq!(rows[1].right.text, "A2");

    assert_eq!(rows[2].left.kind, CellKind::Empty);
    assert_eq!(rows[2].left.ln, "");
    assert_eq!(rows[2].left.sig, ' ');
    assert_eq!(rows[2].left.text, "");
    assert_eq!(rows[2].right.kind, CellKind::Add);
    assert_eq!(rows[2].right.text, "A3");

    assert_eq!(rows[3].left.kind, CellKind::Empty);
    assert_eq!(rows[3].left.ln, "");
    assert_eq!(rows[3].left.sig, ' ');
    assert_eq!(rows[3].left.text, "");
    assert_eq!(rows[3].right.kind, CellKind::Add);
    assert_eq!(rows[3].right.text, "A4");
}

// ── ln values are threaded through correctly in ragged rows ──────────────────

#[test]
fn ragged_ln_values_correct() {
    let lines = vec![del("10", "D"), del("11", "E"), add("20", "A")];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].left.ln, "10");
    assert_eq!(rows[0].right.ln, "20");
    assert_eq!(rows[1].left.ln, "11");
    assert_eq!(rows[1].right.kind, CellKind::Empty);
    assert_eq!(rows[1].right.ln, "");
    assert_eq!(rows[1].right.sig, ' ');
    assert_eq!(rows[1].right.text, "");
}

// ── Two consecutive change blocks, no context between them ───────────────────
//
// NOTE: per the algorithm, a '+' run is only collected after a '-' run in the
// same pass. If we see: del del add add del add, the first pass collects
// [del,del] as dels, then [add,add] as adds (k in 0..2 → 2 rows).
// Then we continue and see del → dels=[del], adds=[add] → 1 row.
// Total: 3 rows.

#[test]
fn two_consecutive_change_blocks_no_context() {
    let lines = vec![
        del("1", "D1"),
        del("2", "D2"),
        add("3", "A1"),
        add("4", "A2"),
        del("5", "D3"),
        add("6", "A3"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);

    assert_eq!(rows[0].left.kind, CellKind::Del);
    assert_eq!(rows[0].left.text, "D1");
    assert_eq!(rows[0].right.kind, CellKind::Add);
    assert_eq!(rows[0].right.text, "A1");

    assert_eq!(rows[1].left.kind, CellKind::Del);
    assert_eq!(rows[1].left.text, "D2");
    assert_eq!(rows[1].right.kind, CellKind::Add);
    assert_eq!(rows[1].right.text, "A2");

    assert_eq!(rows[2].left.kind, CellKind::Del);
    assert_eq!(rows[2].left.text, "D3");
    assert_eq!(rows[2].right.kind, CellKind::Add);
    assert_eq!(rows[2].right.text, "A3");
}

// ── Context at beginning and end of diff ─────────────────────────────────────

#[test]
fn context_at_start_and_end() {
    let lines = vec![
        ctx("1", "start"),
        del("2", "removed"),
        add("3", "added"),
        ctx("4", "end"),
    ];
    let rows = split_side_by_side(&lines);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].left.kind, CellKind::Context);
    assert_eq!(rows[0].left.text, "start");
    assert_eq!(rows[2].right.kind, CellKind::Context);
    assert_eq!(rows[2].right.text, "end");
}
