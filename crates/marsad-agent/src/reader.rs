//! Per-session reader thread: connect the session transport (local FIFO/pipe or
//! a remote durable FIFO bridge), pump its NDJSON stream, and broadcast decoded
//! events to every subscriber — plus the small pure-logic helpers
//! (`reconnect_backoff_next`, `assistant_has_file_modifying_tool_use`, the
//! `RECONNECT_*` constants) that drive its remote auto-reconnect behavior.

mod util;
pub use util::*;

#[cfg(not(coverage))]
use std::collections::HashMap;

#[cfg(not(coverage))]
use super::remote::{
    exec_prefix_for, parse_remote_changes, remote_changes_script, remote_reader_argv,
    remote_resolve_pid_dir, remote_session_dir, remote_transcript_script, remote_usage_script,
    remote_writer_argv, resolve_remote_bin, run_remote_script,
};
#[cfg(not(coverage))]
use super::{PendingGate, SharedWriter};
#[cfg(not(coverage))]
use std::sync::atomic::{AtomicBool, Ordering};

// ── Per-session reader cancellation ──────────────────────────────────────────
// A reader may sit in a steady reconnect-probe backoff after a connection drop
// (an unreachable durable session is probed indefinitely, presumed alive). If the
// user pauses the session, or a NEW reader is spawned for the same session
// (resume / reconnect
// on a flapping host), the OLD reader MUST exit promptly — otherwise readers
// stack up (observed 14-for-one on a flaky etx host), multiplying gate broadcasts
// and stream events into churn. `spawn_session_reader` registers a fresh token
// (tripping any prior one for that sid); `cancel_reader` (StopSession) trips it.

#[cfg(not(coverage))]
fn reader_cancels() -> &'static std::sync::Mutex<HashMap<String, std::sync::Arc<AtomicBool>>> {
    static CANCELS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, std::sync::Arc<AtomicBool>>>,
    > = std::sync::OnceLock::new();
    CANCELS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Trip the current reader thread's cancel token for `sid` (StopSession/pause),
/// so a lingering reconnect-probe loop exits at its next check instead of stacking
/// with the reader a later Resume spawns.
#[cfg(not(coverage))]
pub(crate) fn cancel_reader(sid: &str) {
    if let Some(tok) = reader_cancels().lock().unwrap().get(sid) {
        tok.store(true, Ordering::SeqCst);
    }
}


/// One stateful wasit parser per reader, dispatched by driver. Turns raw CLI
/// stdout lines into `wasit_core::AgentEvent`s. Codex/opencode use a JSON-RPC
/// `MessageParser`; claude uses a stream-json `LineParser`.
///
/// This replaces Marsad's hand-rolled `codex_translate`/`acp_translate` +
/// stream-json passthrough: the driver-specific parsing now lives in the wasit
/// pure layer (`wasit_{claude,codex,opencode}`). Marsad keeps only the
/// transport/handshake/reconnect/backfill machinery around it.
#[cfg(not(coverage))]
enum WasitParser {
    Claude(wasit_claude::LineParser),
    Codex(wasit_codex::MessageParser),
    Opencode(wasit_opencode::MessageParser),
}

#[cfg(not(coverage))]
impl WasitParser {
    fn new(driver: &str) -> Self {
        match driver {
            "codex" => WasitParser::Codex(wasit_codex::MessageParser::new()),
            "opencode" => WasitParser::Opencode(wasit_opencode::MessageParser::new()),
            _ => WasitParser::Claude(wasit_claude::LineParser::new()),
        }
    }

    /// Parse one raw NDJSON line into zero or more `AgentEvent`s.
    fn on_raw(&mut self, raw: &str) -> Vec<wasit_core::AgentEvent> {
        match self {
            WasitParser::Claude(p) => p.on_line(raw),
            WasitParser::Codex(p) => p.on_message(raw),
            WasitParser::Opencode(p) => p.on_message(raw),
        }
    }

    /// Look up the real `(raw_id, method)` stored by the parser for the given
    /// `routing_id`. Returns `None` for Claude (no raw id needed). For Opencode,
    /// `method` is always the empty string (opencode gates have no method field).
    ///
    /// Must be called AFTER `on_raw` processes the gate line so the parser has
    /// already inserted the gate into its `pending_gates` map.
    fn pending_gate_raw(&self, routing_id: &str) -> Option<(serde_json::Value, String)> {
        match self {
            WasitParser::Claude(_) => None,
            WasitParser::Codex(p) => p
                .pending_gate(routing_id)
                .map(|g| (g.raw_id.clone(), g.method.clone())),
            WasitParser::Opencode(p) => p
                .pending_gate(routing_id)
                .map(|g| (g.raw_id.clone(), String::new())),
        }
    }
}

/// U1 (§3.2 trigger 4): observe the model a session launched with, from a
/// `SessionInit` event. This is the only model signal that exists for claude
/// (no list command) — accumulate it into the host's observed model set.
#[cfg(not(coverage))]
fn observe_session_init(
    ev: &wasit_core::AgentEvent,
    provider: Option<&std::sync::Arc<crate::capabilities::CapabilityProvider>>,
    host_id: &str,
) {
    if let wasit_core::AgentEvent::SessionInit {
        model: Some(model), ..
    } = ev
    {
        if let Some(provider) = provider {
            provider.observe_claude_model(host_id, model);
        }
    }
}

/// Capture a pending gate into Marsad's registry, storing the REAL raw JSON-RPC id
/// (and method for codex) from the wasit parser.
///
/// `parser_raw` is the result of `parser.pending_gate_raw(routing_id)` called
/// AFTER `on_raw` has processed the gate line. For codex/opencode it carries
/// `(raw_id, method)` from the parser's internal `pending_gates` map — the real
/// numeric id that was on the wire, not a value derived from the routing_id string.
/// For claude it is `None` and `acp_response_id` stays `None` (claude gates are
/// answered by routing_id, not by JSON-RPC id).
///
/// `acp_response_id` is stored as the CANONICAL JSON serialization of `raw_id`
/// (`serde_json::to_string`) so that `recover_codex_raw_id` /
/// `recover_opencode_raw_id` can round-trip it back to the correct `Value` (a
/// number stored as "7" parses back to `json!(7)`; a string id parses back as a
/// string). The old string-munging fallback is kept only for the `None` parser
/// case (should not happen for codex/opencode with a real gate).
#[cfg(not(coverage))]
fn capture_pending_gate(
    driver: &str,
    gate: &wasit_core::GateRequest,
    pending_fifo: &std::sync::Arc<
        std::sync::Mutex<HashMap<String, std::collections::VecDeque<PendingGate>>>,
    >,
    sid: &str,
    parser_raw: Option<(serde_json::Value, String)>,
) {
    use wasit_core::GateRequest;

    let routing_id = match gate {
        GateRequest::Permission { request_id, .. }
        | GateRequest::Plan { request_id, .. }
        | GateRequest::Question { request_id, .. } => request_id.clone(),
    };
    let elicitation = matches!(
        gate,
        GateRequest::Permission {
            is_elicitation: true,
            ..
        }
    );
    // The stored `input` is the raw tool input echoed back on approve (claude
    // can_use_tool `updatedInput`). For codex/opencode it is unused by the
    // encoders (they key on raw id/method); keep the gate's raw_input when present.
    let input = match gate {
        GateRequest::Permission { raw_input, .. } => raw_input.clone(),
        _ => serde_json::Value::Null,
    };

    // For codex/opencode: use the REAL raw_id from the parser (canonical JSON so
    // round-trip is exact). Fall back to the old string-munging only when the
    // parser didn't have the gate stored (should not happen in practice).
    let acp_response_id: Option<String> = if driver == "claude" {
        None
    } else if let Some((raw_id_val, _)) = &parser_raw {
        serde_json::to_string(raw_id_val).ok()
    } else {
        // Fallback: old string-munging (routing_id may be codex:<n> or <sid>:acp:<n>
        // for the legacy no-itemId case).
        if let Some(n) = routing_id.strip_prefix("codex:") {
            Some(n.to_string())
        } else if routing_id.contains(":acp:") {
            routing_id.rsplit(':').next().map(|s| s.to_string())
        } else {
            Some(routing_id.clone())
        }
    };

    // For codex: use the REAL method from the parser. Fall back to inference only
    // when the parser lookup missed (should not happen for real gates).
    let codex_method: Option<String> = if driver == "codex" {
        if let Some((_, method)) = &parser_raw {
            if !method.is_empty() {
                Some(method.clone())
            } else {
                // Parser returned empty method — infer from gate shape.
                Some(match gate {
                    GateRequest::Permission {
                        is_elicitation: true,
                        ..
                    } => "mcpServer/elicitation/request".to_string(),
                    GateRequest::Permission { tool_name, .. } => match tool_name.as_str() {
                        "apply_patch" => "item/fileChange/requestApproval".to_string(),
                        "permissions" => "item/permissions/requestApproval".to_string(),
                        _ => "item/commandExecution/requestApproval".to_string(),
                    },
                    GateRequest::Question { .. } => "item/tool/requestUserInput".to_string(),
                    GateRequest::Plan { .. } => "unknown".to_string(),
                })
            }
        } else {
            // Parser lookup missed — infer method from gate shape.
            Some(match gate {
                GateRequest::Permission {
                    is_elicitation: true,
                    ..
                } => "mcpServer/elicitation/request".to_string(),
                GateRequest::Permission { tool_name, .. } => match tool_name.as_str() {
                    "apply_patch" => "item/fileChange/requestApproval".to_string(),
                    "permissions" => "item/permissions/requestApproval".to_string(),
                    _ => "item/commandExecution/requestApproval".to_string(),
                },
                GateRequest::Question { .. } => "item/tool/requestUserInput".to_string(),
                GateRequest::Plan { .. } => "unknown".to_string(),
            })
        }
    } else {
        None
    };

    // The replay event stored on the pending gate is the bare `AgentEvent`
    // (`GateRequested`) the reader broadcasts, serialized to a Value so
    // `handle_subscribe` can deserialize it back and replay the gate to a client
    // that (re)connects after the one-shot broadcast.
    let event = serde_json::to_value(wasit_core::AgentEvent::GateRequested {
        gate: gate.clone(),
    })
    .unwrap_or_else(|_| serde_json::json!({}));

    let mut gates = pending_fifo.lock().unwrap();
    let queue = gates.entry(sid.to_string()).or_default();
    // Dedup by routing id ACROSS the whole queue: a flapping remote replays the
    // same gate on each reconnect; a genuinely new routing id is pushed to the
    // BACK (a turn can block on a burst of gates, each answered on its own id).
    if queue.iter().any(|g| g.request_id == routing_id) {
        eprintln!("marsad-agent: session {sid} gate DUPLICATE ignored ({routing_id})");
        return;
    }
    eprintln!(
        "marsad-agent: session {sid} gate captured ({routing_id}, {} now queued)",
        queue.len() + 1
    );
    queue.push_back(PendingGate {
        request_id: routing_id,
        input,
        elicitation,
        acp_response_id,
        codex_method,
        event,
    });
}

