# B06 — `marsad-store` — Workstreams extension (additive spec)

**Additive only.** Do NOT change existing types, methods, key prefixes, `SCHEMA_VERSION`, or existing
tests. This spec adds persistence for the new workstream-domain entities and a pure migration helper.
Dep already wired: `marsad-workstream` (for `Workstream`, `Connection`, `Profile`).

Design source: `docs/design/DESIGN_HANDOFF.md` (§ Domain data model); `docs/specs/workstream.md` (the
entity shapes). Persist the `marsad_workstream` types directly (they already derive serde) — do NOT
define parallel `Persisted*` mirrors for them.

Existing conventions to reuse verbatim (read `crates/marsad-store/src/lib.rs`): the `Backend` trait,
`Versioned<T>` blob wrapper via `encode`/`decode_payload`, `blob_version` + `SCHEMA_VERSION` gating
(→ `StoreError::VersionMismatch` on mismatch, best-effort skip on corrupt like the existing entities),
key-prefix scheme, and deterministic sorted restore.

## New key prefixes
```rust
const PREFIX_WORKSTREAM: &str = "workstream:";
const PREFIX_CONNECTION: &str = "connection:";
const PREFIX_PROFILE: &str = "profile:";
```

## New `Store` methods (all return `Result<(), StoreError>` unless noted; mirror existing upsert/delete)
- `upsert_workstream(&mut self, w: &marsad_workstream::Workstream)` → key `workstream:{w.id}`, versioned blob.
- `delete_workstream(&mut self, id: &str)` → removes `workstream:{id}` (idempotent: deleting an absent id is `Ok`).
- `upsert_connection(&mut self, c: &marsad_workstream::Connection)` → key `connection:{c.id}`.
- `delete_connection(&mut self, id: &str)` → idempotent.
- `upsert_profile(&mut self, p: &marsad_workstream::Profile)` → key `profile:{p.id}`.
- `delete_profile(&mut self, id: &str)` → idempotent.

Delete idempotency + error propagation must match the existing `delete_session`/`delete_host` behavior
(observe them and follow the same pattern, including how a `Backend` op error surfaces as `StoreError`).

## `Restored` — three new fields (append; keep existing field order/names)
```rust
pub workstreams: Vec<marsad_workstream::Workstream>,
pub connections: Vec<marsad_workstream::Connection>,
pub profiles: Vec<marsad_workstream::Profile>,
```
`restore()` populates them by listing each prefix, decoding version-matched blobs (skipping corrupt /
version-mismatched entries exactly as the existing entity loads do), sorted **by id ascending** for
determinism. (Existing `Restored` fields and their load/sort behavior are unchanged.)

Note: `Restored` gains fields, so any code/tests constructing a `Restored` literal must add the three
new fields (empty vecs). This mechanical update to existing store tests is permitted and expected;
assertions must not otherwise change. Prefer updating a shared test helper if one exists.

## Pure migration helper (free function, no IO)
```rust
/// Legacy `Workspace` (a named bag of session ids) → a `Profile` (grouping lens).
/// Pure and deterministic: preserves input order; `Profile.id = Workspace.id`,
/// `Profile.name = Workspace.name`. Session→workstream migration is the caller's job (core).
pub fn profiles_from_workspaces(workspaces: &[Workspace]) -> Vec<marsad_workstream::Profile>;
```

## Test plan (test-agent: add NEW files only — do NOT edit existing test files except the mechanical
`Restored`-literal field additions described above). 100% line+region coverage of the new code required.

Add `tests/workstreams.rs`:
- WS1  `upsert_workstream` then `restore().workstreams` returns it; round-trips all fields incl. `profile: Some/None`.
- WS2  multiple workstreams restore **sorted by id ascending** regardless of insert order.
- WS3  `delete_workstream` removes it from a subsequent `restore()`; deleting an absent id is `Ok(())`.
- WS4  `upsert_connection`/`restore().connections` round-trips incl. `bi:true` and `gate: Some(Handoff{..})`; sorted by id.
- WS5  `delete_connection` removes; absent id `Ok(())`.
- WS6  `upsert_profile`/`restore().profiles` round-trips; sorted by id; `delete_profile` removes; absent id `Ok(())`.
- WS7  a corrupt or version-mismatched `workstream:`/`connection:`/`profile:` blob is skipped by `restore()`
       (not an error) — mirror the existing corrupt-skip test for sessions (use `MemoryBackend::corrupt`).
- WS8  a `Backend` write error on `upsert_*` surfaces as the same `StoreError` variant the existing
       `upsert_session` produces (use `MemoryBackend::fail_next(BackendOp::…)`).
- WS9  `profiles_from_workspaces`: empty → empty; N workspaces → N profiles preserving order, id/name mapped;
       independent of `saved_at`/`session_ids`.
- WS10 the three new entity kinds coexist with existing sessions/hosts/workspaces in one `restore()`
       (a mixed store restores all correctly — no cross-prefix contamination).

Coverage: exercise both the Ok and every error/skip branch of the new methods so the crate stays at
100% line+region. Do not leave any new branch untested; do not add code the tests don't reach.
