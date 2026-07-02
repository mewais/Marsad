# U1 — Dynamic capability discovery: `Capabilities` per (driver, host), never preset

**Blocks touched:** B01 `marsad-proto` (domain type + 3 additive `IpcMsg` variants), B13
`marsad-agent` (capability provider + persistence + broadcast), B12 `marsad-core` (reduce arm +
`VmUpdate` arm + refresh method), B02-adjacent `marsad-viewmodel` (view type; deletion of the
preset slash catalog), desktop `src-tauri` (command + envelope arm). UI *consumption* is U2 — its
contract is pinned in §5 so U2 has no design latitude to reintroduce presets.

**Depends on:** M2 (typed IPC — all new messages are typed `IpcMsg` variants per
[`typed-ipc.md`](./typed-ipc.md) conventions). **Feeds:** U2 (UI), U5 (responsiveness — its rule is
folded in normatively, §4), M7b/M7c′ (drivers reuse the probes' fixtures).

**Owner mandate (verbatim requirements):** slash commands, available models, and resumable past
sessions must be discovered FROM each client CLI dynamically — never preset lists in UI or bridge;
UI must never freeze on backend actions.

## Purpose

Today the "capabilities" of a client CLI are hardcoded in three places:

- **Preset model list** — `apps/desktop/src/bridge/models.ts` (`MODELS`, `DEFAULT_MODEL`,
  `modelLabel`), consumed by `NewSession.tsx:8,73,503`, `Settings.tsx:16,1160,1194`,
  `screens/tabs/Config.tsx:7,59,73`.
- **Preset slash catalog** — `apps/desktop/src/bridge/api.ts:581–593` (`SLASH_CATALOG`) with a
  fallback arm in `filterSlash` (`api.ts:605–615`), mirrored by
  `crates/marsad-viewmodel/src/lib.rs:545–608` (`all_slash_entries` + `slash_catalog`). The live
  per-session list ALREADY flows (claude `system:init` → `marsad-events` reduce at
  `crates/marsad-events/src/lib.rs:255–259` → `SessionState.slash_commands` →
  `SessionView.slash_commands`, `crates/marsad-viewmodel/src/lib.rs:54`) — the presets exist only
  as a fallback that must die.
- **Client knowledge in the bridge** — `apps/desktop/src-tauri/src/bridge.rs:127–164`
  (`list_sessions` Tauri command) hardcodes claude's on-disk layout (`~/.claude/projects`,
  bridge.rs:149) and parses claude's remote listing format (`parse_remote_session_list`,
  bridge.rs:170) in the GUI process.

After U1: a **capability provider in the agent daemon** discovers models and resumable sessions
per (driver, host) from the real CLIs/on-disk stores, caches + persists them, and broadcasts them
as typed `CapabilitiesUpdate` messages → `VmUpdate::Capabilities` → `vm-update` events. Slash
commands stay session-scoped (already dynamic) and the UI consumes ONLY that. No component
below the CLI itself ships a list of models, commands, or sessions.

All driver facts below are grounded in the verified research docs:
[`codex-research.md`](./codex-research.md) (R2, live-verified against codex-cli 0.142.5) and
[`antigravity-research.md`](./antigravity-research.md) (agy 1.0.15, binary/on-disk evidence,
zero live calls). Anything those docs flag as unverified is treated as unverified here too.

---

## 1. The `Capabilities` domain type (normative)

