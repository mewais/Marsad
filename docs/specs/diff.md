# B04 — `marsad-diff`

**Tier:** L0. **Mode:** one-shot. **Coverage:** 100% line+region.

## Purpose

The diff model shared by the desktop (side-by-side) and mobile (unified) Changes/Edit-gate views.
Direct port of `splitDiff` + `cellStyle` from the `.dc.html` files. No IO.

## Public API (authoritative)

```rust
pub struct DiffLine { pub sig: Sig, pub text: String, pub ln: String }   // sig: ' ' context, '+' add, '-' del
pub enum Sig { Context, Add, Del }

pub struct Cell { pub ln: String, pub sig: char, pub text: String, pub kind: CellKind } // kind drives styling
pub enum CellKind { Context, Add, Del, Empty }                           // Empty = padding cell in a ragged pair

pub struct SideRow { pub left: Cell, pub right: Cell }
pub struct UnifiedRow { pub cell: Cell }

pub fn cell_style(line: Option<&DiffLine>) -> Cell;          // None -> Empty padding cell (ln:"", sig:' ', text:"", kind:Empty)
pub fn split_side_by_side(lines: &[DiffLine]) -> Vec<SideRow>;
pub fn unified(lines: &[DiffLine]) -> Vec<UnifiedRow>;
```

Derive `Debug, Clone, PartialEq` (+ serde).

## Behavior notes (port `splitDiff`)

- Context lines appear on both sides of a `SideRow`.
- A run of `-` (deletes) is paired against the following run of `+` (adds): row `k` pairs
  `dels[k]` with `adds[k]`; when the runs are unequal length the shorter side gets `Empty` padding
  cells (`cell_style(None)`). This is the exact pairing in `splitDiff`.
- `unified` simply maps each `DiffLine` to one `UnifiedRow` in order (mobile stacked view).
- Empty input → empty output. A diff that is all-context, all-add, or all-del must round correctly.

## Test buckets (`tests/`)

- `happy.rs` — mixed diff → expected side-by-side rows; unified order; `cell_style` for each `Sig`.
- `errors.rs` — (no error type; this file covers degenerate inputs) empty slice; single line of each
  kind.
- `branches.rs` — ragged del>add and add>del pairings; `Empty` padding cells; context interleaved
  between change blocks; `cell_style(None)`.

## Edit scope

Code-agent: write `crates/marsad-diff/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
