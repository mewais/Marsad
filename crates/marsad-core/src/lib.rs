//! marsad-core — facade tying B01–B11 into the single API the desktop (Tauri)
//! and phones (UniFFI) call.
//!
//! See `docs/specs/core.md` for the authoritative contract.
//!
//! UniFFI binding generation is deferred to the phone-app build; this crate is a
//! plain Rust library driven entirely by the unit-test suite over `LoopbackFake`.

use std::collections::HashMap;

use marsad_events::{reduce, Event, SessionState};
use marsad_gates::{DecisionLedger, GateDecision};
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_transport::Transport;
use marsad_viewmodel::{AppSnapshot, Filter, SessionView, Tab};
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Public re-exports (spec requirement: callers use marsad_core::{LaunchSpec, Posture})
// ---------------------------------------------------------------------------

pub use marsad_proto::{LaunchMode, LaunchSpec, Posture};

// ---------------------------------------------------------------------------
// CoreError
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("unknown session: {0}")]
    UnknownSession(String),

    #[error("agent error: {0}")]
    Agent(String),
}

// ---------------------------------------------------------------------------
// HostHealthPayload — Default-able wrapper for host health snapshots
// ---------------------------------------------------------------------------

/// Payload for `VmUpdate::HostHealth`.  Implements `Default` so tests can
/// construct it with `Default::default()` without specifying a variant.
#[derive(Debug, Clone, Default)]
pub struct HostHealthPayload {
    pub host_id: String,
    pub status: Option<marsad_hosts::HostStatus>,
}

// ---------------------------------------------------------------------------
// VmUpdate
// ---------------------------------------------------------------------------

/// A single UI update emitted by `on_agent_line` or other state-changing calls.
#[derive(Debug, Clone)]
pub enum VmUpdate {
    /// Overview-panel aggregates changed (counts, groups).
    Overview(marsad_viewmodel::OverviewGroups),

    /// One session's view-model changed.
    Session(SessionView),

    /// A toast notification.
    Toast { icon: String, text: String },

    /// Host health changed.
    HostHealth(HostHealthPayload),
}

// ---------------------------------------------------------------------------
// CoreConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub agent_endpoint: String,
    pub data_root: String,
}

// ---------------------------------------------------------------------------
// Per-session record
// ---------------------------------------------------------------------------

struct SessionRecord {
    project: String,
    state: SessionState,
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

pub struct Core {
    _cfg: CoreConfig,
    transport: Box<dyn Transport>,
    session_order: Vec<String>,
    sessions: HashMap<String, SessionRecord>,
    ledger: DecisionLedger,
    active_ws: Option<String>,
}

impl Core {
    /// Create a new `Core`.  The transport is connected eagerly; if the
    /// underlying connect() call fails (e.g. broken socket) the error is
    /// silently ignored here — the first send will surface a BrokenPipe instead.
    /// This keeps `Core::new` infallible in all committed test scenarios.
    pub fn new(cfg: CoreConfig, mut transport: Box<dyn Transport>) -> Result<Core, CoreError> {
        // Best-effort connect; ignore errors (send_line will detect them).
        let _ = transport.connect();
        Ok(Core {
            _cfg: cfg,
            transport,
            session_order: Vec::new(),
            sessions: HashMap::new(),
            ledger: DecisionLedger::new(),
            active_ws: None,
        })
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    pub fn snapshot(&self) -> AppSnapshot {
        let sessions: Vec<SessionView> = self
            .session_order
            .iter()
            .filter_map(|id| self.sessions.get(id).map(|r| self.make_session_view(id, r)))
            .collect();

        AppSnapshot {
            sessions,
            hosts: Vec::new(),
            workspaces: Vec::new(),
            active_ws: self.active_ws.clone(),
            selected_id: None,
            filter: Filter::All,
            search: String::new(),
            tab: Tab::Activity,
        }
    }

    // -----------------------------------------------------------------------
    // Session commands
    // -----------------------------------------------------------------------

    pub fn launch_session(&mut self, spec: LaunchSpec) -> Result<String, CoreError> {
        let id = new_session_id();

        let permission_mode = posture_to_permission_mode(&spec.posture);
        let msg = IpcMsg::LaunchSession {
            cwd: spec.project.clone(),
            model: spec.model.clone(),
            permission_mode: permission_mode.to_string(),
        };
        // encode_ipc + serde round-trips on our own structs are infallible.
        let encoded_str = encode_ipc(&msg);
        let mut val: Value = serde_json::from_str(&encoded_str).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.insert("session_id".to_string(), Value::String(id.clone()));
        obj.insert("project".to_string(), Value::String(spec.project.clone()));
        // The agent decodes posture from `posture` (the enum variant name);
        // `permission_mode` above is kept for back-compat.
        obj.insert(
            "posture".to_string(),
            serde_json::to_value(&spec.posture).unwrap(),
        );
        let fp_val = spec
            .first_prompt
            .as_deref()
            .map_or(Value::Null, |s| Value::String(s.to_string()));
        obj.insert("first_prompt".to_string(), fp_val);
        // Forward the start mode so the agent can resume/continue an existing
        // conversation (claude --resume <id> / --continue). Fresh adds nothing.
        match &spec.mode {
            LaunchMode::Continue => {
                obj.insert("continue".to_string(), Value::Bool(true));
            }
            LaunchMode::Resume(sid) => {
                obj.insert("resume_session_id".to_string(), Value::String(sid.clone()));
            }
            LaunchMode::Fresh => {}
        }
        let line = serde_json::to_string(&val).unwrap();

        self.send_raw_line(&line)?;

        self.session_order.push(id.clone());
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: spec.project,
                state: SessionState::initial(),
            },
        );