Lives in **`marsad-proto`** — it is wire schema, same DAG position as `SlashCommandInfo`
(`crates/marsad-proto/src/lib.rs:93`). `driver` is the plain wire string (`"claude"` | `"codex"` |
`"gemini"`), not the `marsad_workstream::Driver` enum (proto must not depend upward; the
`"gemini"`→antigravity rename question is M7e/R1's, unchanged here).

```rust
/// One selectable model, exactly as the driver reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The exact string passed to the CLI's model flag (`--model` / `-m`). Verbatim,
    /// never normalized: agy model names contain spaces/parens ("Gemini 3.5 Flash (Low)").
    pub id: String,
    /// Driver-reported qualifier, verbatim (e.g. codex catalog priority/flagship info).
    /// NEVER synthesized by Marsad — no hand-written "Most capable" sublabels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
}

/// Provenance of a model catalog — drives the UI's "unverified list" treatment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModelListSource {
    /// The CLI enumerated it (codex `debug models`, `agy models`).
    Listed,
    /// Inferred from observation (claude: models seen in `system:init` + persisted
    /// last-known). UI MUST label this "unverified" and MUST offer free-text entry.
    Observed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCatalog {
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    pub source: ModelListSource,
    /// UI must additionally offer a free-text model field. true for claude (always)
    /// and codex (account catalog may lag); false for agy (the CLI accepts exactly
    /// the `agy models` strings — an unknown model is a designated error case,
    /// antigravity-research §6#4).
    #[serde(default)]
    pub free_text: bool,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        // The honest empty state: nothing observed yet, free text allowed.
        ModelCatalog { models: vec![], source: ModelListSource::Observed, free_text: true }
    }
}

/// One resumable past session discovered on the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredSession {
    /// Driver-native resume id (claude UUID stem / codex thread uuid / agy conversation uuid).
    pub id: String,
    /// Project/workspace directory. None where the store doesn't record it
    /// retrievably (agy: only the most-recent conversation per workspace is mapped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// First user prompt, trimmed + truncated (~100 chars), driver-appropriate parse.
    #[serde(default)]
    pub snippet: String,
    /// Last-activity mtime, Unix epoch milliseconds.
    #[serde(default)]
    pub modified_ms: u64,
}

/// What the session scan covered — claude's on-disk project encoding is lossy
/// (`encode_project_dir`, crates/marsad-core/src/discovery.rs:205 — `/` and `_` both
/// map to `-`), so claude sessions can only be enumerated per REQUESTED project;
/// codex/agy stores are globally enumerable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionScope {
    /// Everything the driver's store contains (codex, agy).
    AllKnown,
    /// Only these requested project paths (claude). Grows as projects are requested;
    /// persisted so the cache replays them.
    Projects(Vec<String>),
}

/// Discovered capabilities for one (driver, host) pair. The unit of caching,
/// persistence, wire transfer, and VmUpdate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub driver: String,
    pub host_id: String,
    /// Whether the driver's binary was found/runnable on the host. Discovered
    /// (`command -v`/probe), never assumed. M7d's driver picker gains its
    /// ground truth here later.
    #[serde(default)]
    pub cli_available: bool,
    #[serde(default)]
    pub models: ModelCatalog,
    #[serde(default)]
    pub session_scope: Option<SessionScope>,
    #[serde(default)]
    pub sessions: Vec<DiscoveredSession>,
    /// When the last successful probe finished; 0 = never (persisted-cache only /
    /// first run). UI derives staleness from this — no boolean baked in.
    #[serde(default)]
    pub fetched_at_ms: u64,
    /// Per-probe failures, verbatim, surfaced not swallowed ("agy models: exit 1: …").
    #[serde(default)]
    pub errors: Vec<String>,
}
```

Serde conventions per typed-ipc.md §2: no `deny_unknown_fields` anywhere; all additive fields
`#[serde(default)]`.

### 1.1 Models, per driver

| driver | mechanism | source | free_text | evidence |
|---|---|---|---|---|
| claude | **No list command exists.** Fallback strategy (normative): union of (a) model ids observed in `system:init` events on that host — the agent's session readers already inspect stream lines (gate detection); they additionally extract `init.model`, which `marsad-events` also reduces per session (`events/src/lib.rs:255`) — accumulated across ALL sessions ever run on the host, and (b) the persisted last-known set (§3.4). First run = empty list + free text; that IS the correct behavior, never seeded. | `Observed` — UI flags "unverified list" | **true** (mandatory) | absence verified during R-series; init flow already in tree |
| codex | Parse `codex debug models` — local, no network, reads the cached model catalog (`~/.codex/models_cache.json` exists as a secondary artifact). Lists per-account models with priority + flagship/hidden flags; **hidden models (e.g. `codex-auto-review`) are excluded**. Exact stdout format is NOT captured in the research doc — **capture the real output as a fixture before writing the parser** (house guardrail: no normalizer from docs alone). | `Listed` | true (account catalog may lag) | codex-research §"Model selection", §d |
| agy | Parse `agy models` — static/cached list, zero cost, one model name per line, verbatim (8 lines captured in the research doc, including spaces/parens/mixed vendors). Pass ids unmodified to `--model`. | `Listed` | false | antigravity-research §0 (verbatim output) |

### 1.2 Resumable sessions, per driver

| driver | store | id | project | snippet | resume with |
|---|---|---|---|---|---|
| claude (local) | `~/.claude/projects/<encoded>/<uuid>.jsonl` (`$CLAUDE_CONFIG_DIR` honored, core:236–246) — **reuse `discovery::{list_sessions, first_user_prompt}`** (`crates/marsad-core/src/discovery.rs:574,243`); scope = requested project (encoding lossy, §1) | filename stem | requested project | `first_user_prompt` | `--resume <id>` (existing launch path) |
| claude (remote) | existing agent SSH one-shot `fetch_remote_session_list` (`crates/marsad-agent/src/lib.rs:2199`); the tab-record parse moves from `src-tauri/bridge.rs:170` INTO the agent provider (the bridge must not know claude's listing format) | ditto | ditto | ditto | ditto |
| codex | scan `~/.codex/sessions/<Y>/<M>/<D>/rollout-<ts>-<uuid>.jsonl` newest-first, capped (default 200 files) | trailing uuid of filename | first line `session_meta.payload.cwd` | first `event_msg`/`response_item` user message text | `codex exec resume <id>` / app-server `thread/resume` (M7b) |
| agy | enumerate `~/.gemini/antigravity-cli/brain/<uuid>/` dirs; snippet from `brain/<id>/.system_generated/logs/transcript_full.jsonl` first `USER_INPUT` step, **stripping the `<USER_REQUEST>…</USER_REQUEST>` wrapper** (and sibling injected blocks); prefer `transcript_full.jsonl` over `transcript.jsonl` (double-escaping trap) | dir name uuid | reverse-map `cache/last_conversations.json` (workspace → most-recent id) where it hits; otherwise `None` (the `.db` is raw protobuf — not parsed) | as left | `--conversation <id>` |

codex evidence: codex-research §d (paths, `session_meta`/`event_msg` line schema, append-on-resume
all live-verified). agy evidence: antigravity-research §3.2/§4/§5 (real files on disk). The codex
app-server `thread/list` method exists but whether it reads sqlite or rescans is unverified
(codex-research §f7) — the file scan is v1; a protocol-backed lister can replace it under M7b
without changing this wire contract.

**Remote hosts for codex/agy:** the same file scans run through `run_remote_script_bounded` when
those drivers become remote-capable; until then a remote (codex|agy, host) probe returns
`cli_available` honestly and `errors: ["remote discovery not implemented for <driver>"]` — an
explicit gap, never a fabricated list.

### 1.3 Slash commands — session-scoped, already dynamic; presets die

Slash commands are **deliberately NOT in `Capabilities`**: claude reports them per session in
`system:init` (`slash_commands`, string-or-object tolerant parse at
`crates/marsad-proto/src/lib.rs:101–158`), the reducer stores them
(`events/src/lib.rs:259`), and `SessionView.slash_commands` already reaches the UI. The complete
chain exists; U1 changes nothing on this wire.

What U1/U2 pin instead — the consumption contract:

- The Composer/Activity slash menu's ONLY source is `sv.slash_commands`
  (`Composer.tsx:72`, `Activity.tsx:553–555` today pass it but fall back to the catalog).
- Empty list (pre-init, or a driver that reports none) → **empty menu with a "waiting for the
  session to report commands" hint**. The empty state is first-class UX, never backfilled from a
  catalog. codex/agy expose no command-listing surface in the research docs; their sessions report
  none until their drivers (M7b/M7c′) surface an equivalent into the same `SessionState` field.
- Deleted: `SLASH_CATALOG` + the fallback arm of `filterSlash` (`api.ts:581–615`);
  `all_slash_entries` + `slash_catalog` (`viewmodel/src/lib.rs:545–608` — dead after the UI
  change; house rule: unreachable code is deleted, not waived).

---

## 2. Wire protocol — additive `IpcMsg` variants (typed-ipc.md conventions)

Three new tags; `parse_ipc`'s `KNOWN` list grows to exactly these + the M2 set. All fields obey
§2 of typed-ipc.md (internally tagged, PascalCase tag strings, `#[serde(default)]` tolerance,
additive-only).

```rust
// ── GUI/core → agent ──
/// Ask the agent to (re)probe capabilities. Fire-and-forget: the reply is the ack
/// below; results arrive later as CapabilitiesUpdate broadcasts. None = all.
RefreshCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    driver: Option<String>,
    /// claude sessions need a project scope (§1.2); adds it to the host's
    /// SessionScope::Projects set. Ignored by AllKnown drivers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
},

// ── agent → GUI/core ──
/// Serve-loop ack (house convention: every command line gets a typed reply).
CapabilitiesRefreshQueued {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
},
/// One (driver, host) snapshot — broadcast to all subscribers on probe completion
/// AND replayed from cache to each new subscriber. Always a COMPLETE snapshot
/// (replace, not merge, on the receiving side).
CapabilitiesUpdate {
    host_id: String,
    driver: String,
    /// Typed — Capabilities lives in proto (§1), unlike ChangesUpdate's Value
    /// (whose FileChange lives in viewmodel; no such DAG constraint here).
    capabilities: Capabilities,
},
```

Golden wire forms (normative; key order non-normative per typed-ipc.md §2):

```json
{"type":"RefreshCapabilities"}
{"type":"RefreshCapabilities","host_id":"local","driver":"claude","project":"/work/mewais/OpenSource/Marsad"}
{"type":"CapabilitiesRefreshQueued","host_id":"local"}
{"type":"CapabilitiesUpdate","host_id":"local","driver":"gemini","capabilities":{"driver":"gemini","host_id":"local","cli_available":true,"models":{"models":[{"id":"Gemini 3.5 Flash (Low)"}],"source":"Listed","free_text":false},"session_scope":"AllKnown","sessions":[{"id":"97197d46-2091-40e2-b36f-0dcedd34bcc4","project":"/work/mewais/OpenSource/Marsad","snippet":"x","modified_ms":1783100616000}],"fetched_at_ms":1783100620000,"errors":[]}}
{"type":"CapabilitiesUpdate","host_id":"hpc","driver":"claude","capabilities":{"driver":"claude","host_id":"hpc","cli_available":true,"models":{"models":[{"id":"claude-opus-4-8"}],"source":"Observed","free_text":true},"session_scope":{"Projects":["/home/u/proj"]},"sessions":[],"fetched_at_ms":0,"errors":[]}}
```

Compat matrix (additive proof): new core → old agent: `RefreshCapabilities` hits the unknown-type
arm → fire-and-forget `Error` reply, harmless. New agent → old core: `CapabilitiesUpdate` /
`CapabilitiesRefreshQueued` hit core's tolerant-skip (typed-ipc.md §4) → logged, `Ok(vec![])`.
Old ↔ old unchanged. No existing variant is touched.

---

## 3. Placement: capability provider in the AGENT daemon

The agent owns hosts, processes, and SSH connections (serve loop `agent/src/lib.rs:1330`,
subscriber registry `:1392`, remote exec plumbing `:2199,2217ff`), survives GUI restarts, and
already broadcasts to subscribers — capabilities are host facts, so they are agent facts. Core
and the desktop only mirror.

### 3.1 Provider seam

New module `crates/marsad-agent/src/capabilities.rs` (consistent with the M4 split; if M4 hasn't
landed, a module in `lib.rs` with the same seam).

```rust
/// One driver's discovery logic. Implementations are BLOCKING and are always
/// invoked on a dedicated spawned thread (§4) — never the serve loop, never
/// under any agent mutex.
pub trait CapabilityProbe: Send + Sync + 'static {
    fn driver(&self) -> &'static str;
    /// Full probe for one host: availability + models + sessions for the given
    /// scope. Must respect `ctx.deadline` internally; errors go into
    /// `Capabilities::errors`, never panic/propagate.
    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities;
}

pub struct HostProbeCtx {
    pub host_id: String,
    /// None = local; Some = SSH/WSL target (exec via run_remote_script_bounded).
    pub host_target: Option<String>,
    /// Session scope to scan (claude: accumulated requested projects).
    pub scope: SessionScope,
    /// Hard wall-clock budget (default 6s — matches the existing bounded-probe
    /// convention, agent/src/lib.rs:1440–1490).
    pub deadline: std::time::Duration,
}
```

**Pure parse functions are separated from IO** so tests need no CLI: `parse_agy_models(&str)`,
`parse_codex_debug_models(&str)`, `parse_codex_rollout_meta(filename, first_line)`,
`agy_snippet_from_transcript(&str)`, `parse_remote_claude_session_list(&str)` (moved from
bridge.rs:170), plus reuse of `marsad_core::discovery::{list_sessions, first_user_prompt}`.
Real subprocess/FS-rooted-at-`$HOME` paths follow the established `RealGit`/`RealTranscript`
pattern (`#[cfg(not(coverage))]`, core:154–159, 235–259): compiled out under coverage, proven by
the gated live tests (§6), with roots parameterized (`&Path`) wherever a temp-dir test can reach
them.

### 3.2 Refresh triggers

1. **Daemon start:** load persisted cache (§3.4), then spawn a full refresh (all drivers × all
   known hosts) in the background. Serving starts immediately; nothing waits on probes.
2. **New subscriber (`Subscribe`):** immediately replay every cached `CapabilitiesUpdate` line to
   that subscriber (same pattern as transcript backfill), then queue a refresh for entries older
   than TTL.
3. **On demand (`RefreshCapabilities`):** ack `CapabilitiesRefreshQueued`, queue matching probes.
4. **Passive claude-model observation:** when a session reader sees a `system:init` line, extract
   `model`; if new for that host, update the observed set, persist, and broadcast an updated
   `CapabilitiesUpdate`. Likewise a `SessionStopped` marks that host's session list stale (a new
   resumable just appeared) and queues a sessions refresh.

**In-flight dedup:** a `(driver, host)` probe already running ⇒ skip queuing another (the imminent
broadcast serves the requester). Tracked in a mutex-guarded set, held only for insert/remove.

### 3.3 Staleness policy

- Model catalogs: TTL **1 hour** (catalogs change rarely; `agy models` / `codex debug models` are
  free but there's no reason to spam them).
- Session lists: TTL **30 seconds** (sessions appear constantly), refreshed eagerly by trigger 3
  when a picker opens (U2 fires `refresh_capabilities` on open — and renders the cached list
  instantly regardless).
- The cache is ALWAYS served instantly; TTL only decides whether a background re-probe is queued.
  `fetched_at_ms` rides the wire so the UI can render "as of …" honestly; a persisted-only entry
  has `fetched_at_ms` from its last real probe (or 0 if none ever succeeded).

### 3.4 Persistence

`<data_root>/capabilities/<host_id>.json` — one file per host holding every driver's last
`Capabilities` plus the claude observed-models set (a superset store: observed ids survive even if
a later snapshot narrows). Written after every successful probe and every observed-model addition;
loaded at daemon start. Corrupt/missing file ⇒ start empty (never an error — same disposition as
`unwrap_or_default` elsewhere). This is what makes claude's "persisted last-known" leg of the
fallback strategy real across daemon restarts.

### 3.5 Core changes (`marsad-core`)

- State: `capabilities: HashMap<(String /*driver*/, String /*host_id*/), Capabilities>`.
- `Core::refresh_capabilities(host_id: Option<String>, driver: Option<String>, project:
  Option<String>)` → `send_ipc(&IpcMsg::RefreshCapabilities{…})` — a single line write, same cost
  class as `interrupt()`; never waits for a reply.
- `Core::capabilities()` → cloned `Vec<CapabilitiesView>` snapshot accessor (lock-only, no IO).
- `on_agent_line` arms: `CapabilitiesUpdate { host_id, driver, capabilities }` → upsert
  (whole-value **replace** — the agent sends complete snapshots, §2) → `Ok(vec![
  VmUpdate::Capabilities(view)])`. `CapabilitiesRefreshQueued` → `Ok(vec![])` (ack class, joins
  the existing ack no-op arm). Missing `host_id`/`driver` is a serde `SchemaMismatch` →
  `CoreError::Protocol`, consistent with the M2 handler contract.
- New `VmUpdate` arm (`crates/marsad-core/src/lib.rs:80`):
  `Capabilities(marsad_viewmodel::CapabilitiesView)`.

### 3.6 Viewmodel + desktop plumbing

- `marsad-viewmodel`: `CapabilitiesView` — display-ready mirror of `Capabilities` (ids verbatim;
  **no label synthesis** — `modelLabel()` has no successor). Pure `From<Capabilities>` mapping.
- `src-tauri/events.rs` `VmUpdateEnvelope` gains
  `Capabilities { capabilities: CapabilitiesView }` (snake_case tag `capabilities`, matching the
  existing envelope convention).
- New Tauri commands (async, `rename_all = "snake_case"`, per the bridge.rs:122–145 lock
  discipline — guard taken and dropped before any await):
  - `capabilities() -> Vec<CapabilitiesView>` — lock, clone, return (initial snapshot for `useVm`,
    same pattern as `board`).
  - `refresh_capabilities(host_id?, driver?, project?)` — forwards to
    `Core::refresh_capabilities`, returns immediately.
- TS: `VmUpdateEvent` union + `useVm` store gain a `capabilities` slice keyed
  `(driver, host_id)`; `api.ts` gains `fetchCapabilities()` / `refreshCapabilities()` with the
  usual mock-mode branch.
- The existing `list_sessions` Tauri command + `parse_remote_session_list`
  (bridge.rs:127–190) stay untouched in U1 (no UI break) and are **deleted in U2** when
  `NewSession.tsx:124` switches to VM data; the parse helper's logic moves agent-side now (§3.1)
  so U2's deletion is pure removal.

---

## 4. U5 responsiveness rule (folded in, normative)

**Capability refresh must never block anything.** Concretely:

1. **Agent:** no probe IO on the serve-loop thread, a reader thread, or while any agent mutex is
   held. Pattern = the existing bounded-probe idiom (agent/src/lib.rs:1440–1490): one
   `std::thread` per queued `(driver, host)` probe; remote exec via `run_remote_script_bounded`
   (hard 6s timeout), local probes honor `ctx.deadline` internally; the result is sent over an
   `mpsc` channel to the broadcast path (subscribers `Arc`, agent:1392). A probe that hangs past
   deadline yields `errors: ["<probe> timed out"]` — it cannot wedge the daemon.
2. **Core:** `refresh_capabilities` writes one IPC line and returns; the `CapabilitiesUpdate`
   reduce arm is a pure in-memory upsert. No new IO is introduced under Core's state lock.
3. **Desktop:** `capabilities` is lock-clone-return; `refresh_capabilities` returns before any
   probe runs; no state-lock guard is held across an `await` (bridge.rs:122–125 comment is the
   binding precedent).

**The proving tests** (spawn + broadcast, asserted, not asserted-by-comment — full list §6):

- *Agent-level:* inject a fake `CapabilityProbe` that sleeps 2s. Send `RefreshCapabilities`, then
  immediately drive an unrelated command round-trip (`Subscribe` → `Subscribed`) and assert it
  completes in < 100ms wall clock; assert the `CapabilitiesUpdate` broadcast arrives afterwards.
- *Core-level:* call `refresh_capabilities(...)` on a Core with a fake transport, then immediately
  call `board()` / `capabilities()` and assert both return current state without touching the
  transport reply path (there is nothing to wait ON — pinned so a future "wait for fresh data"
  regression cannot land silently).
- *Desktop-level:* the U5 audit's deliberately-slow-fake-transport test (PLAN.md U5) covers the
  Tauri command layer; `capabilities`/`refresh_capabilities` join its command matrix.

---

## 5. UI consumption contract (U2 preview) + grep gate

U2 implements this; U1 fixes the contract so no preset can sneak back:

- **NewSession model picker** (`NewSession.tsx`): options = `capabilities[(driver, host)]`
  `.models` from the `useVm` store for the *selected* host + driver — never an import.
  `source == Observed` → render an "unverified — models seen in past sessions" badge and always
  show the free-text model input; empty catalog → free-text input only. Default selection =
  persisted last-used model from settings if present, else the first catalog entry, else empty —
  **no `DEFAULT_MODEL` constant anywhere**.
- **Resume picker** (`NewSession.tsx:124` today calls `listSessions`): reads
  `capabilities[…].sessions` filtered by chosen project (agy `project: None` entries render under
  "unknown project"); fires `refreshCapabilities({host_id, driver, project})` on open and renders
  the cached list instantly with its `fetched_at_ms` ("as of …") — stale-then-fresh, never
  spinner-blocked.
- **Composer/Activity slash menu:** `sv.slash_commands` only; empty ⇒ hint state (§1.3).
- **Settings/Config model dropdowns** (`Settings.tsx:1160`, `Config.tsx:59`): same VM source
  (union across hosts for the global default picker) + free text.

**Grep gate** (CI script; part of U2's done-gate, defined here):

```sh
# 1. No preset catalogs or the models module anywhere in UI/bridge/viewmodel:
git grep -nE 'SLASH_CATALOG|bridge/models|all_slash_entries|slash_catalog' \
  -- apps/desktop/src crates/marsad-viewmodel/src            # MUST be empty
# 2. No hardcoded claude model ids in the UI or the Tauri bridge:
git grep -nE 'claude-(opus|sonnet|haiku)-[0-9]' \
  -- apps/desktop/src apps/desktop/src-tauri/src             # MUST be empty
# 3. No client on-disk knowledge left in the bridge:
git grep -n '\.claude/projects' -- apps/desktop/src-tauri/src  # MUST be empty
```

Model-id literals remain legal ONLY in `crates/marsad-core/src/discovery.rs` (the litellm/hand
pricing table — U4 keeps it for historical backfill, labeled "estimated") and in
tests/fixtures.

**Kill list (the preset artifacts that must be gone after U2):**
`apps/desktop/src/bridge/models.ts` (whole file), `api.ts:581–615` (`SLASH_CATALOG` +
`filterSlash` fallback arm), `crates/marsad-viewmodel/src/lib.rs:545–608` (`SlashEntry` catalog
fns; the `SlashEntry` type itself dies with them if nothing else uses it),
`src-tauri/bridge.rs:127–190` (`list_sessions` + `parse_remote_session_list`), and the preset
imports/usages at `NewSession.tsx:8,73,503`, `Settings.tsx:16,1160,1194`, `Config.tsx:7,59,73`,
`Composer.tsx:25–26,71–72` and `Activity.tsx:553` fallback comments/branches.

---

## 6. Test plan (house style: black-box `tests/`, per TESTPLAN.md)

### marsad-proto (`tests/happy.rs`, `errors.rs`, `branches.rs`) — bar: 100% lines+regions

For each of the 3 new variants, the M2 four-step pattern (typed-ipc.md §6): golden parse (§2
goldens as `r#"…"#` literals → full typed equality), encode→parse identity + re-encode stability,
golden encode compared as `serde_json::Value`, and `x_future_field` injection tolerance. Plus:
minimal `{"type":"RefreshCapabilities"}` parses to all-None; `Capabilities` with absent
`models`/`sessions`/`session_scope`/`fetched_at_ms`/`errors` parses to defaults
(`ModelCatalog::default()` = empty/Observed/free-text); missing `host_id` on `CapabilitiesUpdate`
→ `SchemaMismatch`; unknown `ModelListSource` value → `SchemaMismatch`; `SessionScope` both arms
round-trip.

### marsad-agent (`tests/` additions) — bar: 100% lines+regions (real-IO `#[cfg(not(coverage))]`, live-proven)

**Pure providers, fixtures = the research docs' captured shapes** (checked into `fixtures/`):

- `parse_agy_models`: the verbatim 8-line `agy models` output (antigravity-research §0) → 8
  `ModelInfo`s with ids verbatim ("Claude Opus 4.6 (Thinking)" etc.); trailing
  whitespace/blank-line tolerance; empty stdout → empty + no error.
- `parse_codex_debug_models`: **capture the real stdout first** (free, local — house guardrail
  from phase2-drivers: never implement a normalizer from docs alone); assert hidden models
  excluded, priority text lands in `annotation` verbatim.
- codex rollout scan: temp-dir tree `sessions/2026/07/02/rollout-2026-07-02T11-38-07-<uuid>.jsonl`
  with the verbatim `session_meta` first-line shape (codex-research §d) → id from filename, project
  from `payload.cwd`, snippet from the user message, newest-first ordering, file cap honored;
  malformed first line → session listed with empty project/snippet (skip, don't fail).
- `agy_snippet_from_transcript`: the verbatim 5-line `transcript_full.jsonl` fixture
  (antigravity-research §3.2) → snippet `"x"` with `<USER_REQUEST>`/metadata wrappers stripped;
  brain-dir temp-tree scan + `last_conversations.json` reverse-map (hit → `Some(project)`,
  miss → `None`).
- `parse_remote_claude_session_list`: the existing `mtime\tid\tuser-line` records (moved logic;
  port bridge's cases verbatim).
- claude observed-models: accumulate from init events, dedupe, survive a persistence round-trip
  (`capabilities/<host>.json` write→load), corrupt file → empty start.

**Agent IPC round-trips** (fake probe injected through the provider seam):

- `RefreshCapabilities` line in → `CapabilitiesRefreshQueued` ack + eventual `CapabilitiesUpdate`
  on the subscriber channel, parsed with `parse_ipc` and compared typed.
- New subscriber receives cached `CapabilitiesUpdate` replay before any live event.
- In-flight dedup: two rapid refreshes for the same (driver, host) → one probe execution.
- Failing probe → `Capabilities.errors` populated and broadcast (silence is a bug).
- Project-scope accumulation: two `RefreshCapabilities` with different `project`s →
  `SessionScope::Projects` contains both.
- **The slow-provider non-blocking test** (§4): 2s-sleep probe; concurrent `Subscribe` round-trip
  < 100ms; update arrives after.

### marsad-core (`tests/` additions) — bar: 100% lines+regions

`on_agent_line` + `CapabilitiesUpdate` golden → one `VmUpdate::Capabilities` with the mapped view;
second update for same key replaces (not merges); missing `host_id` → `CoreError::Protocol`;
`CapabilitiesRefreshQueued` inbound → `Ok(vec![])`; `refresh_capabilities()` emits the exact typed
line (asserted **through `parse_ipc`**, M2 style); `capabilities()` snapshot reflects upserts;
the §4 board()-unblocked test.

### marsad-viewmodel — bar: 100% lines+regions

`From<Capabilities> for CapabilitiesView` field mapping (incl. `project: None` passthrough).
Tests-that-change closed list: the `slash_catalog` suite is DELETED with the code (U2); no other
existing viewmodel test may change.

### Desktop TS (vitest — interaction matrix, not line-100%) + integration

- `filterSlash` with empty/absent session list → empty result (fallback gone).
- `useVm` reduces the `capabilities` envelope; `NewSession` picker renders from an injected VM
  snapshot: Listed catalog, Observed catalog with unverified badge + free-text path, empty
  catalog → free-text only; resume list renders from VM data with "as of" staleness.
- Playwright (M6 mock mode): slash menu empty-state visible pre-init.

### Live (feature-gated, token-budget rule)

`agy models` and `codex debug models` are zero-token local invocations — the live suite runs both
and asserts the parsers digest the real current outputs (fixture drift alarm). Claude needs no
extra spend: the existing i03 live launch already yields `system:init`; assert the observed model
subsequently appears in a `CapabilitiesUpdate`. `i09`/`i10` socket suites must pass unmodified.

---

## 7. Non-goals

- No driver launch/normalizer work (M7b/M7c′). Capabilities for codex/agy can exist while their
  drivers stay UI-hidden — M7d's rule ("no driver appears until its full chain is green") governs
  picker *enablement*; discovery is deliberately ahead of it.
- No slash-command execution change (`SlashCmd` forwarding is unchanged).
- No `"gemini"`→`"antigravity"` wire-string decision (M7e/R1).
- No deletion of the `list_sessions` Tauri command in U1 (U2, once the picker reads VM data).
- No cost/pricing changes (U4).

## Edit scope

proto: `crates/marsad-proto/{src,tests}/`. agent: `crates/marsad-agent/{src,tests}/` +
`fixtures/`. core: `crates/marsad-core/{src,tests}/`. viewmodel:
`crates/marsad-viewmodel/{src,tests}/`. desktop: `apps/desktop/src-tauri/src/{lib,bridge,events}.rs`,
`apps/desktop/src/bridge/*.ts` (U2 for screens/components). Per ARCHITECTURE.md's
directory-access contract.
