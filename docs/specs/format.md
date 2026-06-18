# B05 — `marsad-format`

**Tier:** L0. **Mode:** one-shot. **Coverage:** 100% line+region.

## Purpose

Display formatters and presentation metadata — a direct port of the helper functions in the
`.dc.html` files (`fmtTokens`, `fmtElapsed`, `stateMeta`, `ctxColor`, `gateMeta`). These produce the
labels, colors, and summaries the UIs render. No IO.

## Public API (authoritative)

```rust
use marsad_gates::Gate;

pub fn fmt_tokens(n: u64) -> String;        // 1234 -> "1.2k"; 84200 -> "84.2k"; strips trailing ".0"; <1000 -> plain
pub fn fmt_elapsed(secs: u64) -> String;    // "2m 03s"; >=60m -> "2h 5m"

pub enum Phase { Starting, Working, Needs, Error, Done, Stale, Detached }  // mirrors events::Phase by value
pub struct Meta { pub label: &'static str, pub color: Token, pub bg: Token }
pub fn state_meta(phase: Phase) -> Meta;    // label/color/bg per state (port stateMeta)

pub enum Token { /* design token name, e.g. Accent, Needs, Error, Done, Neutral, Working, ... */ }
pub fn ctx_color(pct: f32) -> Token;        // >=90 Error; >=75 Needs; else Accent  (NOTE exact thresholds below)

pub struct GateMeta { pub kind: &'static str, pub icon: &'static str, pub lead: &'static str, pub summary: String, pub approve_label: &'static str }
pub fn gate_meta(gate: &Gate) -> GateMeta;  // port gateMeta for permission/plan/edit
```

Derive `Debug, Clone, PartialEq` (+ serde) where applicable.

## Behavior notes (match the `.dc.html` exactly)

- `fmt_tokens`: `n>=1000` → `(n/1000)` with one decimal, trailing `.0` stripped, suffix `k`; else the
  integer as a string.
- `fmt_elapsed`: minutes+seconds, zero-padded seconds; at `>=60` minutes switch to `Hh Mm`.
- `ctx_color`: the desktop uses `pct>=75 ? needs : (pct>=90 ? error : accent)` — note the desktop's
  literal ordering means 90+ still resolves via the 75 branch first; **the correct intended behavior
  is 90+ → Error, 75–89 → Needs, else Accent** (fix the design's ordering bug; the spec is
  authoritative over the prototype here). Mobile `ctxColor` only had the 75 threshold.
- `state_meta`: provide a `Meta` for every `Phase`. Tokens: Starting→Neutral, Working→Accent,
  Needs→Needs, Error→Error, Done→Done, **Stale→Neutral**, **Detached→Accent2**. Labels mirror the
  prototype ("Working", "Needs you", "Error", "Done", "Starting") plus "Stale" and "Detached".
- `gate_meta`: permission → kind "Permission", icon "⌘", lead "Wants to run", summary `"$ "+command`,
  approve "Approve & run"; plan → "Plan review"/"❑"/"Proposed plan"/`"N steps · <first title>…"`/
  "Approve plan"; edit → "File change"/"±"/"Wants to edit"/`"<basename> · +A −D"`/"Approve change".
  Empty-plan edge case: when `steps` is empty the plan summary is exactly `"0 steps"` (no `· …`
  suffix, no index into a missing first step).

## Test buckets (`tests/`)

- `happy.rs` — representative `fmt_tokens`/`fmt_elapsed`; `gate_meta` for each gate.
- `errors.rs` — degenerate inputs: 0 tokens, 0 secs, exactly 1000, exactly 3600s.
- `branches.rs` — `.0` strip vs not; `<1000`; `>=60m`; `ctx_color` at 74/75/89/90/100; every `Phase`
  in `state_meta`; each `Gate` variant in `gate_meta` incl. empty plan steps.

## Edit scope

Code-agent: write `crates/marsad-format/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