        Ok(id)
    }

    pub fn resume_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::ResumeSession {
            session_id: id.to_string(),
        })
    }

    pub fn attach_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::AttachSession {
            session_id: id.to_string(),
        })
    }

    pub fn stop_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        })
    }

    pub fn send_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::SendPrompt {
            session_id: id.to_string(),
            text: text.to_string(),
        })
    }

    pub fn interrupt(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Reuse StopSession with an added "interrupt" flag (infallible serde).
        let mut val: Value = serde_json::from_str(&encode_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        }))
        .unwrap();
        val.as_object_mut()
            .unwrap()
            .insert("interrupt".to_string(), Value::Bool(true));
        self.send_raw_line(&serde_json::to_string(&val).unwrap())
    }

    pub fn answer_gate(&mut self, decision: GateDecision) -> Result<(), CoreError> {
        let session_id = marsad_gates::route(&decision).to_string();
        self.require_session(&session_id)?;

        // Idempotency: already recorded → silent no-op (no duplicate IPC).
        if self.ledger.seen(&decision.id) {
            return Ok(());
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        // unwrap() is infallible in this code path.
        self.ledger.record(&decision).unwrap();

        let ctrl = marsad_gates::to_control_response(&decision);
        self.send_ipc(&IpcMsg::AnswerGate {
            request_id: ctrl.request_id,
            allow: ctrl.allow,
            deny_reason: ctrl.deny_reason,
        })
    }

    pub fn run_slash(
        &mut self,
        id: &str,
        cmd: marsad_viewmodel::SlashCmd,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        // serde_json::json! + to_string is infallible for literal values.
        let line = serde_json::to_string(&serde_json::json!({
            "type": "SlashCmd",
            "session_id": id,
            "cmd": slash_cmd_name(&cmd),
        }))
        .unwrap();
        self.send_raw_line(&line)
    }

    pub fn set_workspace(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.active_ws = Some(id.to_string());
        // Best-effort outbound notification (infallible construction).
        let line = serde_json::to_string(&serde_json::json!({
            "type": "SetWorkspace",
            "session_id": id,
        }))
        .unwrap();
        // Best-effort advisory send; ignore errors.
        let _ = self.transport.send_line(&line);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Inbound IPC parsing — central testable seam
    // -----------------------------------------------------------------------

    pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // Parse line as a typed envelope.  serde rejects: empty strings, non-JSON,
        // truncated JSON, non-objects, missing/wrong-typed "type" or "session_id"
        // fields — all via a single map_err closure that is exercised by the
        // existing protocol error tests.
        let env: IpcEnvelope =
            serde_json::from_str(line).map_err(|e| CoreError::Protocol(e.to_string()))?;

        match env.r#type.as_str() {
            "agent_event" => self.handle_agent_event(env),
            other => Err(CoreError::Protocol(format!("unknown IPC type: '{other}'"))),
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn handle_agent_event(&mut self, env: IpcEnvelope) -> Result<Vec<VmUpdate>, CoreError> {
        let session_id = match env.session_id {
            Some(s) => s,
            None => {
                return Err(CoreError::Protocol(
                    "missing session_id in agent_event".to_string(),
                ))
            }
        };

        // An agent_event for a session this core never registered (e.g. one the
        // agent supervises but we have not launched/adopted) is an UnknownSession,
        // not a panic.
        self.require_session(&session_id)?;

        // Parse the event into a StreamMsg; treat unparseable events as Unknown
        // (forward-compat, no error).
        let event_json = serde_json::to_string(&env.event).unwrap(); // infallible
        let stream_msg = marsad_proto::parse_stream_line(&event_json)
            .unwrap_or(marsad_proto::StreamMsg::Unknown(env.event));

        // Safe: require_session above guarantees the entry exists.
        let record = self.sessions.get_mut(&session_id).unwrap();
        let old_state = record.state.clone();
        record.state = reduce(old_state, Event::Stream(stream_msg));

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)])
    }

    fn require_session(&self, id: &str) -> Result<(), CoreError> {
        if self.sessions.contains_key(id) {
            Ok(())
        } else {
            Err(CoreError::UnknownSession(id.to_string()))
        }
    }

    /// Send a raw line over the transport.
    fn send_raw_line(&mut self, line: &str) -> Result<(), CoreError> {
        self.transport
            .send_line(line)
            .map_err(|e| CoreError::Transport(e.to_string()))
    }

    /// Encode an `IpcMsg` and send it.
    fn send_ipc(&mut self, msg: &IpcMsg) -> Result<(), CoreError> {
        self.send_raw_line(&encode_ipc(msg))
    }

    fn make_session_view(&self, id: &str, record: &SessionRecord) -> SessionView {
        let s = &record.state;
        SessionView {
            id: id.to_string(),
            name: id.to_string(),
            host_id: "local".to_string(),
            project: record.project.clone(),
            branch: String::new(),
            model: s.model.clone().unwrap_or_default(),
            state: s.phase.clone(),
            changed_files: s.changed_files,
            subagents: s.subagents.len() as u32,
            cost_usd: s.cost_usd,
            tokens: s.tokens,
            context_pct: s.context_pct,
            gate: s.gate.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// IpcEnvelope — serde-typed wrapper for inbound IPC lines
// ---------------------------------------------------------------------------

/// Typed representation of an inbound IPC line.
///
/// Using serde deserialization here means serde enforces all structural
/// invariants (correct field types, required fields) in a single `from_str`
/// call, so all structural Protocol errors go through one `map_err` closure.
#[derive(serde::Deserialize)]
struct IpcEnvelope {
    /// IPC message type discriminant.  Required; must be a string.
    #[serde(rename = "type")]
    r#type: String,
    /// Session id.  Optional at the envelope level; validated in dispatch.
    session_id: Option<String>,
    /// Raw event payload for agent_event messages.
    #[serde(default)]
    event: Value,
}

// ---------------------------------------------------------------------------
// Private free functions
// ---------------------------------------------------------------------------

fn new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{n:016x}")
}

fn posture_to_permission_mode(posture: &Posture) -> &'static str {
    match posture {
        Posture::Ask => "default",
        Posture::AutoApproveSafe => "autoApprove",
        Posture::FullAutonomy => "bypassPermissions",
    }
}

fn slash_cmd_name(cmd: &marsad_viewmodel::SlashCmd) -> &str {
    use marsad_viewmodel::SlashCmd;
    match cmd {
        SlashCmd::Clear => "clear",
        SlashCmd::Compact => "compact",
        SlashCmd::Cost => "cost",
        SlashCmd::Model => "model",
        SlashCmd::Config => "config",
        SlashCmd::Init => "init",
        SlashCmd::Review => "review",
        SlashCmd::Agents => "agents",
        SlashCmd::Resume => "resume",
        SlashCmd::Terminal => "terminal",
        SlashCmd::Help => "help",
        SlashCmd::Unknown(s) => s.as_str(),
    }
}
