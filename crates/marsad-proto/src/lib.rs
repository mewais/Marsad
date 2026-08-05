//! marsad-proto — wire types + serde for Claude stream-json and the GUI↔agent IPC protocol.
//!
//! See `docs/specs/proto.md` for the authoritative contract.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// wasit-core re-exports
// ---------------------------------------------------------------------------
//
// The rich, canonical event model now lives in `wasit-core`. The agent→core
// wire carries `AgentEvent` directly (see `IpcMsg::AgentEvent`), and downstream
// Marsad crates match on the rich supporting types re-exported here.
//
// The rich supporting value-types are re-exported so downstream Marsad crates
// keep using the `marsad_proto::X` names while the definitions live in
// `wasit-core`. Wave 1 folded the old marsad-proto stream types away entirely,
// so there is no longer a name clash to avoid — `SessionMode`, `McpServerInfo`,
// `SlashCommandInfo`, `PlanEntry`, `ElicitationChoice`, `ModelUsage`, and
// `RateLimitInfo` are now re-exported under their own names. `CompactionInfo` is
// re-exported additionally under the alias `CompactMetadata` (its historical
// marsad name; the field shape is identical). `Usage` is re-exported as
// `WasitUsage` (its historical alias here).
pub use wasit_core::{
    AgentEvent, CompactionInfo, CompactionInfo as CompactMetadata, ElicitationChoice, EndReason,
    GateDecision, GateRequest, McpServerInfo, ModelUsage, PlanEntry, QuestionAnswer, QuestionOption,
    QuestionPrompt, RateLimitInfo, ResumeMode, SessionMode, SlashCommandInfo, SpawnSpec,
    Usage as WasitUsage,
};

// ---------------------------------------------------------------------------
// ProtoError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProtoError {
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
    #[error("missing type discriminant")]
    MissingDiscriminant,
    #[error("unknown discriminant: {0}")]
    UnknownDiscriminant(String),
    #[error("schema mismatch at field `{field}`: {detail}")]
    SchemaMismatch { field: &'static str, detail: String },
}

// ---------------------------------------------------------------------------
// LaunchSpec (protocol data)
// ---------------------------------------------------------------------------

/// How a session is started:
/// - `Fresh`: a brand-new conversation in the project dir.
/// - `Continue`: resume the most recent conversation in the project dir
///   (`claude --continue`).
/// - `Resume(id)`: resume a specific prior conversation by its claude
///   session id (`claude --resume <id>`).
///
/// `Continue`/`Resume` let the user pick up existing Claude Code sessions —
/// including ones started outside Marsad — by running in the right `cwd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum LaunchMode {
    #[default]
    Fresh,
    Continue,
    Resume(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub project: String,
    pub host_id: String,
    pub model: String,
    /// The chosen backend mode id (a raw `SessionMode.id` the backend truly
    /// offers, e.g. claude's `"default"|"acceptEdits"|"plan"|"bypassPermissions"`,
    /// or an opencode/codex mode id). Empty string = "use the backend's default".
    /// Applied verbatim by the driver (`--permission-mode <id>` for claude); NOT a
    /// re-invented Marsad abstraction. Renamed from the removed `posture` field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_mode: String,
    pub first_prompt: Option<String>,
    /// Start mode (fresh / continue / resume-by-id). Defaults to `Fresh` so
    /// older callers and serialized specs without this field keep working.
    #[serde(default)]
    pub mode: LaunchMode,
    /// Resolved remote execution target for the session's host, or `None`/`local`
    /// for the local machine. `user@host` (or a bare ssh alias) selects SSH;
    /// `wsl:<distro>` selects WSL. Drives which container/transport the agent uses.
    /// `#[serde(default)]` keeps older serialized specs (no field) working.
    #[serde(default)]
    pub host_target: Option<String>,
    /// The `claude` executable to run on the host (a wrapper like `genie` or an
    /// absolute path). `None` → the default `"claude"`. Resolved per-host from the
    /// registry; `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub claude_bin: Option<String>,
    /// A shell command run BEFORE claude on the host (e.g. `module load cuda`).
    /// `None` → no setup. `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub setup: Option<String>,
    /// Environment variables exported before claude on the host, as `(key, value)`
    /// pairs. `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Optional human-readable display name for the session. `None` → name is
    /// derived from the project directory basename. `#[serde(default)]` keeps
    /// older serialized specs (no field) working.
    #[serde(default)]
    pub name: Option<String>,
    /// Tools that should be disabled for this session. When non-empty the agent
    /// passes `--disallowedTools <comma-joined-list>` to the `claude` CLI.
    /// `#[serde(default)]` keeps older serialized specs (no field) working.
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// X4a (`docs/specs/wasl-connector.md` §X4a): request the agent make this
    /// session Wasl-capable by injecting a `--mcp-config` entry for the host's
    /// `wasl-mcpd` at spawn time. Defaults to `false`; a `false`/absent value
    /// is byte-identical to pre-X4 behavior (the load-bearing optionality
    /// invariant). `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub wasl: bool,
    /// Agent driver to use for this session (`"claude"` / `"codex"`).
    /// Defaults to `"claude"` so older callers and serialized specs without
    /// this field keep working. `#[serde(default = "default_driver")]` keeps
    /// older serialized specs working.
    #[serde(default = "default_driver")]
    pub driver: String,
}