/// Drive the remote git Changes + project-usage probes off an `AgentEvent`.
///
/// Trigger 1: `TurnCompleted` (turn ended) → probe changes AND usage.
/// Trigger 2 (#4): a `ToolCallStarted` for a file-modifying tool (debounced) →
/// probe changes only. Remote sessions only (local sessions read changes/usage
/// locally in core). No-op when there is no host_target / project.
#[cfg(not(coverage))]
#[allow(clippy::too_many_arguments)]
fn maybe_probe_changes_usage(
    ev: &wasit_core::AgentEvent,
    host_target: Option<&str>,
    remote_project: Option<&str>,
    sid: &str,
    subs_fifo: &std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    last_changes_probe: &mut Option<std::time::Instant>,
    debounce_secs: u64,
) {
    let is_result = matches!(ev, wasit_core::AgentEvent::TurnCompleted { .. });
    let is_file_tool = agent_event_is_file_modifying_tool(ev);
    let should_probe = is_result
        || (is_file_tool && {
            let now = std::time::Instant::now();
            let ok = last_changes_probe
                .map(|t| now.duration_since(t).as_secs() >= debounce_secs)
                .unwrap_or(true);
            if ok {
                *last_changes_probe = Some(now);
            }
            ok
        });
    if !should_probe {
        return;
    }
    let (Some(target), Some(project)) = (host_target, remote_project) else {
        return;
    };

    // Changes probe (both triggers).
    let script = remote_changes_script(project);
    if let Some(stdout) = run_remote_script(target, &script) {
        let changes = parse_remote_changes(&stdout);
        let cu_line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::ChangesUpdate {
            session_id: sid.to_string(),
            changes,
        });
        if let Ok(subs) = subs_fifo.lock() {
            subs.iter().for_each(|s| {
                let _ = s.send(cu_line.clone());
            });
        }
        eprintln!(
            "marsad-agent: session {sid} changes update broadcast ({})",
            if is_result { "result" } else { "tool_use" }
        );
    }

    // Project-wide usage aggregation: result events only (reads all *.jsonl —
    // more expensive than the git probe).
    if is_result {
        let usage_script = remote_usage_script(project);
        if let Some(usage_out) = run_remote_script(target, &usage_script) {
            let trimmed = usage_out.trim();
            if !trimmed.is_empty() && trimmed != "{}" {
                if let Ok(usage_val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    let uu_line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::UsageUpdate {
                        session_id: sid.to_string(),
                        usage: usage_val,
                    });
                    if let Ok(subs) = subs_fifo.lock() {
                        subs.iter().for_each(|s| {
                            let _ = s.send(uu_line.clone());
                        });
                    }
                    eprintln!("marsad-agent: session {sid} usage update broadcast");
                }
            }
        }
    }
}

/// OPENCODE handshake response interception.
///
/// The opencode wasit `MessageParser` deliberately returns NO events for the
/// `session/new`/`session/load` RESPONSE (id==2) — it leaves SessionInit
/// synthesis to the transport owner. Marsad owns the handshake, so it detects
/// that response here and returns the events to emit (SessionInit plus an
/// optional ModesUpdated), storing the ACP sessionId and delivering the launch
/// model-pick then the first prompt. Returns `None` for any line that is NOT the
/// id==2 response (the caller then feeds the line to the parser as normal).
#[cfg(not(coverage))]
#[allow(clippy::too_many_arguments)]
fn opencode_handshake_intercept(
    raw: &str,
    is_first_attach: bool,
    first_prompt: Option<&str>,
    opencode_spec: Option<&crate::CachedOpencodeSpec>,
    acp_sessions: &std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    sid: &str,
    writer: &SharedWriter,
) -> Option<Vec<wasit_core::AgentEvent>> {
    let val: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    // Must be a RESPONSE (has id + result, no method) with id == 2.
    if val.get("method").is_some() {
        return None;
    }
    if val.get("id").and_then(|v| v.as_u64()) != Some(2) {
        return None;
    }
    let res = val.get("result")?;
    let acp_session_id = res.get("sessionId").and_then(|v| v.as_str())?.to_string();
    eprintln!("marsad-agent: session {sid} opencode ACP sessionId={acp_session_id}");
    acp_sessions
        .lock()
        .unwrap()
        .insert(sid.to_string(), acp_session_id.clone());

    // Synthesize SessionInit from the handshake result (mirrors the wasit
    // opencode driver's own handshake synthesis).
    let (available_modes, current_mode) = wasit_opencode::parse_config_modes(res);
    let model = wasit_opencode::parse_config_model(res)
        .or_else(|| opencode_spec.map(|s| s.model.clone()).filter(|m| !m.is_empty()));
    let cwd = opencode_spec.map(|s| s.project.clone());
    let mut events = vec![wasit_core::AgentEvent::SessionInit {
        native_session_id: Some(acp_session_id.clone()),
        model,
        cwd,
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        permission_mode: String::new(),
        claude_code_version: String::new(),
        available_modes: available_modes.clone(),
        current_mode: current_mode.clone(),
    }];
    if !available_modes.is_empty() || !current_mode.is_empty() {
        events.push(wasit_core::AgentEvent::ModesUpdated {
            available_modes,
            current_mode: if current_mode.is_empty() {
                None
            } else {
                Some(current_mode)
            },
        });
    }

    // First attach only: apply the launch model pick (opencode acp rejects
    // --model; the supported path is set_config_option AFTER session/new, done
    // BEFORE the first prompt so the first turn runs on the chosen model), then
    // deliver the first prompt as session/prompt.
    if is_first_attach {
        use std::io::Write;
        if let Some(spec) = opencode_spec {
            let want = spec.model.trim();
            if !want.is_empty() {
                let (current, valid) = opencode_launch_model_values(res);
                if valid.iter().any(|v| v == want) && current.as_deref() != Some(want) {
                    let line = wasit_opencode::encode_set_model(3, &acp_session_id, want);
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                    eprintln!("marsad-agent: session {sid} opencode applied launch model {want}");
                } else if !valid.is_empty() && !valid.iter().any(|v| v == want) {
                    eprintln!("marsad-agent: session {sid} opencode launch model {want:?} not in options — left at {current:?}");
                }
            }
        }
        if let Some(prompt) = first_prompt {
            let line = wasit_opencode::encode_session_prompt(4, &acp_session_id, prompt);
            let mut w = writer.lock().unwrap();
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
            let _ = w.flush();
            eprintln!(
                "marsad-agent: session {sid} opencode sent first_prompt as session/prompt ({} chars)",
                prompt.len()
            );
        }
    }

    Some(events)
}

