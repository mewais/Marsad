# B11 — `marsad-viewmodel`

**Tier:** L0. **Mode:** one-shot. **Coverage:** 100% line+region.

## Purpose

Pure dashboard aggregation — the Rust port of `renderVals()` from the `.dc.html` files. Given an
immutable `AppSnapshot`, produces the view-models the desktop and phones render: overview counts and
groups, sidebar list (filter + search + state sort), session detail, gate VM, settings VM, workspaces
VM, and slash-command parsing/dispatch. No IO, no state.

## Public API (authoritative)

```rust
use marsad_events::{SessionState, Phase};
use marsad_format::Token;

pub struct AppSnapshot {
    pub sessions: Vec<SessionView>,        // id, name, host_id, project, branch, model, state(SessionState), changed_files, subagents
    pub hosts: Vec<marsad_hosts::Host>,
    pub workspaces: Vec<Workspace>,
    pub active_ws: Option<String>,
    pub selected_id: Option<String>,
    pub filter: Filter,                    // All | Needs | Working | Error
    pub search: String,
    pub tab: Tab,                          // Activity | Changes | Config | Usage | Terminal
}

pub enum Filter { All, Needs, Working, Error }
pub enum Tab { Activity, Changes, Config, Usage, Terminal }

pub struct Counts { pub total: u32, pub needs: u32, pub errors: u32, pub working: u32, pub cost: String, pub tokens: String, pub host_count: u32 }
pub fn counts(s: &AppSnapshot) -> Counts;

pub struct OverviewGroups { pub attention: Vec<SessionCard>, pub active: Vec<SessionCard>, pub idle: Vec<SessionCard> }
pub fn group_overview(s: &AppSnapshot) -> OverviewGroups;   // attention = needs+error (needs first); active = working+starting; idle = done

pub fn sidebar_list(s: &AppSnapshot) -> Vec<SessionRow>;    // apply filter, then search, then sort by {needs,error,working,starting,done}

pub fn rescope(workspace_id: &str, s: &AppSnapshot) -> AppSnapshot;  // restrict to a workspace's session_ids

pub struct SessionDetailVM { /* header, host/branch/model badges, tab flags, ctx warn>=75, usage labels */ }
pub fn session_detail(s: &AppSnapshot) -> Option<SessionDetailVM>;

pub struct GateVM { /* kind/icon/lead/summary + permission|plan|edit payload + diff rows for edit */ }
pub fn gate_vm(s: &AppSnapshot) -> Option<GateVM>;

pub enum SlashCmd { Clear, Compact, Cost, Model, Config, Init, Review, Agents, Resume, Terminal, Help, Unknown(String) }
pub fn parse_slash(input: &str) -> SlashCmd;
pub fn slash_catalog(query: &str) -> Vec<SlashEntry>;       // filtered popover list (prefix match), top item first
```

Derive `Debug, Clone, PartialEq` (+ serde).

## Behavior notes (match `renderVals()` exactly)

- Counts: needs/errors/working tallies; `cost` = `"$"+sum.to_fixed(2)`; `tokens` via
  `marsad_format::fmt_tokens(sum)`; `host_count` = distinct hosts.
- `group_overview`: attention = sessions in `needs` or `error`, sorted needs-before-error; active =
  `working` or `starting`; idle = `done`.
- `sidebar_list`: filter (`needs`/`working`/`error` restrict; `all` keeps), then case-insensitive
  search over `name+project+host_label+branch`, then sort by state order `{needs:0,error:1,working:2,
  starting:3,done:4}`.
- `rescope`: returns a snapshot whose `sessions` are restricted to the workspace's `session_ids`
  (re-scopes counts/lists/cost). Unknown workspace → unchanged snapshot.
- `session_detail`: `ctx_warn = context_pct >= 75`; usage labels via `marsad-format`.
- `gate_vm`: build from the selected session's gate via `marsad-format::gate_meta` + (for edit)
  `marsad-diff::split_side_by_side`.
- `parse_slash`: exact command match → variant; unknown → `Unknown`. `slash_catalog`: prefix filter
  over the 11-command list (`/clear /compact /cost /model /config /init /review /agents /resume
  /terminal /help`); empty query returns all; top match first (Enter runs it).

## Test buckets (`tests/`)

- `happy.rs` — counts/groups/sidebar/detail/gate_vm on a representative snapshot.
- `errors.rs` — empty snapshot; unknown workspace in `rescope`; no selection in detail/gate_vm.
- `branches.rs` — each `Filter`; empty search vs match vs no-match; full state-sort ordering; each
  `SlashCmd` + `Unknown`; `slash_catalog` empty/prefix/no-match; ctx warn boundary 74/75.

## Edit scope

Code-agent: write `crates/marsad-viewmodel/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