// ---------------------------------------------------------------------------
// SessionMode — re-exported from wasit-core (see the re-export block above).
// `Capabilities.available_modes` uses that re-exported type; wasit's `SessionMode`
// has the identical {id, label, description} shape.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Capabilities (U1 — dynamic capability discovery, docs/specs/dynamic-capabilities.md §1)
// ---------------------------------------------------------------------------

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
        ModelCatalog {
            models: vec![],
            source: ModelListSource::Observed,
            free_text: true,
        }
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
    /// Whether the driver's binary was detected on the AGENT'S LOCAL machine
    /// (a plain `which`). Informational only — meaningful for the local host,
    /// best-effort otherwise (a remote host's PATH is not probed here). This is
    /// NOT used to gate launchability; see `launchable`. `#[serde(default)]` so
    /// old cached snapshots without the field still load.
    #[serde(default)]
    pub cli_available: bool,
    /// Whether `wasl-mcpd` was detected on the probed host (a `command -v
    /// wasl-mcpd` in the discovery script). This is a HOST-GLOBAL fact carried
    /// on every driver's caps for that host (each discovery script probes it),
    /// so the launch UI can gate a managed `wasl-*` driver on BOTH its native
    /// CLI (`cli_available`) AND wasl itself being installed. `#[serde(default)]`
    /// so old cached snapshots without the field load as `false`.
    #[serde(default)]
    pub wasl_available: bool,
    /// Whether Marsad has a real LAUNCH DRIVER for this agent — a static,
    /// per-driver fact (claude/codex → true, gemini → false), NOT a probe of
    /// the host. This is what the launch UI gates on: a driver Marsad can
    /// actually spawn is launchable on every host; a bad host setup surfaces as
    /// a session error at launch, never a permanently greyed button.
    /// `#[serde(default)]` so old cached snapshots (no field) load as `false`.
    #[serde(default)]
    pub launchable: bool,
    #[serde(default)]
    pub models: ModelCatalog,
    /// The session modes this backend truly offers on this host, discovered like
    /// `models` (never a hardcoded Marsad list). claude fills it from its version
    /// (`events::claude_available_modes`); opencode/codex leave it empty here (their
    /// modes are only knowable from a live session — they surface post-launch via
    /// the runtime `available_modes` path). Empty ⇒ the launch UI shows no mode
    /// picker and launches at the backend default. `#[serde(default)]` so old
    /// cached snapshots load with none.
    #[serde(default)]
    pub available_modes: Vec<SessionMode>,
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

// ---------------------------------------------------------------------------
// IpcMsg
// ---------------------------------------------------------------------------