/// Extract `(current_model, valid_models)` from an opencode `configOptions`
/// result — the `model` config group's `currentValue` + the `value`s of its
/// `options`. Used to guard the launch model-pick (only apply a real option that
/// differs from the current). Pure.
#[cfg(not(coverage))]
fn opencode_launch_model_values(res: &serde_json::Value) -> (Option<String>, Vec<String>) {
    let current = wasit_opencode::parse_config_model(res);
    let valid = res
        .get("configOptions")
        .and_then(|v| v.as_array())
        .and_then(|opts| {
            opts.iter()
                .find(|o| o.get("id").and_then(|v| v.as_str()) == Some("model"))
        })
        .and_then(|g| g.get("options"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (current, valid)
}

/// CODEX `collaborationMode/list` response interception.
///
/// The codex wasit `MessageParser` returns NO events for JSON-RPC RESPONSE lines
/// (the `{"id":N,"result":{…}}` shape). Marsad intercepts the id==5 response
/// (the `collaborationMode/list` request Marsad sends after `thread/started` to
/// discover what modes the session supports) and synthesizes a `ModesUpdated` event
/// from the result. Returns `None` for any line that is NOT the id==5 response.
///
/// Wire shape: `{"id":5,"result":{"data":[{"mode":"<id>","settings":{...}}, ...]}}`
#[cfg(not(coverage))]
fn codex_collab_mode_intercept(raw: &str, sid: &str) -> Option<Vec<wasit_core::AgentEvent>> {
    let val: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    // Must be a RESPONSE (has id + result, no method) with id == 5.
    if val.get("method").is_some() {
        return None;
    }
    if val.get("id").and_then(|v| v.as_u64()) != Some(5) {
        return None;
    }
    let result = val.get("result")?;
    let available_modes = wasit_codex::codex_modes_from_collab_list(result);
    eprintln!(
        "marsad-agent: session {sid} codex collaborationMode/list response → {} mode(s)",
        available_modes.len()
    );
    Some(vec![wasit_core::AgentEvent::ModesUpdated {
        available_modes,
        current_mode: None,
    }])
}

/// Spawn the per-session reader thread: connect the transport (local FIFO/pipe, or a
/// remote durable FIFO bridge), send the control `initialize` handshake + an optional
/// first prompt, register the writer, then stream claude's stdout to every subscriber
/// until the session ends. Used both when a session is launched AND when reconnecting
/// a session that survived an agent restart (pass `first_prompt: None` for reconnect).
///
/// # Remote durable model (design 2B)
///
/// A durable remote session uses two independent ssh/wsl children per agent connection:
///   - **Writer child**: runs `tee -a $HOME/.marsad/<sid>/in.fifo` with its stdin piped
///     from the agent.  The agent writes prompts + gate responses to it.  When the agent
///     drops the child's stdin pipe the `tee` exits, but the remote claude keeps running
///     because the keepalive holder (`sleep 999999 > in.fifo`, started at launch) still
///     holds the FIFO write-end open.
///   - **Reader child**: runs `tail -n0 --pid=<claude> -f $HOME/.marsad/<sid>/out.log`
///     with its stdout piped to the agent.  Streams only NEW output (lines written after
///     connect time) so reconnects don't replay historical output.  Historical
///     conversation is supplied by the `TranscriptBackfill` mechanism over a separate
///     `run_remote_script` call.
///
/// The writer child is stored in `children` so it can be reaped on thread exit.
/// Killing the session runs `remote_kill_script` over ssh (SIGTERM to ka+claude, dir
/// removal) rather than killing a child process.
///
/// # Auto-reconnect (connection-drop recovery)
///
/// When the reader stream ends for a REMOTE session, the thread probes liveness via
/// `remote_liveness_script` with `run_remote_script_bounded`.  If the probe itself
/// fails or times out (the connection/ControlMaster is still down), the thread retries
/// with exponential backoff.  When the connection comes back:
///   - Probe returns `LIVE` → old children are reaped, pid+dir are re-resolved, new
///     writer+reader children are spawned, TranscriptBackfill is re-run, and the read
///     loop resumes — all without any user action.
///   - Probe returns `DEAD` → claude truly exited (turn done / crashed) → normal
///     teardown (session finalized to Paused).
///   - Probe returns `Unreachable` → the host can't be reached; the durable session
///     is PRESUMED ALIVE, so the reader keeps probing at the steady backoff and
///     surfaces `Disconnected` (never a false Paused), flipping back to Connected
///     the moment the host answers LIVE. The thread exits only on cancel/stop or DEAD.
#[cfg(not(coverage))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_session_reader(
    sid: String,
    host_target: Option<String>,
    first_prompt: Option<String>,
    send_init: bool,
    run_command: Option<String>,
    transcript_hint: Option<(String, Option<String>)>,
    data_root: String,
    subs_fifo: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    writers_fifo: std::sync::Arc<std::sync::Mutex<HashMap<String, SharedWriter>>>,
    // Prompts queued (by handle_send_prompt) while this session had no registered
    // writer — flushed in order right after the writer registers below.
    pending_prompts: super::PendingPrompts,
    pending_fifo: std::sync::Arc<
        std::sync::Mutex<HashMap<String, std::collections::VecDeque<PendingGate>>>,
    >,
    children: std::sync::Arc<std::sync::Mutex<HashMap<String, std::process::Child>>>,
    // U1 (§3.2 trigger 4): passive claude-model observation. `None` for
    // reconnect paths that don't carry the Arc (never actually passed as
    // None today — kept Option so a future caller without a provider handy
    // degrades to a no-op rather than a forced dependency).
    capability_provider: Option<std::sync::Arc<crate::capabilities::CapabilityProvider>>,
    // Stage 2 codex: driver discriminator ("codex" or "claude").
    driver: String,
    // codex app-server: cached spec (model/project/posture/bin) for the reader's
    // and translate-and-broadcast). None for claude sessions.
    codex_spec: Option<crate::CachedCodexSpec>,
    // codex app-server: shared thread/turn state (thread_id from thread/start,
    // turn_id from turn/started). Shared with handle_send_prompt/handle_interrupt/
    // handle_set_session_config_option to address JSON-RPC requests.
    codex_threads: std::sync::Arc<std::sync::Mutex<HashMap<String, crate::CodexThread>>>,
    // opencode ACP: cached spec for the ACP handshake (initialize + session/new).
    // None for non-opencode sessions.
    opencode_spec: Option<crate::CachedOpencodeSpec>,
    // opencode ACP: shared map for storing the ACP sessionId from the session/new
    // (or session/load) result. Shared with handle_send_prompt / handle_interrupt /
    // handle_answer_gate so they can address subsequent ACP requests.
    acp_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    // Reader-liveness registry (see `LiveReaders`). This reader inserts its sid
    // here on spawn and removes it when it gives up (unless superseded), so the
    // resume guard knows whether a live reader owns the session.
    live_readers: super::LiveReaders,
) {
    // `run_command` is only used when the caller signals that a launch script
    // was not yet run (i.e. not provided to spawn_container).  For the durable
    // model, it's not needed here — the launch script already ran.
    let _ = run_command;

    // Register a fresh cancellation token for this session, tripping any prior
    // reader's token so a stale reconnect-probe loop (from a flapping host or a
    // prior resume) exits promptly instead of stacking with this new reader.
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    {
        let mut m = reader_cancels().lock().unwrap();
        if let Some(old) = m.insert(sid.clone(), cancel.clone()) {
            old.store(true, Ordering::SeqCst);
        }
    }
    // Mark this session as having a live reader (the resume guard's signal).
    // Idempotent with the synchronous insert in `launch_with_id`; also covers the
    // startup-reconnect path, which spawns readers without going through
    // `launch_with_id`. Removed on give-up below (unless superseded).
    live_readers.lock().unwrap().insert(sid.clone());

    let session_dir = std::path::PathBuf::from(&data_root)
        .join("sessions")
        .join(format!("marsad-{sid}"));
    std::thread::spawn(move || {
        use marsad_transport::Transport;

        // For REMOTE sessions: save the project path so the read loop can probe
        // git after each result event. We extract it from transcript_hint when
        // available; otherwise we have no project path to probe (skip silently).
        // U1 (§3.2 trigger 4): the capability-provider host key for this
        // session — "local" when host_target is unset, mirroring the
        // provider's own local/remote key convention (capabilities.rs).
        let capability_host_id = host_target.clone().unwrap_or_else(|| "local".to_string());

        let remote_project: Option<String> = if host_target.is_some() {
            transcript_hint.as_ref().map(|(p, _)| p.clone())
        } else {
            None
        };

        // ── Supervising reconnect loop ────────────────────────────────────────
        // For LOCAL sessions this loop runs exactly once (no reconnect logic).
        // For REMOTE sessions it may run multiple times: after the stream ends
        // we probe liveness and re-attach if the connection dropped but claude
        // is still alive on the remote.
        let mut is_first_attach = true;

        // Backoff state (remote reconnect only).
        let mut backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;

        // Per-session CONNECTION health (distinct from liveness). Emitted on REAL
        // stream events (`conn_on_drop` + the signal-driven `reconnect_action`):
        // attach → Connected; drop → Reconnecting/Unstable; sustained unreachable →
        // Disconnected; process gone → Paused. The UI uses it to show the true state
        // and HARD-BLOCK sending into a broken pipe.
        let emit_conn = |health: marsad_proto::ConnHealth| {
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionConn {
                session_id: sid.clone(),
                health,
            });
            if let Ok(subs) = subs_fifo.lock() {
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
        };

        // Emit a typed `wasit_core::AgentEvent` to subscribers. The wire now
        // carries the rich AgentEvent directly (Wave 0); the driver-specific
        // parsing lives in the wasit `LineParser`/`MessageParser`, and
        // `marsad-events` reduces over `AgentEvent`.
        let emit_event = |event: wasit_core::AgentEvent| {
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::AgentEvent {
                session_id: sid.clone(),
                event,
            });
            if let Ok(subs) = subs_fifo.lock() {
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
        };

        // ONE stateful wasit parser per reader, keyed by driver. Declared OUTSIDE
        // the reconnect loop so parser state (tool_use_id→name correlation, turn
        // flags, native thread/session ids) survives a reconnect — a subagent or
        // tool call can span a connection drop. `WasitParser` dispatches
        // `on_line`/`on_message` to the right concrete parser.
        let mut parser = WasitParser::new(&driver);

        'reconnect: loop {
            // Superseded (a newer reader took over) or stopped (paused) → exit now,
            // don't attach/re-probe.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }
            // Per-attempt flag: set by the reader child's stderr thread when it sees
            // a transport error. Read at stream-end to distinguish a network drop
            // (reattach) from a dead process (pause). Reset each iteration.
            let saw_conn_error =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            // ── Open and connect the transport ─────────────
            // Local session → platform FIFO/pipe transport
            // (tmux+FIFO on Unix, named pipe on Windows), yielding
            // a `SplitIo` (boxed writer + buffered boxed reader).
            //
            // Remote session (host_target resolves to an exec prefix) → DURABLE model:
            //   • Writer child: ssh/wsl running `tee -a in.fifo` (stdin piped from agent).
            //   • Reader child: ssh/wsl running `tail -n0 --pid=<cl> -f out.log` (stdout piped).
            // The writer child is stored in `children` so it can be reaped on exit.
            // The reader child is owned entirely by this thread.
            let (writer, mut reader): marsad_transport::SplitIo = match exec_prefix_for(
                host_target.as_deref(),
            ) {
                Some(prefix) => {
                    use std::process::{Command, Stdio};

                    // Resolve the session's ABSOLUTE dir + detached claude pid over
                    // the reliable stdin-pipe path, so the writer/reader commands can
                    // be built from literals (no $HOME/$()/quotes that Rust→wsl.exe/
                    // ssh.exe would mangle). If claude isn't running, bail — the
                    // session reader has nothing to attach to.
                    let host = host_target.clone().unwrap_or_default();
                    // Retry the resolve: on a host with heavy setup (e.g. `source
                    // ~/module.sh` on an HPC login node) claude can take many seconds
                    // to be up after the durable launch returns. A single resolve
                    // right after the launch's `sleep 0.3` loses that race and wrongly
                    // reports "claude not running" → the session sticks at Starting.
                    // Poll for a few seconds before giving up.
                    let resolved = {
                        let mut r = None;
                        for attempt in 0..12 {
                            if let Some(pd) = remote_resolve_pid_dir(&host, &sid) {
                                r = Some(pd);
                                break;
                            }
                            if attempt == 0 {
                                eprintln!(
                                    "marsad-agent: session {sid} claude not up yet — polling (slow remote setup?)"
                                );
                            }
                            std::thread::sleep(std::time::Duration::from_millis(600));
                        }
                        r
                    };
                    let (claude_pid, abs_dir) = match resolved {
                        Some(pd) => pd,
                        None => {
                            // Fetch the remote out.log tail so a genuine crash (claude
                            // not on PATH, setup error) is visible instead of a bare
                            // "not running".
                            let dir = remote_session_dir(&sid);
                            let log_tail = run_remote_script(
                                &host,
                                &format!("dir={dir}\ntail -c 4000 \"$dir/out.log\" 2>/dev/null"),
                            )
                            .unwrap_or_default();
                            eprintln!(
                                "marsad-agent: session {sid} remote resolve failed after retries (claude not running?) — not attaching. out.log tail:\n{log_tail}"
                            );
                            break 'reconnect;
                        }
                    };

                    // ── Spawn writer child ─────────────────────────────────────
                    // Appends everything written to its stdin into the remote FIFO.
                    let writer_argv = remote_writer_argv(&prefix, &abs_dir);
                    let w_bin = resolve_remote_bin(&writer_argv[0]);
                    let mut w_cmd = Command::new(&w_bin);
                    w_cmd
                        .args(&writer_argv[1..])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        w_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut writer_child = match w_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote writer spawn {sid}: {e}");
                            break 'reconnect;
                        }
                    };
                    let child_stdin = match writer_child.stdin.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote writer child {sid} has no stdin");
                            let _ = writer_child.kill();
                            let _ = writer_child.wait();
                            break 'reconnect;
                        }
                    };
                    // Store writer child so it is reaped on thread exit.
                    children.lock().unwrap().insert(sid.clone(), writer_child);

                    // ── Spawn reader child ─────────────────────────────────────
                    // Streams NEW out.log lines (tail -n0), EXCEPT an opencode ACP
                    // session's first attach, which must read from the start so the
                    // one-shot initialize/session/new responses (the ACP sessionId)
                    // are never missed by a late-attaching remote tail (see
                    // remote_reader_argv). Reconnects stay -n0 (no replay; sessionId
                    // already known).
                    let read_from_start = is_first_attach && driver == "opencode";
                    let reader_argv =
                        remote_reader_argv(&prefix, &claude_pid, &abs_dir, read_from_start);
                    let r_bin = resolve_remote_bin(&reader_argv[0]);
                    let mut r_cmd = Command::new(&r_bin);
                    r_cmd
                        .args(&reader_argv[1..])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        r_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut reader_child = match r_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote reader spawn {sid}: {e}");
                            // Clean up writer child we already stored.
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Stream reader's stderr to agent.log for diagnosis, AND flag a
                    // transport error so the post-drop classifier can tell a network
                    // drop from a dead process even in the rare case ssh exits 0.
                    // (The reader child's exit code is the primary signal; this is
                    // corroboration — see `reconnect_action`.)
                    if let Some(errpipe) = reader_child.stderr.take() {
                        let esid = sid.clone();
                        let flag = saw_conn_error.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let r = std::io::BufReader::new(errpipe);
                            for line in r.lines().map_while(Result::ok) {
                                if !line.trim().is_empty() {
                                    let l = line.to_ascii_lowercase();
                                    if l.contains("connection reset")
                                        || l.contains("closed by remote host")
                                        || l.contains("connection to")
                                        || l.contains("client_loop")
                                        || l.contains("broken pipe")
                                        || l.contains("timed out")
                                        || l.contains("connection refused")
                                    {
                                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    eprintln!("marsad-agent: session {esid} reader-stderr: {line}");
                                }
                            }
                        });
                    }
                    let child_stdout = match reader_child.stdout.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote reader child {sid} has no stdout");
                            let _ = reader_child.kill();
                            let _ = reader_child.wait();
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Reader child: store in the session_children registry under a
                    // distinct key so the read-loop cleanup can reap it.  We use a
                    // `"reader:<sid>"` key (never used by other code) so the normal
                    // kill path (keyed by plain sid) is unaffected.
                    let reader_key = format!("reader:{sid}");
                    children.lock().unwrap().insert(reader_key, reader_child);

                    let w: Box<dyn std::io::Write + Send> = Box::new(child_stdin);
                    let r: std::io::BufReader<Box<dyn std::io::Read + Send>> =
                        std::io::BufReader::new(Box::new(child_stdout));
                    (w, r)
                }
                None => {
                    #[cfg(unix)]
                    {
                        if driver == "codex" || driver == "opencode" {
                            // Codex eval-loop transport AND opencode ACP persistent
                            // process: both write to out.log (a regular file) rather
                            // than out.fifo, so we use the same in.fifo + out.log
                            // transport. For opencode, the process is persistent and
                            // writes ACP JSON-RPC lines continuously to out.log.
                            //
                            // We cannot use ClaudeFifoTransport here because opening
                            // out.fifo for reading would block until opencode's first
                            // write — which can't happen until after the ACP handshake,
                            // creating a deadlock. A regular file never blocks on open().
                            //
                            // Open in.fifo for writing: blocks until the tmux shell opens
                            // it for reading (which it does immediately on the `cat in.fifo |`
                            // side). We wait for the session dir to appear (tmux creates
                            // it when the session starts).
                            let in_fifo = session_dir.join("in.fifo");
                            let out_log = session_dir.join("out.log");

                            // Wait up to 15s for in.fifo to appear (tmux creates it
                            // when the session starts).
                            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                            loop {
                                if in_fifo.exists() {
                                    break;
                                }
                                if std::time::Instant::now() >= deadline {
                                    eprintln!("marsad-agent: {driver} session {sid}: in.fifo not created within 15s");
                                    break 'reconnect;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }

                            // Open in.fifo for writing (non-blocking: the cat in the
                            // shell is already waiting as a reader).
                            let in_file = match std::fs::OpenOptions::new()
                                .write(true)
                                .open(&in_fifo)
                            {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: {driver} session {sid}: open in.fifo: {e}");
                                    break 'reconnect;
                                }
                            };

                            // Ensure out.log exists (create it if not yet).
                            if !out_log.exists() {
                                let _ = std::fs::File::create(&out_log);
                            }

                            // Open out.log for reading. Since it's a regular file, this
                            // never blocks.
                            let out_file = match std::fs::File::open(&out_log) {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: {driver} session {sid}: open out.log: {e}");
                                    break 'reconnect;
                                }
                            };

                            eprintln!("marsad-agent: session {sid} {driver} transport connected (in.fifo + out.log)");
                            let w: Box<dyn std::io::Write + Send> = Box::new(in_file);
                            let r: std::io::BufReader<Box<dyn std::io::Read + Send>> =
                                std::io::BufReader::new(Box::new(out_file));
                            (w, r)
                        } else {
                            use marsad_transport::ClaudeFifoTransport;
                            let mut fifo = match ClaudeFifoTransport::open(&session_dir) {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: fifo open {sid}: {e}");
                                    break 'reconnect;
                                }
                            };
                            if let Err(e) = fifo.connect() {
                                eprintln!("marsad-agent: fifo connect {sid}: {e}");
                                break 'reconnect;
                            }
                            match fifo.into_split() {
                                Some(pair) => pair,
                                None => {
                                    eprintln!("marsad-agent: fifo not connected after connect {sid}");
                                    break 'reconnect;
                                }
                            }
                        }
                    }
                    #[cfg(windows)]
                    {
                        let _ = &session_dir;
                        use marsad_transport::ClaudeNamedPipeTransport;
                        let container_name = format!("marsad-{sid}");
                        let mut transport = ClaudeNamedPipeTransport::for_session(&container_name);
                        if let Err(e) = transport.connect() {
                            eprintln!("marsad-agent: pipe connect {sid}: {e}");
                            break 'reconnect;
                        }
                        match transport.into_split() {
                            Some(pair) => pair,
                            None => {
                                eprintln!("marsad-agent: pipe not connected after connect {sid}");
                                break 'reconnect;
                            }
                        }
                    }
                }
            };

            // The writer is shared (Arc<Mutex>) between this
            // thread (initialize handshake + first prompt) and
            // the serve thread (handle_send_prompt / answer_gate)
            // — boxed writers can't be try_clone'd like a File.
            let writer: SharedWriter = std::sync::Arc::new(std::sync::Mutex::new(writer));

            // Send the control-protocol `initialize` handshake first (LAUNCH only,
            // first attach only, CLAUDE ONLY). It pairs with `--permission-prompt-tool stdio` so
            // claude routes tool-permission prompts back as `can_use_tool`
            // control_requests. On RECONNECT the remote claude is already running +
            // initialized, so re-sending it would be wrong — hence `send_init` and
            // `is_first_attach`. Fire-and-forget: the CLI's ack is ignored.
            // CODEX: no initialize handshake — codex exec uses no control protocol.
            // OPENCODE: ACP handshake is sent below (initialize + session/new|session/load).
            if send_init && is_first_attach && driver != "codex" && driver != "opencode" {
                use std::io::Write;
                let init = wasit_claude::encode_initialize("req_1_init");
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(init.as_bytes());
                let _ = w.write_all(b"\n");
                let _ = w.flush();
                eprintln!("marsad-agent: session {sid} sent initialize handshake");
            }

            // ── opencode ACP handshake (LAUNCH only, first attach only) ───────
            // For a fresh opencode session: send `initialize` then `session/new`
            // (or `session/load` if we have a stored ACP sessionId for resume).
            // On reconnect: the remote process is already running and in-session;
            // DO NOT re-send the handshake — we just re-attach to the stream.
            // The ACP request IDs used here are the SAME ids the response-tracking
            // map below (`pending_acp_reqs`) uses to correlate responses.
            if driver == "opencode" && send_init && is_first_attach {
                use std::io::Write;

                // Numeric request ids: 1 = initialize, 2 = session/new|load. The
                // reader loop below correlates the id=2 RESPONSE to synthesize the
                // SessionInit (the opencode parser leaves that to the transport
                // owner) and to deliver the first prompt.
                let init_line =
                    wasit_opencode::encode_initialize("marsad", env!("CARGO_PKG_VERSION"));
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(init_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} opencode sent initialize");

                // session/new (fresh) or session/load (resume with a stored ACP
                // sessionId). cwd from the cached spec; model applied AFTER
                // session/new via set_config_option (opencode acp rejects --model).
                let cwd = opencode_spec
                    .as_ref()
                    .map(|s| s.project.clone())
                    .unwrap_or_else(|| ".".to_string());
                let session_line = match opencode_spec.as_ref().and_then(|s| s.acp_session_id.clone()) {
                    Some(acp_sid) => wasit_opencode::encode_session_load(2, &acp_sid, &cwd),
                    None => wasit_opencode::encode_session_new(2, &cwd, None),
                };
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(session_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} opencode sent session/new|load");
            }

            // ── codex app-server handshake (LAUNCH only, first attach only) ───
            // Persistent JSON-RPC: `initialize` (id 1) → `initialized`
            // (notification) → `thread/start` (id 2). thread/start carries the
            // project cwd + model + posture-derived approvalPolicy/sandbox so the
            // very first turn runs under the right settings. The thread id comes
            // back on the id==2 response (handled in the read loop), which then
            // delivers the first_prompt as a `turn/start`.
            if driver == "codex" && send_init && is_first_attach {
                use std::io::Write;
                let model = codex_spec.as_ref().map(|cs| cs.model.clone()).unwrap_or_default();
                let mode = codex_spec
                    .as_ref()
                    .map(|cs| cs.session_mode.clone())
                    .unwrap_or_default();
                let cwd = codex_spec
                    .as_ref()
                    .map(|cs| cs.project.clone())
                    .unwrap_or_else(|| ".".into());
                // id 1 = initialize, id 2 = thread/start. The thread id arrives on
                // the SessionInit the parser emits from the `thread/started`
                // notification; the reader loop delivers the first turn then.
                let init_line = wasit_codex::encode_initialize("marsad", env!("CARGO_PKG_VERSION"));
                let ts_line = wasit_codex::encode_thread_start(
                    2,
                    &cwd,
                    if model.is_empty() { None } else { Some(model.as_str()) },
                    &mode,
                );
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(init_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.write_all(ts_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} codex sent initialize + thread/start");
            }

            // Deliver the first_prompt (if any) immediately so the process receives its
            // initial task.  Only on the very first attach — on reconnect the process is
            // already running (a prompt was already sent) and re-sending would be wrong.
            //
            // CLAUDE: delivers as a stream-json user message on stdin.
            // CODEX: delivers first_prompt as a `turn/start` after thread/start;
            //        the eval-loop container spawns `codex exec …` as a subprocess.
            // OPENCODE: first_prompt is delivered as a `session/prompt` ACP request
            //           AFTER the session/new result arrives (so the ACP sessionId is
            //           known). The read loop below handles this delivery on the
            //           session/new response. We skip the first-prompt delivery here
            //           for opencode.
            // CODEX: like opencode, the first_prompt is delivered as a `turn/start`
            // AFTER the thread/start result arrives (so the threadId is known); the
            // read loop handles that on the id==2 response. Skip here.
            if is_first_attach && driver != "opencode" && driver != "codex" {
                if let Some(ref prompt) = first_prompt {
                    use std::io::Write;
                    let plen = prompt.len();
                    {
                        // Claude: stream-json user message (built by the wasit encoder).
                        let encoded = wasit_claude::encode_user_message(prompt);
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(encoded.as_bytes());
                        let _ = w.write_all(b"\n");
                        let _ = w.flush();
                        eprintln!("marsad-agent: session {sid} delivered first_prompt ({plen} chars) to claude stdin");
                    }
                } else {
                    eprintln!("marsad-agent: session {sid} no first_prompt — process idle until a prompt is sent (phase stays Starting)");
                }
            }

            // Register the shared writer so handle_send_prompt
            // / handle_answer_gate can deliver future prompts and
            // gate responses to this session's stdin. Both sides
            // hold an Arc to the same Mutex<Box<dyn Write>>.
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.insert(sid.clone(), std::sync::Arc::clone(&writer));
            }

            // Flush any prompts that arrived while the writer wasn't registered yet
            // (queued by handle_send_prompt during this (re)attach window). Deliver
            // in order; `drain_fresh_prompts` drops stale ones (TTL) so a prompt
            // typed long before a manual resume isn't fired out of context. Held
            // under the pending_prompts lock so a concurrent send_prompt either
            // queues before the drain (delivered here) or finds the writer already
            // registered (delivered directly) — never lost, never duplicated.
            {
                let queued = { pending_prompts.lock().unwrap().remove(&sid).unwrap_or_default() };
                if !queued.is_empty() {
                    use std::io::Write;
                    let fresh = super::drain_fresh_prompts(queued, std::time::Instant::now());
                    let mut w = writer.lock().unwrap();
                    let mut delivered = 0usize;
                    for bytes in &fresh {
                        if w.write_all(bytes).and_then(|_| w.flush()).is_ok() {
                            delivered += 1;
                        }
                    }
                    if delivered > 0 {
                        eprintln!(
                            "marsad-agent: session {sid} flushed {delivered} queued prompt(s) on (re)attach"
                        );
                    }
                }
            }

            // For a REMOTE resume/reconnect, fetch the prior conversation from the
            // remote host's `.jsonl` over a fresh connection (which reuses the
            // session's ControlMaster / plink-share, so no extra MFA), and broadcast
            // it as a `TranscriptBackfill` so core can show the history. The remote
            // file is unreachable by core's local JSONL reader, so this is the only
            // way a resumed WSL/SSH session shows its past turns. Done before the read
            // loop so the backfill precedes any live events.
            // On reconnect after a connection drop (is_first_attach == false) we do
            // NOT re-run the backfill: the transcript already streamed; re-running
            // it would duplicate all previous turns in the UI.
            //
            // OPENCODE: skip this entirely. This backfill reads claude's on-disk
            // transcript (`~/.claude/projects/<project>/<uuid>.jsonl`), which an
            // opencode session does NOT own — and when a claude AND an opencode
            // session share a project dir, it would splice the CLAUDE session's
            // transcript under the opencode session (opencode header + claude
            // transcript — a real cross-wire bug). opencode history comes from the
            // ACP stream (session/load replay on resume, live session/update events).
            if is_first_attach && driver != "opencode" {
                if let Some((project, uuid)) = &transcript_hint {
                    if let Some(target) = host_target.as_deref() {
                        let script = remote_transcript_script(project, uuid.as_deref());
                        // run_remote_script pipes the script via stdin to `bash -l -s` —
                        // login shell (nested wsl.exe keeps $HOME), no argv quoting, and
                        // CREATE_NO_WINDOW so ssh.exe doesn't flash a console.
                        if let Some(jsonl) = run_remote_script(target, &script) {
                            if !jsonl.trim().is_empty() {
                                let line = marsad_proto::encode_ipc(
                                    &marsad_proto::IpcMsg::TranscriptBackfill {
                                        session_id: sid.clone(),
                                        jsonl: jsonl.clone(),
                                    },
                                );
                                let subs = subs_fifo.lock().unwrap();
                                for s in subs.iter() {
                                    let _ = s.send(line.clone());
                                }
                                eprintln!(
                                    "marsad-agent: session {sid} transcript backfill ({} bytes) broadcast",
                                    jsonl.len()
                                );
                            } else {
                                eprintln!("marsad-agent: session {sid} transcript backfill: remote .jsonl empty/absent");
                            }
                        }
                    }
                }
            }

            // For a REMOTE first-attach, probe full-project usage AFTER the transcript
            // backfill so the result supersedes the partial per-session value the
            // backfill set.  Best-effort: if the output is empty or the script fails,
            // broadcast nothing (do not send an empty UsageUpdate — the empty-guard in
            // core would no-op it, but skipping the broadcast is cleaner and avoids
            // the network round-trip).
            if is_first_attach {
                if let (Some(target), Some(project)) =
                    (host_target.as_deref(), remote_project.as_deref())
                {
                    let usage_script = remote_usage_script(project);
                    if let Some(usage_out) = run_remote_script(target, &usage_script) {
                        let trimmed = usage_out.trim();
                        if !trimmed.is_empty() && trimmed != "{}" {
                            if let Ok(usage_val) =
                                serde_json::from_str::<serde_json::Value>(trimmed)
                            {
                                let uu_line =
                                    marsad_proto::encode_ipc(&marsad_proto::IpcMsg::UsageUpdate {
                                        session_id: sid.clone(),
                                        usage: usage_val,
                                    });
                                let subs = subs_fifo.lock().unwrap();
                                subs.iter().for_each(|s| {
                                    let _ = s.send(uu_line.clone());
                                });
                                eprintln!(
                                    "marsad-agent: session {sid} usage update broadcast (attach)"
                                );
                            }
                        }
                    }
                }
            }

            is_first_attach = false;

            // Read out.log tail lines and broadcast to all subscribers. The reader
            // blocks here; the writer clone is in the session_writers registry for
            // handle_send_prompt — no lock contention.
            eprintln!("marsad-agent: session {sid} stream connected");
            // Two-axis lifecycle: attaching to a running session ⇒ Resumed. Tell
            // core authoritatively (it starts restored sessions Paused until this
            // lands). Emitted on every (re)connect so a post-drop reconnect
            // re-asserts liveness.
            {
                eprintln!("marsad-agent: session {sid} liveness -> LIVE (broadcast)");
                let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLiveness {
                    session_id: sid.clone(),
                    live: true,
                });
                let subs = subs_fifo.lock().unwrap();
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
            // A successful (re)attach means we have a clean stream RIGHT NOW →
            // Connected. This always clears any prior Reconnecting/Unstable/
            // Disconnected — self-healing, so an idle-after-flap session can never
            // get stuck on "Unstable".
            emit_conn(marsad_proto::ConnHealth::Connected);
            use std::io::BufRead;
            let mut lines_seen = 0u64;
            // Debounce tracker for the tool_use-triggered Changes probe (#4).
            // Stores the Instant of the last probe so rapid edits don't spam ssh
            // (at most one probe per CHANGES_PROBE_DEBOUNCE_SECS per session).
            let mut last_changes_probe: Option<std::time::Instant> = None;
            const CHANGES_PROBE_DEBOUNCE_SECS: u64 = 3;
            loop {
                let mut raw = String::new();
                match reader.read_line(&mut raw) {
                    Ok(0) => {
                        // For codex/opencode sessions reading from out.log (a regular
                        // file), EOF means "no more data right now" — the process is
                        // still running (between ACP messages / turns), so we poll-sleep
                        // rather than breaking. We detect session liveness by checking
                        // if in.fifo still exists (it disappears when the tmux session
                        // exits). For non-codex/opencode (FIFO/pipe) sessions, EOF
                        // always means done.
                        if driver == "codex" || driver == "opencode" {
                            let in_fifo = session_dir.join("in.fifo");
                            if in_fifo.exists() {
                                // Process still running, waiting for more output.
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                continue;
                            }
                            // Process exited (in.fifo gone): real EOF.
                            eprintln!(
                                "marsad-agent: session {sid} {driver} reader done (process exited) after {lines_seen} line(s)"
                            );
                        } else {
                            eprintln!(
                                "marsad-agent: session {sid} reader EOF after {lines_seen} line(s)"
                            );
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("marsad-agent: session {sid} reader ERROR after {lines_seen} line(s): {e}");
                        break;
                    }
                    Ok(_) => {
                        lines_seen += 1;
                        let raw = raw.trim_end_matches('\n');

                        // ── Unified wasit parse → emit path ──────────────────
                        // Feed EVERY inbound line (including handshake responses)
                        // to the ONE stateful wasit parser for this driver. It
                        // turns the raw CLI stdout line into zero or more typed
                        // `wasit_core::AgentEvent`s — gate extraction, tool
                        // correlation, usage, mode/model, compaction, subagent
                        // grouping all live inside the wasit driver now.
                        //
                        // OPENCODE handshake interception: the opencode parser
                        // deliberately does NOT synthesize `SessionInit` from the
                        // `session/new`/`session/load` RESPONSE (it leaves that to
                        // the transport owner). Marsad owns the handshake, so it
                        // parses the id==2 response itself → SessionInit +
                        // ModesUpdated + model-apply + first-prompt delivery.
                        if driver == "opencode" {
                            if let Some(evs) = opencode_handshake_intercept(
                                raw,
                                is_first_attach,
                                first_prompt.as_deref(),
                                opencode_spec.as_ref(),
                                &acp_sessions,
                                &sid,
                                &writer,
                            ) {
                                for ev in evs {
                                    observe_session_init(
                                        &ev,
                                        capability_provider.as_ref(),
                                        &capability_host_id,
                                    );
                                    emit_event(ev);
                                }
                                continue;
                            }
                        }

                        // CODEX collaborationMode/list response interception.
                        // The codex MessageParser returns empty for all JSON-RPC
                        // RESPONSE lines (by design — it only handles notifications
                        // and server-initiated requests). We intercept the id==5
                        // response here and emit ModesUpdated with the real mode
                        // list, mirroring the opencode_handshake_intercept above.
                        if driver == "codex" {
                            if let Some(evs) = codex_collab_mode_intercept(raw, &sid) {
                                for ev in evs {
                                    emit_event(ev);
                                }
                                continue;
                            }
                        }

                        let events = parser.on_raw(raw);
                        for ev in events {
                            // Native id capture (from SessionInit) → store for
                            // encodes + storage, then deliver a deferred first
                            // prompt (codex/opencode need the native id first).
                            if let wasit_core::AgentEvent::SessionInit {
                                ref native_session_id,
                                ..
                            } = ev
                            {
                                if let Some(nid) = native_session_id.clone() {
                                    if driver == "codex" {
                                        codex_threads.lock().unwrap().insert(
                                            sid.clone(),
                                            crate::CodexThread {
                                                thread_id: nid.clone(),
                                                turn_id: None,
                                            },
                                        );
                                        // Deliver the first turn now that the
                                        // thread id is known (first attach only).
                                        if is_first_attach {
                                            if let Some(ref prompt) = first_prompt {
                                                use std::io::Write;
                                                let line =
                                                    wasit_codex::encode_turn_start(3, &nid, prompt);
                                                let mut w = writer.lock().unwrap();
                                                let _ = w.write_all(line.as_bytes());
                                                let _ = w.write_all(b"\n");
                                                let _ = w.flush();
                                                eprintln!("marsad-agent: session {sid} codex sent first turn/start ({} chars)", prompt.len());
                                            }
                                            // Query the collaboration modes so the
                                            // Config "Session mode" UI can render.
                                            use std::io::Write;
                                            let ml = wasit_codex::encode_collab_mode_list(5, &nid);
                                            let mut w = writer.lock().unwrap();
                                            let _ = w.write_all(ml.as_bytes());
                                            let _ = w.write_all(b"\n");
                                            let _ = w.flush();
                                        }
                                    } else if driver == "opencode" {
                                        acp_sessions
                                            .lock()
                                            .unwrap()
                                            .insert(sid.clone(), nid.clone());
                                    }
                                }
                            }

                            // Capture a gate into Marsad's pending-gate registry so
                            // the answer path can encode a response to the right
                            // native routing id. We look up the REAL raw_id (and
                            // method for codex) from the wasit parser BEFORE capture
                            // so the stored acp_response_id is the canonical JSON of
                            // the actual wire id rather than a string-munged guess.
                            if let wasit_core::AgentEvent::GateRequested { ref gate } = ev {
                                let routing_id = match gate {
                                    wasit_core::GateRequest::Permission { request_id, .. }
                                    | wasit_core::GateRequest::Plan { request_id, .. }
                                    | wasit_core::GateRequest::Question { request_id, .. } => {
                                        request_id.clone()
                                    }
                                };
                                let parser_raw = parser.pending_gate_raw(&routing_id);
                                capture_pending_gate(
                                    &driver,
                                    gate,
                                    &pending_fifo,
                                    &sid,
                                    parser_raw,
                                );
                            }

                            // U1: observe the model claude/opencode launched with.
                            observe_session_init(
                                &ev,
                                capability_provider.as_ref(),
                                &capability_host_id,
                            );

                            // Mid-turn / end-of-turn remote git Changes + usage probe.
                            maybe_probe_changes_usage(
                                &ev,
                                host_target.as_deref(),
                                remote_project.as_deref(),
                                &sid,
                                &subs_fifo,
                                &mut last_changes_probe,
                                CHANGES_PROBE_DEBOUNCE_SECS,
                            );

                            emit_event(ev);
                        }
                    }
                }
            }

            eprintln!("marsad-agent: session {sid} stream ended after {lines_seen} line(s)");
            // If claude produced NO output on a remote session, run a diagnosis so the
            // agent.log gives a clue (PATH, module, etc.).
            if lines_seen == 0 {
                if let Some(target) = host_target.as_deref() {
                    // Dump claude's actual out.log — claude's stderr/error is redirected
                    // there (start.sh: `>> out.log 2>&1`), but the reader's `tail -n0`
                    // skips pre-existing content, so a startup error (bad flag, crash,
                    // `--continue` with no session, etc.) never reaches the stream. This
                    // surfaces the REAL reason claude exited, plus a PATH check.
                    let dir = remote_session_dir(&sid);
                    let script = format!(
                        "echo '== claude =='; command -v claude 2>/dev/null || echo 'claude NOT on PATH'; \
                         echo '== out.log (claude startup output/error) =='; \
                         tail -c 2000 {dir}/out.log 2>/dev/null"
                    );
                    if let Some(out) = run_remote_script(target, &script) {
                        eprintln!(
                            "marsad-agent: session {sid} remote diagnosis:\n{}",
                            out.trim()
                        );
                    }
                }
            }

            // ── Teardown for this connection attempt ─────────────────────────
            // Remove writer from registry and reap children.  For remote sessions
            // we may re-attach below (reconnect loop), so the registry slot will be
            // re-inserted on the next iteration; for local sessions and definitive
            // exits the slot stays absent (correct).
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.remove(&sid);
            }
            drop(writer);
            // Reap the writer child (remote sessions: the `tee -a in.fifo` child).
            // Dropping the writer above closes the child's stdin; `wait()` reaps it.
            // Local sessions never insert into `children`, so this is a no-op for them.
            if let Some(mut child) = children.lock().unwrap().remove(&sid) {
                let _ = child.wait();
            }
            // Reap the reader child (remote sessions: the `ssh … tail --pid -f`
            // child) stored under the `"reader:<sid>"` key. Capture its exit code
            // FIRST — it's the authoritative signal for why the stream ended (ssh
            // 255 = transport drop; the remote `tail --pid` exiting 0 = the watched
            // process died). The stream already EOF'd, so the child has normally
            // exited on its own; `try_wait` reads that. Only if it's somehow still
            // running (e.g. a half-open connection) do we kill it — and a kill means
            // it did NOT exit cleanly, i.e. treat as a transport drop, not death.
            let reader_exit_code: Option<i32> = {
                let reader_key = format!("reader:{sid}");
                let child = children.lock().unwrap().remove(&reader_key);
                if let Some(mut rchild) = child {
                    match rchild.try_wait() {
                        Ok(Some(status)) => status.code(),
                        _ => {
                            // Still running (or unwaitable): kill it. A killed reader
                            // is NOT a clean exit → don't let it read as "process
                            // died"; None routes to Reattach in `reconnect_action`.
                            let _ = rchild.kill();
                            let _ = rchild.wait();
                            None
                        }
                    }
                } else {
                    None
                }
            };

            // ── Remote reconnect: classify the stream-end, then reattach or stop ─
            // Only for REMOTE sessions. Local sessions (host_target None) stop here.
            if !matches!(host_target.as_deref(), Some(h) if exec_prefix_for(Some(h)).is_some()) {
                break 'reconnect;
            }

            // A COMMANDED stop (pause / supersede) is NOT a connection blip. The
            // kill that ends a pause EOFs this stream too, so if the cancel token is
            // already set the drop we just saw is that intentional kill — exit
            // WITHOUT emitting Reconnecting/Unstable (which would paint a paused
            // session "reconnecting"). handle_stop_session sets the token before the
            // kill, so this check is deterministic; a real SSH drop leaves it unset.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }

            // Decide from the reader child's OWN exit signal — NOT a fixed-timeout
            // probe. A clean `tail --pid` exit (code 0, no connection error) means
            // the watched process died → finalize (Pause). Any transport error
            // (ssh 255 / a captured "connection reset" / "closed by remote host")
            // means the connection dropped, not the process → reattach. This is the
            // fix for slow hosts (ETX: 4–26s SSH round-trip) where an 8s probe timed
            // out and stranded a LIVE session on Disconnected for ~25 min.
            let saw_err = saw_conn_error.load(Ordering::SeqCst);
            match reconnect_action(reader_exit_code, saw_err) {
                ReconnectAction::Pause => {
                    eprintln!(
                        "marsad-agent: session {sid} reader exited cleanly (code {reader_exit_code:?}) — process gone, finalizing"
                    );
                    break 'reconnect;
                }
                ReconnectAction::Reattach => {
                    // A connection that was actually working (produced lines) then
                    // dropped is a fresh blip → reset the backoff so we retry fast. A
                    // reattach that never streamed keeps the backoff growing, so a
                    // genuinely-unreachable host escalates Reconnecting → Disconnected
                    // (via the SAME state machine) — but is NEVER falsely Paused.
                    if lines_seen > 0 {
                        backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
                    }
                    let backoff_at_max = backoff_secs >= RECONNECT_MAX_BACKOFF_SECS;
                    let health = if backoff_at_max {
                        marsad_proto::ConnHealth::Disconnected
                    } else {
                        conn_on_drop(lines_seen)
                    };
                    // Cancel-guard: if a pause/supersede raced in during teardown, a
                    // fresh reader may have already emitted Connected — don't clobber
                    // it with a stale Reconnecting/Disconnected.
                    if !cancel.load(Ordering::SeqCst) {
                        emit_conn(health);
                    }
                    eprintln!(
                        "marsad-agent: session {sid} connection dropped (exit {reader_exit_code:?}, conn_err={saw_err}) — reattaching in {backoff_secs}s"
                    );
                    // Backoff before the reattach, in 1s increments so a cancel
                    // (pause/supersede) is honored within ~1s.
                    for _ in 0..backoff_secs {
                        if cancel.load(Ordering::SeqCst) {
                            break 'reconnect;
                        }
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    backoff_secs = reconnect_backoff_next(backoff_secs);
                    continue 'reconnect; // re-open the transport + reader
                }
            }
        } // 'reconnect loop

        // Whether THIS reader was superseded/stopped (vs it giving up on its own).
        let was_cancelled = cancel.load(Ordering::SeqCst);
        // De-register our token if it's still the current one (a newer reader may
        // have already replaced it — don't clobber that).
        {
            let mut m = reader_cancels().lock().unwrap();
            if m.get(&sid)
                .map(|t| std::sync::Arc::ptr_eq(t, &cancel))
                .unwrap_or(false)
            {
                m.remove(&sid);
                // This reader is still the current one for the session (NOT
                // superseded by a fresh reader that took over) and it has given up
                // (re)attaching. Clear the liveness marker so a subsequent resume
                // relaunches (re-spawning a reader, which drains any queued
                // prompts) instead of no-op'ing forever. A superseded reader must
                // NOT clear it — the newer reader owns the session's liveness.
                live_readers.lock().unwrap().remove(&sid);
            }
        }

        // Two-axis lifecycle: this reader has given up (re)attaching. For a
        // REMOTE session that means the process is gone or unreachable — claude
        // exited, the host rebooted / OOM-killed / scheduler-killed it, or the
        // reconnect cap was hit. Tell core so the card flips to Paused instead of
        // lying with its last activity state (Gap B: host-death → Paused). Local
        // sessions manage liveness elsewhere; skip here to avoid a false Paused
        // on a still-live local session. Also SKIP when we were cancelled: a
        // supersede means a newer reader owns liveness (it will broadcast LIVE) and
        // a stop already set Paused in core — a stray PAUSED here would race and
        // wrongly flip the fresh session Paused.
        if exec_prefix_for(host_target.as_deref()).is_some() && !was_cancelled {
            eprintln!("marsad-agent: session {sid} liveness -> PAUSED (reader gave up; broadcast)");
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLiveness {
                session_id: sid.clone(),
                live: false,
            });
            let subs = subs_fifo.lock().unwrap();
            for s in subs.iter() {
                let _ = s.send(line.clone());
            }
        }

        // Final cleanup: ensure the writer registry slot is gone (it may already
        // be absent if the reconnect loop tore it down; the remove is idempotent).
        {
            let mut registry = writers_fifo.lock().unwrap();
            registry.remove(&sid);
        }
        // Do NOT wipe still-queued prompts here. This reader gave up, but the host
        // session is durable — a later resume spawns a fresh reader that will flush
        // this same (session-keyed) queue on attach, so anything the user sent must
        // survive. Wiping it here was a silent-loss bug: a prompt sent during a long
        // outage vanished when the reader hit its cap. The queue is bounded
        // (PENDING_PROMPT_MAX) so it can't grow without limit.
    });
}