/// GUI ↔ agent IPC messages (closed protocol — no Unknown variant; see
/// `docs/specs/typed-ipc.md` §4 for core's tolerant-skip disposition of
/// unknown inbound types).
///
/// `LaunchSession` is much larger than the other variants (it carries the
/// full launch wire shape, `docs/specs/typed-ipc.md` §2) — these messages are
/// constructed and immediately encoded/matched, never stored in bulk, so
/// boxing fields to shrink the enum would add indirection for no real benefit
/// (and would change the field types the pinned `ipc_typed.rs` golden tests
/// construct directly).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "type")]
pub enum IpcMsg {
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// Full launch message. This variant IS the wire shape core's
    /// `launch_session_with_id` builds directly (no post-encode mutation).
    /// Required on parse: `project`, `model`. Everything else is parse-optional
    /// (including `session_mode`, which defaults to the backend default) so the
    /// existing minimal test corpus (typed-ipc.md §7) keeps parsing.
    LaunchSession {
        /// Marsad session id, generated by core. Optional so raw clients that
        /// omit it still parse (the agent then generates one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Project directory (the agent's cwd for claude). Required.
        project: String,
        /// Legacy mirror of `project`; kept for wire compat with old readers.
        /// Core always emits it (== project); defaulted on parse.
        #[serde(default)]
        cwd: String,
        /// Host registry id ("local" when absent). Core always emits it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
        /// Remote execution target (`user@host` / ssh alias / `wsl:<distro>`);
        /// absent for local. Emitted only when resolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_target: Option<String>,
        /// Per-host claude executable; emitted only when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_bin: Option<String>,
        /// Per-host setup shell command; emitted only when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        /// Per-host env as [k,v] pairs; emitted only when non-empty.
        /// `Vec<(String,String)>` serializes to `[["K","V"],…]` — today's exact shape.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        model: String,
        /// The chosen backend mode id (a raw `SessionMode.id` the backend truly
        /// offers). Empty = the backend's default. Passed to the driver verbatim.
        /// Replaces the removed `posture`/`permission_mode` pair. Defaulted on
        /// parse so older wire messages (which omit it) launch at backend default;
        /// omitted on encode when empty so minimal goldens stay byte-clean.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        session_mode: String,
        /// ALWAYS serialized, `null` when absent — today core unconditionally
        /// inserts it. Plain Option (no skip attribute).
        #[serde(default)]
        first_prompt: Option<String>,
        /// LaunchMode::Continue on the wire. Absent otherwise.
        #[serde(default, rename = "continue", skip_serializing_if = "Option::is_none")]
        continue_: Option<bool>,
        /// LaunchMode::Resume(id) on the wire. Absent otherwise.
        /// Empty string == Fresh — preserved by `launch_mode()`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_session_id: Option<String>,
        /// Custom display name; emitted only when trimmed non-empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// --disallowedTools list; emitted only when non-empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        disallowed_tools: Vec<String>,
        /// X4a (`docs/specs/wasl-connector.md` §X4a): mirrors `LaunchSpec.wasl`
        /// on the wire. `skip_serializing_if` keeps every pre-X4 golden
        /// (`tests/ipc_typed.rs`) byte-identical on encode — `false` is simply
        /// omitted, exactly like the other optional fields above.
        #[serde(default, skip_serializing_if = "is_false")]
        wasl: bool,
        /// Agent driver discriminator.  Defaults to `"claude"` when absent so
        /// older wire messages and test fixtures keep parsing unchanged.
        /// `skip_serializing_if = "is_claude"` keeps every pre-Stage-1 golden
        /// byte-identical on encode — `"claude"` is simply omitted.
        #[serde(default = "default_driver", skip_serializing_if = "is_claude")]
        driver: String,
    },
    ResumeSession {
        session_id: String,
        /// ADDITIVE: the agent already reads this from the raw value; core
        /// never sends it today. Absent → None; absent on the wire when None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_prompt: Option<String>,
    },
    AttachSession {
        session_id: String,
    },
    StopSession {
        session_id: String,
    },
    SendPrompt {
        session_id: String,
        text: String,
    },
    /// Change a session config option on the running agent (opencode ACP
    /// `session/set_config_option`): `config_id` is "model" or "mode", `value`
    /// the option's wire value. The agent applies it and the reader surfaces the
    /// agent-confirmed new state — no optimistic local update.
    SetSessionConfigOption {
        session_id: String,
        config_id: String,
        value: String,
    },
    /// Typed form of the raw `Interrupt` line. Stops the current turn via
    /// claude's `interrupt` control_request; never kills the session.
    Interrupt {
        session_id: String,
    },
    AnswerGate {
        /// The session whose pending gate this answers (the agent keys pending
        /// gates by session id).
        session_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` — the agent's
        /// handler never actually reads the WIRE `request_id` (it answers the
        /// PENDING gate captured when the `can_use_tool` control_request
        /// arrived, keyed by `session_id`, using THAT gate's own
        /// `request_id`); a caller that omits it is exactly as functional as
        /// one that includes it. Emission unchanged (core always sets it).
        #[serde(default)]
        request_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` preserves the
        /// agent's current lenient read
        /// `value["allow"].as_bool().unwrap_or(false)` — a missing `allow`
        /// means deny, not a schema error. Emission unchanged (core always
        /// sets it).
        #[serde(default)]
        allow: bool,
        #[serde(default)]
        deny_reason: Option<String>,
        /// For an MCP elicitation that offered a scope-of-allowance choice, the
        /// `content` object to send back on accept (e.g. `{ "grant": "session" }`).
        /// The UI builds it from the picked choice's `key`/`value`. `None` for a
        /// schemaless accept or a `can_use_tool` gate. Additive + defaulted +
        /// skipped-when-absent so older callers/wire messages (and golden
        /// contracts) are unaffected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elicitation_content: Option<serde_json::Value>,
        /// For a `can_use_tool` gate where the human picked one of claude's own
        /// `permission_suggestions` ("always allow …"), the chosen suggestion
        /// object(s) — echoed back VERBATIM as `updatedPermissions`. `None` for a
        /// plain one-time allow / a deny. Additive + defaulted + skipped-when-absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_updates: Option<serde_json::Value>,
    },
    AnswerQuestion {
        /// The session whose pending AskUserQuestion gate this answers.
        session_id: String,
        request_id: String,
        /// Answers map: question text → selected label(s).  An empty object
        /// counts as "skip" (the tool returns "did not answer").
        answers: serde_json::Value,
    },
    Subscribe {
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` — the agent has
        /// never required it. Emission unchanged.
        #[serde(default)]
        session_id: String,
    },
    /// Typed form of the stringly arm that used to ignore `Unsubscribe`.
    /// Benign no-op today.
    Unsubscribe {
        /// Tolerated-absent for the same reason: today's arm ignores the payload.
        #[serde(default)]
        subscriber_id: String,
    },
    /// Typed form of the raw `SlashCmd` line. NOTE: the agent has no handler
    /// today — it falls into the unknown-type arm and replies `Error`. This
    /// becomes an EXPLICIT arm with the same observable reply
    /// (behavior-preserving; real slash forwarding is U-series work).
    SlashCmd {
        session_id: String,
        cmd: String,
    },
    /// Typed form of the raw `SetWorkspace` line. Advisory broadcast; agent
    /// has no handler today (same explicit-arm treatment as SlashCmd).
    SetWorkspace {
        workspace_id: String,
    },
    /// Ask the agent to (re)probe capabilities. Fire-and-forget: the reply is
    /// the `CapabilitiesRefreshQueued` ack; results arrive later as
    /// `CapabilitiesUpdate` broadcasts. `None` = all.
    RefreshCapabilities {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        driver: Option<String>,
        /// claude sessions need a project scope (dynamic-capabilities.md §1.2);
        /// adds it to the host's `SessionScope::Projects` set. Ignored by
        /// `AllKnown` drivers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        /// The host's remote execution target (`user@host`, `wsl:<distro>`,
        /// `plink:<inner>`, …), resolved from the registry by core. `None` = the
        /// AGENT'S LOCAL machine — discovery runs locally (setup still runs). A
        /// remote target makes the probe run its discovery script over that
        /// host's connection (`run_remote_script_bounded`). Distinct from
        /// `host_id` (an opaque registry key that is NOT a shell target).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_target: Option<String>,
        /// The host's pre-command run BEFORE any discovery on this host (e.g.
        /// `module load cuda`, `source ~/.env`) so `command -v`, `codex debug
        /// models`, etc. see the same environment a launched session would.
        /// `None` = no setup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        /// The host's per-driver command overrides (`(driver, command)` pairs),
        /// so the probe resolves the EXACT command a launch would use (the first
        /// token becomes the `command -v` word, the whole command runs models
        /// subcommands). Empty = no overrides (fall back to `claude_bin`/bare
        /// name).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        agent_commands: Vec<(String, String)>,
        /// The host's `claude` executable override (wrapper like `genie` / an
        /// absolute path). Fallback for the `claude` driver when `agent_commands`
        /// has no `"claude"` entry. `None` = bare `claude`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_bin: Option<String>,
    },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    VmUpdate {
        session_id: String,
        update: serde_json::Value,
    },
    /// One rich, typed agent event flowing agent → core. The wire now carries a
    /// `wasit_core::AgentEvent` (internally-tagged, snake_case) directly instead
    /// of a raw claude stream-json `Value` — the driver-specific parsing moved
    /// into the wasit drivers, and `marsad-events` reduces over `AgentEvent`.
    /// `session_id` correlates the event to a session (existing parse rules
    /// unchanged: required on the wire, keyed by core).
    AgentEvent {
        session_id: String,
        event: AgentEvent,
    },
    /// Typed form of the raw `TranscriptBackfill` line. Remote session
    /// history: the raw `.jsonl` text read on the remote host, shipped so
    /// core can backfill the transcript and aggregate model usage.
    TranscriptBackfill {
        session_id: String,
        /// Absent/empty → core no-ops. Mirrors IpcEnvelope.
        #[serde(default)]
        jsonl: String,
    },
    /// Typed form of the raw `ChangesUpdate` line. Remote git-probe result.
    ChangesUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core parses it leniently
        /// (`from_value::<Vec<FileChange>>(…).unwrap_or_default()` — malformed
        /// → empty, never an error) and `FileChange` lives in
        /// marsad-viewmodel, which proto must not depend on (DAG). Element
        /// shape (informative): `{"path":"a.rs","status":"M","added":3,"removed":1}`.
        #[serde(default)]
        changes: serde_json::Value,
    },
    /// Typed form of the raw `UsageUpdate` line. Project-wide per-model usage
    /// from the remote python3 aggregation script.
    UsageUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core's lenient handling (missing →
        /// no-op, non-object → no-op) must survive. Shape (informative):
        /// `{"<model>":{"input":N,"output":N,"cache_read":N,"cache_creation":N}}`.
        #[serde(default)]
        usage: serde_json::Value,
    },
    // Command acks (agent serve loop replies).
    SessionLaunched {
        session_id: String,
    },
    SessionResumed {
        session_id: String,
    },
    SessionAttached {
        session_id: String,
    },
    SessionStopped {
        session_id: String,
    },
    PromptSent {
        session_id: String,
    },
    Interrupted {
        session_id: String,
    },
    GateAnswered {
        session_id: String,
    },
    QuestionAnswered {
        session_id: String,
    },
    Subscribed {
        subscriber_id: String,
    },
    /// The serve loop's error reply.
    Error {
        message: String,
    },
    /// Serve-loop ack (house convention: every command line gets a typed
    /// reply) for `RefreshCapabilities`.
    CapabilitiesRefreshQueued {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
    },
    /// One (driver, host) snapshot — broadcast to all subscribers on probe
    /// completion AND replayed from cache to each new subscriber. Always a
    /// COMPLETE snapshot (replace, not merge, on the receiving side).
    CapabilitiesUpdate {
        host_id: String,
        driver: String,
        /// Typed — `Capabilities` lives in proto (§1), unlike `ChangesUpdate`'s
        /// `Value` (whose `FileChange` lives in viewmodel; no such DAG
        /// constraint here).
        capabilities: Capabilities,
    },

    // ── X1 Wasl connector (docs/specs/wasl-connector.md §3) ─────────────────
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// NEW: relay ONE explicit human decision on a Wasl gate to the node's
    /// operator endpoint. Marsad-originated only — never synthesized (hard rule 2).
    AnswerWaslGate {
        host_id: String,
        gate_id: String,
        /// Wasl outcome string, relayed verbatim: "approved" | "denied"
        /// ("dismissed" is wire-valid for forward compat; core never emits it in X1).
        #[serde(default)]
        outcome: String,
        /// Denial reason. ALWAYS serialized (null when absent), like LaunchSession.first_prompt.
        #[serde(default)]
        reason: Option<String>,
    },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    /// NEW broadcast: a Wasl gate surfaced (or re-surfaced via resnapshot reconcile).
    WaslGateRequested {
        host_id: String,
        gate_id: String,
        #[serde(default)]
        mode: String,
        #[serde(default)]
        decidable: bool,
        /// Verbatim Wasl GateRequest object; Value ON PURPOSE (typed form is
        /// marsad_wasl::GateRequest; core parses leniently — §5.2).
        #[serde(default)]
        request: serde_json::Value,
        #[serde(default)]
        requested_at: String,
        #[serde(default)]
        timeout_ms: u64,
        #[serde(default)]
        deadline: String,
    },
    /// NEW broadcast: a Wasl gate resolved.
    WaslGateDecided {
        host_id: String,
        gate_id: String,
        /// Wasl outcome verbatim, PLUS "unknown" for connector-synthesized
        /// GateVanished (Marsad-wire-only value; never sent to Wasl).
        #[serde(default)]
        outcome: String,
        /// Wasl decided_by verbatim, PLUS "resync" for GateVanished.
        #[serde(default)]
        decided_by: String,
        #[serde(default)]
        reason: Option<String>, // always serialized, null when absent
        /// RFC 3339 from the frame; "" for GateVanished (no node timestamp exists).
        #[serde(default)]
        decided_at: String,
    },
    /// NEW broadcast: one audit-feed entry (the observation feed — EVERY durable
    /// audit append, including auto-approved sends).
    WaslObservation {
        host_id: String,
        /// Verbatim Wasl AuditEntry; Value ON PURPOSE (same rationale as `request`).
        #[serde(default)]
        entry: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to AnswerWaslGate): the decision was RELAYED to the
    /// connector — not that the POST succeeded (the WaslGateDecided broadcast is the
    /// authoritative resolution signal, mirroring the contract's own 204 semantics).
    WaslGateAnswered {
        host_id: String,
        gate_id: String,
    },

    // ── Wasl backend (Wasl-managed session, docs/design/wasl-backend-plan.md §7) ──
    // A DIFFERENT feature from the X1 operator connector above: here the
    // session's process is owned end-to-end by Wasl; Marsad is a thin client of
    // a Wasl DeliveryTarget (prompt / observe / native-gate / interrupt / stop).
    // Payloads whose typed form lives in `marsad-wasl-session` are carried as
    // `serde_json::Value` ON PURPOSE (proto must not depend on that crate, DAG).
    // ── GUI/core → agent ─────────────────────────────────────────────────────────
    /// NEW: start a Wasl-managed session. The agent constructs a
    /// `WaslSessionClient` via the provider factory (fake in tests; real later)
    /// and registers it under `session_id`. No argv, no container — the process
    /// is Wasl's. `wasl_target_key` selects the Wasl target (protocol + context).
    /// Analogue of `LaunchSession` for the `WaslManaged` owner. Identity:
    /// `session_id`, `host_id` (required → SchemaMismatch when absent).
    LaunchWaslSession {
        session_id: String,
        host_id: String,
        /// Opaque Wasl target key (protocol id + context). Value ON PURPOSE
        /// (typed form `WaslTargetKey` lives in marsad-wasl-session). Relayed
        /// verbatim to `WaslSessionProvider::get_or_start`; also persisted into
        /// the `WaslSessionBinding` (§6.1) as `provider_key` for reconnect.
        #[serde(default)]
        wasl_target_key: serde_json::Value,
        #[serde(default)]
        name: Option<String>,
    },
    /// NEW: prompt a Wasl-managed session (Wasl `send`, source LocalHuman).
    /// Carries the busy policy and idempotency key so the agent builds a
    /// `WaslTurnRequest`. Analogue of `SendPrompt`. `text` is REQUIRED (mirrors
    /// `SendPrompt.text`): a prompt with no text is not a meaningful send, so a
    /// missing `text` is a SchemaMismatch.
    SendWaslPrompt {
        session_id: String,
        text: String,
        #[serde(default)]
        idempotency_key: String,
        /// "queue" | "reject" | "queue_related" (+ max_wait_ms); default queue.
        #[serde(default)]
        busy_policy: String,
        #[serde(default)]
        max_wait_ms: u64,
    },
    /// NEW: (re)subscribe to a Wasl session's observe stream from a sequence.
    /// Analogue of `AttachSession`; the agent (re)pumps `attach(after_seq)` on
    /// the per-session observe thread.
    AttachWaslSession {
        session_id: String,
        #[serde(default)]
        after_seq: u64,
    },
    /// NEW: resolve a native gate (Wasl `gate approve|deny`, §8) — the SAME
    /// human decision the gate UI produces, routed to `resolve_native_gate`.
    /// Identity: `session_id`, `gate_id` (required). `outcome` is REQUIRED
    /// ("approved" | "denied", relayed as `WaslGateDecision`).
    AnswerWaslNativeGate {
        session_id: String,
        gate_id: String,
        outcome: String,
        /// Denial reason. ALWAYS serialized (null when absent), like
        /// `AnswerWaslGate.reason` / `LaunchSession.first_prompt`.
        #[serde(default)]
        reason: Option<String>,
    },
    /// NEW: interrupt the active turn (Wasl `cancel_turn`). Analogue of
    /// `Interrupt`.
    InterruptWaslSession {
        session_id: String,
    },
    /// NEW: stop the session (Wasl `shutdown`). `drain` = Drain else Immediate.
    /// Analogue of `StopSession`.
    StopWaslSession {
        session_id: String,
        #[serde(default)]
        drain: bool,
    },

    // ── agent → GUI/core ──────────────────────────────────────────────────────────
    /// NEW broadcast: a Wasl session's advisory status changed. Carries the raw
    /// status kind + fields as a Value; core projects it to `Phase` (§6.3).
    /// Emitted on every status transition the observe thread sees (and on
    /// connect).
    WaslSessionStatus {
        session_id: String,
        /// Verbatim `WaslTargetStatus`; Value ON PURPOSE (typed form in
        /// marsad-wasl-session).
        #[serde(default)]
        status: serde_json::Value,
    },
    /// NEW broadcast: one normalized observe event (assistant delta / tool /
    /// turn lifecycle / native-gate / exit / observe-lost). Value-carried; core
    /// folds it into the transcript + gate projection. The Wasl analogue of a
    /// claude `StreamMsg` line. `seq` is REQUIRED (no `#[serde(default)]`): it is
    /// the replay/dedup/ordering key (§6.1/§10.4); an event with no sequence
    /// cannot be folded gap/dup-free, so a missing `seq` is a SchemaMismatch, not
    /// a silent 0.
    WaslSessionEvent {
        session_id: String,
        seq: u64,
        #[serde(default)]
        event: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to `SendWaslPrompt`): carries the `TurnReceipt`
    /// so the UI can show queued/rejected outcomes. NOT the reply (that arrives
    /// as `WaslSessionEvent`::TurnCompleted).
    WaslPromptReceipt {
        session_id: String,
        /// Verbatim `WaslTurnReceipt`; Value ON PURPOSE.
        #[serde(default)]
        receipt: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to `AnswerWaslNativeGate`): the decision was
    /// RELAYED to the client (not proof the CLI accepted it — the follow-on
    /// `WaslSessionStatus`/`WaslSessionEvent` is authoritative, mirroring X1's
    /// `WaslGateAnswered` semantics). Identity: `session_id`, `gate_id`.
    WaslNativeGateAnswered {
        session_id: String,
        gate_id: String,
    },

    // ── PTY (WAVE 3b — interactive raw-shell escape hatch, Terminal tab) ────
    // A SEPARATE channel from the claude driver: this streams a real
    // interactive shell (local or the session's existing SSH/WSL host), NOT
    // claude's stream-json protocol. `docs/design/DESIGN_HANDOFF.md`'s
    // "Terminal tab" (raw escape-hatch terminal, mono, dark).
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// Start (or, if already live, no-op re-attach to) the pty for a session.
    /// No ack variant — the pty coming alive is observed via `PtyOutput`
    /// broadcasts, same "fire and forget" disposition as `SetWorkspace`.
    PtyStart {
        session_id: String,
        /// Which terminal within the session (v2: multiple terminals per
        /// session). Empty → the session's default terminal (back-compat: the
        /// agent keys the pty registry by session_id when this is empty).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Raw keystroke bytes typed into the terminal, forwarded verbatim to the
    /// pty's stdin. Plain UTF-8 text (unlike `PtyOutput.data`): terminal INPUT
    /// is always a JS string from xterm.js's `onData` — even control
    /// characters (arrow-key escape sequences, Ctrl-C) are valid Unicode
    /// scalar values and round-trip through a JSON string with no encoding
    /// loss, so no base64 is needed here.
    PtyInput {
        session_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
        data: String,
    },
    /// The terminal's viewport was resized (e.g. the desktop window resized,
    /// or `xterm-addon-fit` recomputed cols/rows).
    PtyResize {
        session_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Close (kill + reap) ONE terminal of a session (v2: the user clicked ✕ on
    /// a terminal tab, or "restart" = close then a fresh PtyStart). Fire-and-
    /// forget; the teardown is observed via the terminal's `PtyExit` broadcast.
    PtyClose {
        session_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
    },
    // ── agent → GUI/core ────────────────────────────────────────────────────
    /// A chunk of raw bytes read from the pty (merged stdout+stderr — a pty
    /// has exactly one fd for both). `data` is BASE64-encoded (unlike
    /// `PtyInput.data`): terminal OUTPUT is arbitrary bytes — control
    /// sequences, and reads that land mid-way through a multi-byte UTF-8
    /// sequence — so it cannot be carried losslessly as a plain JSON string;
    /// base64 round-trips it byte-for-byte. The frontend decodes it and feeds
    /// the raw bytes straight to `xterm.js`'s `Terminal.write`.
    PtyOutput {
        session_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
        data: String,
    },
    /// The pty's child process exited (the shell was closed, `exit` was
    /// typed, or the process crashed). `code` is the process exit code
    /// (`-1` when the pty could not even be spawned).
    PtyExit {
        session_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        terminal_id: String,
        code: i32,
    },

    // ── Wasl peer registration (GUI/core → agent) ────────────────────────────
    /// Register one peer node in a Wasl node's `/v1/peers` registry. Best-effort:
    /// the agent logs errors but does NOT fail the connection operation. `host_id`
    /// identifies which Wasl node receives the peer (the "to" node). `peer_id`,
    /// `peer_host`, and (optional) `peer_endpoint` describe the peer being added.
    RegisterWaslPeer {
        /// The Marsad host id of the node whose `/v1/peers` we are writing to.
        host_id: String,
        /// The stable agent id of the peer to register.
        peer_id: String,
        /// The peer node's hostname/address label.
        peer_host: String,
        /// The peer's HTTP control endpoint (e.g. `"http://127.0.0.1:8787"`), or
        /// absent when not yet resolvable. Skipped on encode when None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer_endpoint: Option<String>,
    },
    /// Deregister a peer node from a Wasl node's `/v1/peers` registry. Best-effort.
    UnregisterWaslPeer {
        /// The Marsad host id of the node whose `/v1/peers` we are writing to.
        host_id: String,
        /// The stable agent id of the peer to remove.
        peer_id: String,
    },

    // ── Session liveness (agent → GUI/core) ──────────────────────────────────
    /// Authoritative session liveness, emitted by the reader thread: `true` when
    /// it attaches to a running session (launch/resume/reconnect), `false` when
    /// the process is confirmed gone — the remote/local claude exited, the host
    /// rebooted / OOM-killed / scheduler-killed it, or reconnect gave up. Core
    /// flips the session's lifecycle (Resumed ↔ Paused) from THIS, not from the
    /// turn phase, so a finished-but-alive session stays Resumed and a
    /// host-killed session honestly shows Paused (the two-axis model).
    SessionLiveness {
        session_id: String,
        live: bool,
    },
    /// Agent→core: the per-session CONNECTION health, distinct from liveness. A
    /// remote session's stream can drop / go slow while the process is still
    /// alive on the host (still Resumed) — this reports whether Marsad currently
    /// has a clean stream to it. Driven by REAL reader events (stream EOF, LIVE
    /// re-attach, repeated flaps), not the periodic host poll, so it's prompt and
    /// per-session. Core surfaces it on the SessionView so the UI can show the
    /// true state and HARD-BLOCK sending into a broken pipe. `live=false`
    /// (Paused) always wins over this.
    SessionConn {
        session_id: String,
        health: ConnHealth,
    },
}

/// Per-session connection health (see `IpcMsg::SessionConn`). Serializes as its
/// PascalCase variant name ("Connected"/"Reconnecting"/"Unstable"), matching the
/// TS `ConnHealth` mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnHealth {
    /// Clean stream to the session.
    Connected,
    /// Stream dropped; the reader is actively re-attaching (auto-reconnect,
    /// backoff still growing) — a transient blip.
    Reconnecting,
    /// The stream keeps flapping (re-attached then dropped again before it
    /// restabilized) — reachable but unreliable.
    Unstable,
    /// Sustained loss of contact: the reconnect backoff has reached its steady
    /// max and the host is still unreachable. The durable session is PRESUMED
    /// ALIVE (we could not confirm it dead — that would be `Paused`), and the
    /// reader keeps probing at the steady interval; it flips back to `Connected`
    /// the moment contact returns. Distinct from `Reconnecting` (a brief blip)
    /// and from `Paused`/`live=false` (process confirmed gone).
    Disconnected,
}

impl IpcMsg {
    /// `LaunchSession` only: derive the `LaunchMode` from the wire fields.
    /// Precedence: non-empty `resume_session_id` wins, then `continue == true`,
    /// else `Fresh`. Empty-string `resume_session_id` is treated as absent
    /// (preserves the agent's historical `parse_launch_mode_value` behavior).
    pub fn launch_mode(&self) -> Option<LaunchMode> {
        match self {
            IpcMsg::LaunchSession {
                continue_,
                resume_session_id,
                ..
            } => {
                if let Some(id) = resume_session_id {
                    if !id.is_empty() {
                        return Some(LaunchMode::Resume(id.clone()));
                    }
                }
                if *continue_ == Some(true) {
                    Some(LaunchMode::Continue)
                } else {
                    Some(LaunchMode::Fresh)
                }
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a serde deserialization error into a `SchemaMismatch`.
/// Centralised so the closure body is in one place and always covered by
/// whichever arm fires a schema error first (currently the "result" arm).
fn schema_err(field: &'static str, e: serde_json::Error) -> ProtoError {
    ProtoError::SchemaMismatch {
        field,
        detail: e.to_string(),
    }
}

/// Deserialise `value` into `T`, mapping any error to `SchemaMismatch`.
fn from_val<T: serde::de::DeserializeOwned>(
    field: &'static str,
    value: serde_json::Value,
) -> Result<T, ProtoError> {
    serde_json::from_value(value).map_err(|e| schema_err(field, e))
}

/// `skip_serializing_if` predicate: true when `b` is `false` (X4a's
/// `wasl` field — omitted on encode when off, matching every other
/// optional/default field's "absent means off" wire convention).
fn is_false(b: &bool) -> bool {
    !*b
}

/// Default driver string for `LaunchSpec.driver` and `IpcMsg::LaunchSession`'s
/// `driver` field.  Returned by `#[serde(default = "default_driver")]` so
/// older serialized specs (no `driver` key) and pre-Stage-1 wire messages
/// transparently load as `"claude"` — the only driver in production before
/// Stage 1.
pub fn default_driver() -> String {
    "claude".to_string()
}

/// `skip_serializing_if` predicate for the `driver` field in
/// `IpcMsg::LaunchSession`: omits the field when the driver is `"claude"`
/// (the default) so older wire readers keep parsing byte-identically.
/// Non-`"claude"` drivers (e.g. `"codex"`) are always emitted.
fn is_claude(s: &str) -> bool {
    s == "claude"
}

// ---------------------------------------------------------------------------
// parse_ipc
// ---------------------------------------------------------------------------

/// Parse one NDJSON line from the GUI↔agent IPC channel.
///
/// Unlike the stream protocol, an unrecognized `type` is `UnknownDiscriminant` (a bug).
pub fn parse_ipc(line: &str) -> Result<IpcMsg, ProtoError> {
    // Parse raw JSON.
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtoError::MalformedJson(e.to_string()))?;

    // Extract the `type` discriminant — must be a string field in the object.
    let type_str = value
        .as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(|v| v.as_str())
        .ok_or(ProtoError::MissingDiscriminant)?;

    // Known IPC variants — any other type is a bug (UnknownDiscriminant).
    const KNOWN: &[&str] = &[
        "LaunchSession",
        "ResumeSession",
        "AttachSession",
        "StopSession",
        "SendPrompt",
        "Interrupt",
        "AnswerGate",
        "AnswerQuestion",
        "Subscribe",
        "Unsubscribe",
        "SlashCmd",
        "SetWorkspace",
        "VmUpdate",
        "AgentEvent",
        "TranscriptBackfill",
        "ChangesUpdate",
        "UsageUpdate",
        "SessionLaunched",
        "SessionResumed",
        "SessionAttached",
        "SessionStopped",
        "PromptSent",
        "Interrupted",
        "GateAnswered",
        "QuestionAnswered",
        "Subscribed",
        "Error",
        "RefreshCapabilities",
        "CapabilitiesRefreshQueued",
        "CapabilitiesUpdate",
        "AnswerWaslGate",
        "WaslGateRequested",
        "WaslGateDecided",
        "WaslObservation",
        "WaslGateAnswered",
        "LaunchWaslSession",
        "SendWaslPrompt",
        "AttachWaslSession",
        "AnswerWaslNativeGate",
        "InterruptWaslSession",
        "StopWaslSession",
        "WaslSessionStatus",
        "WaslSessionEvent",
        "WaslPromptReceipt",
        "WaslNativeGateAnswered",
        "PtyStart",
        "PtyInput",
        "PtyResize",
        "PtyOutput",
        "PtyExit",
        "SessionLiveness",
        "SessionConn",
        "RegisterWaslPeer",
        "UnregisterWaslPeer",
    ];

    if !KNOWN.contains(&type_str) {
        return Err(ProtoError::UnknownDiscriminant(type_str.to_string()));
    }

    // Now deserialize — schema errors will propagate as SchemaMismatch.
    from_val("ipc", value)
}

// ---------------------------------------------------------------------------
// encode_ipc
// ---------------------------------------------------------------------------

/// Encode an `IpcMsg` as an NDJSON line.
pub fn encode_ipc(msg: &IpcMsg) -> String {
    serde_json::to_string(msg).expect("IpcMsg is always serializable")
}
