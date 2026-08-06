//! Session lifecycle and interaction — launch/restart/resume/attach/stop/remove,
//! shell injection, prompt/interrupt, gate & question answers, slash commands,
//! workspace switching, plus startup session restore (`load_sessions`). Extracted
//! from `lib.rs` verbatim; `impl Core` is reopened and `use super::*` pulls in the
//! crate types and the root module's private plumbing.

use super::*;

impl Core {
    // -----------------------------------------------------------------------
    // Session commands
    // -----------------------------------------------------------------------

    pub fn launch_session(&mut self, spec: LaunchSpec) -> Result<String, CoreError> {
        let id = new_session_id();
        self.launch_session_with_id(id, spec)
    }

    /// Build the launch-time VmUpdates for a just-launched session id: its
    /// initial `SessionView` (Phase::Starting) plus the derived Overview + Board.
    /// `launch_session`/`launch_wasl_session` return only the id (the caller needs
    /// it to select the session), so the bridge command emits THESE so the new
    /// session appears immediately on every subscriber — the board, the sidebar,
    /// and the session view — without waiting for the backend's first stream
    /// event. claude/codex emit nothing until prompted, so without this a fresh
    /// session was invisible (and its optimistic placeholder never retired → the
    /// "Loading transcript…" that never cleared, #2/#4/#8). Empty for an unknown
    /// id (a defensive no-op, never a panic).
    pub fn session_launch_updates(&self, id: &str) -> Vec<VmUpdate> {
        match self.sessions.get(id) {
            Some(rec) => {
                let sv = self.make_session_view(id, rec);
                let snap = self.snapshot();
                vec![
                    VmUpdate::Session(sv),
                    VmUpdate::Overview(marsad_viewmodel::group_overview(&snap)),
                    VmUpdate::Board(self.board()),
                ]
            }
            None => vec![],
        }
    }

    /// X6 (Wasl Backend, §6.1): launch a `WaslManaged` session — one whose
    /// underlying process is owned END-TO-END by Wasl. Emits `LaunchWaslSession`
    /// (NOT `LaunchSession` — no container/argv semantics apply), registers a
    /// `RuntimeOwner::WaslManaged` record, and PERSISTS the `WaslSessionBinding`
    /// (as its own store record) so a fresh `Core` over the same backend can
    /// reconnect after a restart. Returns the Marsad session id.
    pub fn launch_wasl_session(
        &mut self,
        host_id: &str,
        wasl_target_key: serde_json::Value,
        name: Option<String>,
    ) -> Result<String, CoreError> {
        let id = new_session_id();

        // Drive the agent: build the WaslSessionClient behind its registry.
        self.send_ipc(&IpcMsg::LaunchWaslSession {
            session_id: id.clone(),
            host_id: host_id.to_string(),
            wasl_target_key: wasl_target_key.clone(),
            name: name.clone(),
        })?;

        let binding = WaslSessionBinding {
            // target_id is Wasl-assigned and learned on connect; unknown at launch
            // time (the provider maps the key → a target inside the agent). Empty
            // until a status/event surfaces it; the provider_key is what reconnect
            // actually keys on.
            target_id: String::new(),
            host_id: host_id.to_string(),
            provider_key: wasl_target_key.clone(),
        };

        // Persist a session record + the Wasl binding so a fresh Core restores
        // this as WaslManaged (routing the `*Wasl*` family) and can reconnect.
        // A WaslManaged session has no claude container/argv; the persisted
        // `driver` is a display marker ("wasl") and every claude-only field is
        // empty/default.
        let custom_name = name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let persisted_name = custom_name.clone().unwrap_or_else(|| id.clone());
        let persisted = marsad_store::PersistedSession {
            id: id.clone(),
            session_id: None,
            name: persisted_name,
            host_id: host_id.to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: String::new(),
            branch: "main".to_string(),
            model: String::new(),
            last_phase: "Starting".to_string(),
            mode: None,
            disallowed_tools: Vec::new(),
            driver: "wasl".to_string(),
            slash_commands: Vec::new(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
        };
        // Best-effort: a store error must not fail the launch (the session is
        // already live in-memory + driven over IPC); reconnect degrades but the
        // running session is unaffected. Mirrors the store-write disposition
        // elsewhere in this file.
        let _ = self.store.upsert_session(&persisted);
        let _ = self
            .store
            .upsert_wasl_binding(&marsad_store::PersistedWaslBinding {
                session_id: id.clone(),
                target_id: binding.target_id.clone(),
                host_id: binding.host_id.clone(),
                provider_key: binding.provider_key.clone(),
                last_seq: 0,
            });

        let mut state = SessionState::initial();
        state.phase = Phase::Starting;
        self.session_order.push(id.clone());
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: String::new(),
                host_id: host_id.to_string(),
                state,
                changes: Vec::new(),
                transcript: Vec::new(),
                seen_msg_ids: std::collections::HashSet::new(),
                name: custom_name,
                shell_running: false,
                // Display driver only — the real driver is Wasl's (§6.1).
                driver: "wasl".to_string(),
                live: true,
                conn_health: marsad_proto::ConnHealth::Connected,
                runtime_owner: RuntimeOwner::WaslManaged { binding },
                wasl_last_seq: 0,
                wasl_seen_seqs: std::collections::HashSet::new(),
                wasl_open_assistant: None,
                open_assistant: None,
                wasl_turn_closed: false,
                wasl_native_gates: HashMap::new(),
            },
        );

        // Auto-create the single-node fast-lane workstream, mirroring
        // launch_session_with_id, so a WaslManaged session shows on the board too.
        if !self.membership.contains_key(&id) {
            let record = self.sessions.get(&id).unwrap();
            let ws_name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name: ws_name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id.clone());
            let _ = self.store.set_membership(&self.membership);
        }

        Ok(id)
    }

    /// X6 (§6.1): the runtime owner of `id`. Used by the centralized routing
    /// methods and `answer_gate` to pick the MarsadManaged vs WaslManaged path.
    fn runtime_owner(&self, id: &str) -> RuntimeOwner {
        self.sessions
            .get(id)
            .map(|r| r.runtime_owner.clone())
            .unwrap_or_default()
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "attach" happens
    /// in EXACTLY ONE place. MarsadManaged → today's `AttachSession`; WaslManaged
    /// → `AttachWaslSession { after_seq = wasl_last_seq }` (re-pump from the last
    /// observed seq — the reconnect contract, §6.1 step 2).
    fn attach_session_events(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::AttachSession {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => {
                let after_seq = self.sessions.get(id).map(|r| r.wasl_last_seq).unwrap_or(0);
                self.send_ipc(&IpcMsg::AttachWaslSession {
                    session_id: id.to_string(),
                    after_seq,
                })
            }
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "interrupt".
    fn interrupt_session_routed(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::Interrupt {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::InterruptWaslSession {
                session_id: id.to_string(),
            }),
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "stop".
    fn stop_session_routed(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::StopSession {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::StopWaslSession {
                session_id: id.to_string(),
                drain: false,
            }),
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "send prompt".
    /// WaslManaged mints a fresh UUID idempotency key per human send (§4.4/§12 Q5)
    /// and defaults to the Queue busy policy (§12 Q4).
    fn send_session_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::SendPrompt {
                session_id: id.to_string(),
                text: text.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::SendWaslPrompt {
                session_id: id.to_string(),
                text: text.to_string(),
                idempotency_key: marsad_wasl_session::new_idempotency_key(),
                busy_policy: "queue".to_string(),
                max_wait_ms: 0,
            }),
        }
    }

    /// Send the LaunchSession IPC for the given `id` and `spec`, seed the
    /// transcript, and register the session record.
    ///
    /// This is the shared implementation used by both `launch_session` (which
    /// generates a fresh id) and `restart_session` (which reuses the existing id).
    fn launch_session_with_id(
        &mut self,
        id: String,
        spec: LaunchSpec,
    ) -> Result<String, CoreError> {
        // Forward the host so the agent runs the session on the right machine.
        // Resolve the remote execution target from the host id via the registry
        // (local/unknown hosts → None → the agent runs locally). An explicit
        // `spec.host_target` wins if a caller already set one.
        let host_target = spec.host_target.clone().or_else(|| {
            self.hosts
                .list()
                .iter()
                .find(|h| h.id == spec.host_id && !h.is_local)
                .map(|h| h.target.clone())
        });
        // Resolve the per-host launch config (claude_bin / setup / env) from the
        // registry by host_id and inject it for the agent. Applies to ALL host
        // types (local, WSL, SSH/plink). An explicit value already on the spec
        // wins over the registry lookup. `env` is forwarded as `[k, v]` pairs
        // (the agent parses it back identically).
        //
        // Driver-aware bin resolution (deliverable #1): look up `spec.driver` in
        // `host.agent_commands`; fall back to `claude_bin` for "claude"; else bare
        // driver name. An explicit `spec.claude_bin` still wins (preserves the
        // restart path's persisted-value passthrough).
        let host_cfg = self.hosts.list().iter().find(|h| h.id == spec.host_id);
        let claude_bin = if spec.claude_bin.is_some() {
            spec.claude_bin.clone()
        } else if let Some(host) = host_cfg {
            let resolved = resolve_driver_bin(
                &spec.driver,
                host.claude_bin.as_deref(),
                &host.agent_commands,
            );
            // Only set it when the resolved value differs from the bare driver name
            // (i.e., there IS an override); the agent defaults to the bare name too.
            if resolved != spec.driver {
                Some(resolved)
            } else {
                None
            }
        } else {
            None
        };
        let setup = spec
            .setup
            .clone()
            .or_else(|| host_cfg.and_then(|h| h.setup.clone()));
        let env = if !spec.env.is_empty() {
            spec.env.clone()
        } else {
            host_cfg.map(|h| h.env.clone()).unwrap_or_default()
        };
        // Forward the custom name so the agent persists it. Trim and treat
        // whitespace-only as absent (pass nothing rather than a blank name).
        let custom_name: Option<String> = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let msg = IpcMsg::LaunchSession {
            session_id: Some(id.clone()),
            project: spec.project.clone(),
            cwd: spec.project.clone(),
            host_id: Some(spec.host_id.clone()),
            host_target,
            claude_bin,
            setup,
            env,
            model: spec.model.clone(),
            session_mode: spec.session_mode.clone(),
            first_prompt: spec.first_prompt.clone(),
            continue_: matches!(spec.mode, LaunchMode::Continue).then_some(true),
            resume_session_id: match &spec.mode {
                LaunchMode::Resume(sid) => Some(sid.clone()),
                _ => None,
            },
            name: custom_name.clone(),
            disallowed_tools: spec.disallowed_tools.clone(),
            // X4a (docs/specs/wasl-connector.md §X4a): forward the caller's
            // request verbatim so the agent can inject a Wasl MCP-server
            // entry at spawn time.
            wasl: spec.wasl,
            // Stage 1 codex launch: forward the driver discriminator so the
            // agent spawns the right binary. Defaults to "claude" on older
            // wire messages (handled by `default_driver` in marsad-proto).
            driver: spec.driver.clone(),
        };
        self.send_ipc(&msg)?;

        // Seed the transcript with prior history when resuming (loaded once — old,
        // fully-flushed JSONL, so no race), then the first prompt as a user turn.
        let mut transcript = match &spec.mode {
            LaunchMode::Resume(sid) => self.transcript.session(&spec.project, sid),
            LaunchMode::Continue => self.transcript.most_recent(&spec.project),
            LaunchMode::Fresh => Vec::new(),
        };
        if let Some(fp) = spec.first_prompt.as_deref() {
            let fp = fp.trim();
            if !fp.is_empty() {
                transcript.push(marsad_events::TranscriptEntry::User {
                    text: fp.to_string(),
                    at: Some(now_ms()),
                });
            }
        }

        // Upsert: if the id already exists (restart path), reset the record;
        // if new, push onto the order list first.
        if !self.sessions.contains_key(&id) {
            self.session_order.push(id.clone());
        }
        // Seed permission_mode from the launch session_mode so the Config tab shows
        // the correct mode immediately — before the authoritative system/init event
        // arrives (which will override it when it does). An empty session_mode means
        // "backend default" → leave permission_mode as None until system/init lands.
        let mut initial_state = SessionState::initial();
        if !spec.session_mode.is_empty() {
            initial_state.permission_mode = Some(spec.session_mode.clone());
        }
        let driver = spec.driver.clone();
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: spec.project,
                host_id: spec.host_id,
                state: initial_state,
                changes: Vec::new(),
                transcript,
                seen_msg_ids: std::collections::HashSet::new(),
                name: custom_name,
                shell_running: false,
                driver,
                // A launch (or a resume, which reuses this path) is starting the
                // process now → Resumed. The agent's SessionLiveness will correct
                // this to Paused if the launch never actually comes up.
                live: true,
                conn_health: marsad_proto::ConnHealth::Connected,
                // A MarsadManaged launch (today's path) — unchanged behavior.
                runtime_owner: RuntimeOwner::MarsadManaged,
                wasl_last_seq: 0,
                wasl_seen_seqs: std::collections::HashSet::new(),
                wasl_open_assistant: None,
                open_assistant: None,
                wasl_turn_closed: false,
                wasl_native_gates: HashMap::new(),
            },
        );

        // Auto-create a single-node fast-lane workstream for newly launched sessions
        // that have no existing membership (fresh launches from NewSession; NodeModal
        // calls assign_session separately so those sessions already have a workstream).
        if !self.membership.contains_key(&id) {
            let record = self.sessions.get(&id).unwrap();
            let ws_name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name: ws_name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id.clone());
            let _ = self.store.set_membership(&self.membership);
        }

        Ok(id)
    }

    /// Restart a session with a new model, session_mode, and optionally a new set of
    /// disallowed tools.
    ///
    /// Sends `StopSession` to kill the running claude process, then re-launches
    /// the session with the **same `id`** so the UI row stays in place.  If a
    /// claude session-UUID is known (from a prior `system/init` event), the
    /// relaunch uses `Resume(<uuid>)` so the conversation history is preserved;
    /// otherwise it falls back to `Continue` (most-recent session in that project).
    ///
    /// Returns `UnknownSession` for an unrecognised `id`.
    pub fn restart_session(
        &mut self,
        id: &str,
        model: &str,
        session_mode: String,
        disallowed_tools: Vec<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;

        // Snapshot what we need before mutating state.
        let project = self.sessions[id].project.clone();
        let host_id = self.sessions[id].host_id.clone();
        let claude_uuid = self.sessions[id].state.session_id.clone();
        // Preserve any custom name so a restart doesn't lose it.
        let existing_name = self.sessions[id].name.clone();

        // 1. Stop the running claude process.
        self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        })?;

        // 2. Choose the launch mode: Resume if we have the claude UUID, else Continue.
        let mode = if let Some(uuid) = claude_uuid.filter(|u| !u.is_empty()) {
            LaunchMode::Resume(uuid)
        } else {
            LaunchMode::Continue
        };

        // 3. Relaunch with the SAME marsad session id.
        // Preserve the session's original host so a restart re-launches on the
        // same machine (WSL/SSH), not silently back on local.
        //
        // Load the persisted session to recover its per-session launch config
        // (setup / env / claude_bin). The store is the authoritative source: it
        // was written at launch time with the resolved values, so a restart
        // reproduces EXACTLY what the initial launch used — even if the host
        // registry has since been modified or the host removed.  If the record
        // is absent (e.g. session was never persisted, or was already deleted),
        // fall back to None/empty so launch_session_with_id re-resolves from
        // the registry as before.
        let (persisted_setup, persisted_env, persisted_claude_bin, persisted_driver) = self
            .store
            .restore()
            .ok()
            .and_then(|r| r.sessions.into_iter().find(|s| s.id == id))
            .map(|s| (s.setup, s.env, s.claude_bin, s.driver))
            .unwrap_or((None, Vec::new(), None, "claude".to_string()));

        let spec = LaunchSpec {
            project,
            host_id,
            model: model.to_string(),
            session_mode,
            first_prompt: None,
            mode,
            host_target: None,
            claude_bin: persisted_claude_bin,
            setup: persisted_setup,
            env: persisted_env,
            name: existing_name,
            disallowed_tools,
            // A restart never re-attaches a Wasl MCP-server entry this cycle
            // (mirrors the agent's own resume path, `marsad-agent/src/ipc.rs`
            // — out of X4's scope; start a fresh `wasl:true` launch instead).
            wasl: false,
            // Preserve the original driver so a restart uses the same agent
            // binary. Defaults to "claude" for pre-Stage-1 persisted records.
            driver: persisted_driver,
        };
        self.launch_session_with_id(id.to_string(), spec)?;

        // 4. Return a session view so the UI can update immediately.
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)])
    }

    /// Persist a named config template (model + mode).
    ///
    /// Overwrites any existing template with the same name.
    pub fn save_template(
        &mut self,
        name: &str,
        model: &str,
        mode: &str,
    ) -> Result<(), CoreError> {
        let tmpl = marsad_store::PersistedTemplate {
            name: name.to_string(),
            model: model.to_string(),
            mode: mode.to_string(),
        };
        self.store
            .upsert_template(&tmpl)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    /// Return all persisted config templates.
    pub fn list_templates(&self) -> Result<Vec<marsad_store::PersistedTemplate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore templates: {e:?}")))?;
        Ok(restored.templates)
    }

    /// Resume a stopped session: tell the agent to relaunch/re-attach it, and
    /// flip the UI badge OFF Stale immediately.
    ///
    /// Previously this only fired the IPC and returned `()`, so the dashboard
    /// never changed — the user saw "resume did nothing, still stopped" (the same
    /// bug `stop_session` had). The agent does relaunch claude, but a resumed
    /// session has no first prompt, so claude stays idle and emits no
    /// `system/init` — meaning NO inbound stream event would ever move the phase
    /// off Stale. We therefore set `Starting` optimistically and return the
    /// VmUpdates the bridge emits; typing a prompt then drives it to Active.
    pub fn resume_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::ResumeSession {
            session_id: id.to_string(),
            first_prompt: None,
        })?;
        // require_session above guarantees it exists.
        {
            let rec = self.sessions.get_mut(id).unwrap();
            rec.state.phase = Phase::Starting;
            // Optimistically Resumed: we are (re)starting the process now. The
            // agent's SessionLiveness corrects this to Paused if it never comes
            // up. (For an already-live session the agent no-ops the relaunch —
            // see handle_resume_session — and live stays true anyway.)
            rec.live = true;
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Resuming moves the session from Paused → live on the board.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn attach_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits today's
        // AttachSession; WaslManaged emits AttachWaslSession{after_seq}.
        self.attach_session_events(id)
    }

    /// Stop a session: kill its running claude process (the agent drops the
    /// pipe/container) and reflect it in the UI by marking the row not-live.
    /// The session stays in the list and is resumable via `restart_session`.
    ///
    /// Previously this only fired the IPC and returned `()`, so the dashboard
    /// never changed — the user saw "stop did nothing". Now it returns the
    /// VmUpdates the bridge emits so the badge flips to Stale immediately.
    pub fn stop_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits StopSession;
        // WaslManaged emits StopWaslSession.
        self.stop_session_routed(id)?;
        // Mark the session not-live (Paused). require_session guarantees it
        // exists. `live=false` is the authoritative lifecycle flip; the phase is
        // also set Stale for legacy consumers, but the board/affordance now key
        // off `live`.
        {
            let rec = self.sessions.get_mut(id).unwrap();
            rec.state.phase = Phase::Stale;
            rec.live = false;
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Stopping moves the session into the board's Paused group.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Remove a session entirely (the user's explicit declutter action). Stops
    /// the process best-effort, drops the session from the in-memory list AND the
    /// persisted store (so it does NOT reappear on relaunch), and tells the UI to
    /// delete the row. Unlike `stop_session` (which keeps it resumable), this is
    /// permanent — the conversation is still re-creatable as a fresh resume if the
    /// claude JSONL survives.
    pub fn remove_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        // Best-effort stop so we don't orphan a running process. The session is
        // going away regardless, so a transport error here is non-fatal.
        let _ = self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        });
        // Delete from the store FIRST so an error leaves in-memory state intact.
        self.store
            .delete_session(id)
            .map_err(|e| CoreError::Agent(format!("delete session: {e:?}")))?;
        // X6: also drop any Wasl binding record so a removed WaslManaged session
        // leaves no stale reconnect binding behind. Best-effort (its absence for a
        // MarsadManaged session is a no-op).
        let _ = self.store.delete_wasl_binding(id);
        self.sessions.remove(id);
        self.session_order.retain(|s| s != id);

        // Clean up membership and cascade-delete the parent workstream when this
        // node was its LAST member, so the board never keeps a ghost empty
        // workstream requiring a second delete (#5). Capture the parent BEFORE
        // dropping membership. This subsumes the old `ws-<id>` special case: the
        // auto single-node workstream always has exactly one member (this session)
        // so removing it empties the parent — but it ALSO handles a user-authored
        // workstream whose only node is this one (its id is NOT `ws-<id>`), which
        // the old auto-id-only cleanup left behind as an empty shell.
        let parent_ws = self.membership.remove(id);
        let mut removed_ws = false;
        if let Some(ws) = parent_ws {
            let still_has_members = self.membership.values().any(|w| *w == ws);
            if !still_has_members {
                let _ = self.store.delete_workstream(&ws);
                self.workstreams.retain(|w| w.id != ws);
                // Drop any connections scoped to the now-deleted workstream.
                let ws_conns: Vec<String> = self
                    .connections
                    .iter()
                    .filter(|c| c.ws == ws)
                    .map(|c| c.id.clone())
                    .collect();
                for cid in ws_conns {
                    let _ = self.store.delete_connection(&cid);
                }
                self.connections.retain(|c| c.ws != ws);
                self.ws_leads.remove(&ws);
                removed_ws = true;
            }
        }
        let _ = self.store.set_membership(&self.membership);

        // Cascade-delete every connection that touches the removed node (inbound
        // OR outbound). Best-effort store deletes (mirror delete_workstream).
        let dangling: Vec<String> = self
            .connections
            .iter()
            .filter(|c| c.from == id || c.to == id)
            .map(|c| c.id.clone())
            .collect();
        for cid in dangling {
            let _ = self.store.delete_connection(&cid);
        }
        self.connections.retain(|c| c.from != id && c.to != id);

        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        let mut updates = vec![
            VmUpdate::SessionRemoved { id: id.to_string() },
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ];
        // When we cascade-deleted the parent workstream, the client must be told
        // so the empty workstream vanishes in the SAME op (no lingering shell).
        // Callers that also emit a Workstreams update (delete_workstream) tolerate
        // the duplicate — the receiver replaces its slice wholesale.
        if removed_ws {
            updates.push(self.workstreams_update());
        }
        Ok(updates)
    }

    /// Inject a shell command and its output into the session transcript.
    ///
    /// Called by the bridge after it runs `!<command>` on the session's host
    /// (local or remote). Core is IO-free: the bridge does the actual execution
    /// and passes the command string + captured output here. Two transcript
    /// entries are appended:
    ///   1. `User { text: "! <command>" }` — the issued command (matches how
    ///      Claude Code shows shell commands: `!cmd` prefix).
    ///   2. `Tool { name: "shell", input: <output> }` — the captured output,
    ///      rendered in the Activity tab as a ⚙ tool row with monospaced output.
    ///
    /// Returns `UnknownSession` for an unrecognised `id`.
    pub fn inject_shell_output(
        &mut self,
        id: &str,
        command: &str,
        output: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        let at = Some(now_ms());
        record
            .transcript
            .push(marsad_events::TranscriptEntry::User {
                text: format!("! {}", command.trim()),
                at,
            });
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Tool {
                name: "shell".to_string(),
                input: output.to_string(),
                at,
            });
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Step 1 of an ASYNC `!` shell command: append the `! <command>` line to the
    /// transcript immediately (so it shows the instant the user hits enter) and
    /// mark a shell command as running, so the UI can show a working indicator
    /// while the command executes off-thread. Paired with
    /// [`inject_shell_result`], which appends the output when the command
    /// finishes. Returns a Session update to render right away.
    pub fn inject_shell_command(
        &mut self,
        id: &str,
        command: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        record
            .transcript
            .push(marsad_events::TranscriptEntry::User {
                text: format!("! {}", command.trim()),
                at: Some(now_ms()),
            });
        record.shell_running = true;
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Step 2 of an ASYNC `!` shell command: append the captured output as a
    /// shell tool row and clear the running flag. The `! <command>` line was
    /// already appended by [`inject_shell_command`], so only the output is added.
    pub fn inject_shell_result(
        &mut self,
        id: &str,
        output: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        record.shell_running = false;
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Tool {
                name: "shell".to_string(),
                input: output.to_string(),
                at: Some(now_ms()),
            });
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    pub fn send_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Record the user's prompt immediately — claude does not echo it on the
        // stream, so this is the only place it can enter the transcript. It surfaces
        // on the next agent event (claude's reply) which re-emits the SessionView.
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            // Safe: require_session above guarantees the entry exists.
            let rec = self.sessions.get_mut(id).unwrap();
            rec.transcript
                .push(marsad_events::TranscriptEntry::User {
                    text: trimmed.to_string(),
                    at: Some(now_ms()),
                });
            // Optimistically enter Working the instant the user sends — don't wait
            // for claude's first token. Clears any prior end reason / stale
            // interrupt so a fresh turn starts clean. The stream re-drives phase
            // authoritatively from here (assistant → Working, result → Done).
            rec.state.phase = Phase::Working;
            rec.state.end_reason = None;
            rec.state.interrupt_requested = false;
        }
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits SendPrompt;
        // WaslManaged emits SendWaslPrompt (fresh per-send UUID idempotency key).
        self.send_session_prompt(id, text)
    }

    /// Change a session config option (opencode: model / mode) on the running
    /// agent. Fire-and-forget: the agent applies it via ACP and the reader
    /// surfaces the agent-confirmed new state — we do NOT optimistically mutate
    /// local state (truthful; the agent could reject or normalize the value).
    pub fn set_session_config_option(
        &mut self,
        id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::SetSessionConfigOption {
            session_id: id.to_string(),
            config_id: config_id.to_string(),
            value: value.to_string(),
        })
    }

    pub fn interrupt(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Record that WE initiated this interrupt so the resulting error turn is
        // classified as a calm user Cancel (Done, "Cancelled by the user") rather
        // than a failure. require_session guarantees the entry exists.
        self.sessions.get_mut(id).unwrap().state.interrupt_requested = true;
        // Emit a dedicated `Interrupt` message so the agent routes it to
        // handle_interrupt — which writes claude's `interrupt` control_request to
        // stop the CURRENT TURN (claude then ends the turn with
        // error_during_execution, so the UI leaves "Working").
        //
        // Previously this sent a StopSession message with an extra `interrupt`
        // flag, but the agent dispatches purely on `type`: it hit
        // handle_stop_session, which KILLED the durable session AND never stopped
        // the turn (so the badge stayed "Working").
        //
        // X6 (§6.1): route by RuntimeOwner — WaslManaged emits InterruptWaslSession.
        self.interrupt_session_routed(id)
    }

    /// Answer an `AskUserQuestion` gate by sending the user's chosen answers to
    /// the agent, which forwards them to claude as an `updatedInput` control_response.
    ///
    /// `id` is the marsad session id; `request_id` is the control-request id captured
    /// in the `Gate::Question`; `answers` maps question text to the selected label.
    pub fn answer_question(
        &mut self,
        id: &str,
        request_id: &str,
        answers: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        self.send_ipc(&marsad_proto::IpcMsg::AnswerQuestion {
            session_id: id.to_string(),
            request_id: request_id.to_string(),
            answers,
        })?;
        // Clear the answered question gate immediately (same rationale as
        // answer_gate) so its card disappears on answer.
        if let Some(record) = self.sessions.get_mut(id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Refresh the board too so the answered question's needs-you card clears.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn answer_gate(&mut self, decision: GateDecision) -> Result<Vec<VmUpdate>, CoreError> {
        let session_id = marsad_gates::route(&decision).to_string();
        self.require_session(&session_id)?;

        // X6 (§6.1/§8): a WaslManaged session's gate is a native gate — route it
        // to the dedicated native-gate answer path (which relays
        // AnswerWaslNativeGate, not AnswerGate). Identify the gate by the
        // session's current pending Permission's request_id (== the gate_id).
        if matches!(
            self.runtime_owner(&session_id),
            RuntimeOwner::WaslManaged { .. }
        ) {
            let gate_id = self
                .sessions
                .get(&session_id)
                .and_then(|r| match &r.state.gate {
                    Some(marsad_gates::Gate::Permission { request_id, .. }) => {
                        Some(request_id.clone())
                    }
                    _ => None,
                });
            if let Some(gate_id) = gate_id {
                return self.answer_wasl_native_gate(&session_id, &gate_id, decision.decision);
            }
            // No pending native gate to answer → silent no-op (mirrors the
            // benign "nothing to answer" disposition).
            return Ok(vec![]);
        }

        // Idempotency: already recorded → silent no-op (no duplicate IPC).
        if self.ledger.seen(&decision.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        // unwrap() is infallible in this code path.
        self.ledger.record(&decision).unwrap();

        let ctrl = marsad_gates::to_control_response(&decision);
        self.send_ipc(&IpcMsg::AnswerGate {
            session_id: session_id.clone(),
            request_id: ctrl.request_id,
            allow: ctrl.allow,
            deny_reason: ctrl.deny_reason,
            // For an accepted scope-elicitation, forward the chosen content
            // (e.g. { "grant": "session" }); None for everything else.
            elicitation_content: decision.elicitation_content.clone(),
            // For an accepted "always allow …" suggestion, forward the chosen
            // rule verbatim → claude's updatedPermissions; None otherwise.
            permission_updates: decision.permission_updates.clone(),
        })?;

        // Optimistically clear the pending gate NOW so the card disappears the
        // instant the human answers — don't wait for a later stream message
        // (a bare permission approval may not emit one that clears it, leaving
        // the answered gate stuck on screen). The claude stream stays the source
        // of truth: its next messages re-drive phase/gate as usual.
        if let Some(record) = self.sessions.get_mut(&session_id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Refresh the board too: clearing the gate removes the attention card's
        // needs-you headline, so the dashboard card must update immediately.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    /// X3 (docs/specs/wasl-connector.md §5.2): relay ONE explicit human
    /// decision on a pending Wasl gate. NO session lookup (Wasl gates are
    /// host-scoped, not session-scoped — deliberately not `require_session`).
    pub fn answer_wasl_gate(
        &mut self,
        host_id: &str,
        gate_id: &str,
        decision: marsad_gates::Decision,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let view = self
            .wasl_gates
            .get(&(host_id.to_string(), gate_id.to_string()))
            .ok_or_else(|| CoreError::UnknownWaslGate(format!("{host_id}/{gate_id}")))?;
        let gate = marsad_gates::Gate::Handoff {
            gate_id: view.gate_id.clone(),
            kind: view.kind.clone(),
            from: view.from.clone(),
            summary: view.summary.clone(),
            preview: view.preview.clone(),
        };
        let scope = format!("wasl:{host_id}");
        let gd = match &decision {
            marsad_gates::Decision::Approve => marsad_gates::approve(&scope, &gate),
            marsad_gates::Decision::Reject { reason } => {
                marsad_gates::reject(&scope, &gate, reason)
            }
        };

        // Idempotency: identical to answer_gate — seen -> silent no-op, no
        // duplicate IPC. `decision_id` is content-stable over (scope,
        // gate_id), so approve-then-deny of the same gate is ALSO deduped.
        if self.ledger.seen(&gd.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is
        // false. unwrap() is infallible in this code path.
        self.ledger.record(&gd).unwrap();

        let (outcome, reason) = match gd.decision {
            marsad_gates::Decision::Approve => ("approved".to_string(), None),
            marsad_gates::Decision::Reject { reason } => ("denied".to_string(), Some(reason)),
        };
        // Node-authoritative: the pending list changes only on WaslGateDecided,
        // so the success value is always an empty update list; `.map` (rather
        // than `send_ipc(...)?; Ok(vec![])`) keeps that a single expression.
        self.send_ipc(&IpcMsg::AnswerWaslGate {
            host_id: host_id.to_string(),
            gate_id: gate_id.to_string(),
            outcome,
            reason,
        })
        .map(|()| vec![])
    }

    /// X6 (Wasl Backend, §8): answer a NATIVE gate on a WaslManaged session.
    /// SESSION-scoped (not host-scoped like the X1 operator gate): the gate is a
    /// `Gate::Permission` in the session's own gate slot, exactly as an opencode
    /// ACP permission is. Records the decision in the EXISTING `DecisionLedger`
    /// (idempotent — a second answer, even a different verdict, is a silent
    /// no-op) and emits ONE `AnswerWaslNativeGate`. Answering a gate that was
    /// never requested is an error (the session-scoped analogue of
    /// `UnknownWaslGate`), not a silent success.
    ///
    /// Does NOT touch the host-scoped `wasl_gates` / `VmUpdate::WaslGates` list —
    /// that stays exclusively for X1 operator handoff gates (regression pin).
    pub fn answer_wasl_native_gate(
        &mut self,
        session_id: &str,
        gate_id: &str,
        decision: marsad_gates::Decision,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(session_id)?;

        // Reconstruct the exact Gate::Permission this native gate maps to, so the
        // derived DecisionId is the SAME as the one the request path would use
        // (the command encodes gate_id via native_gate_command → stable+unique).
        // A gate that was never requested (no stored detail) is an error.
        let detail = self
            .sessions
            .get(session_id)
            .and_then(|r| r.wasl_native_gates.get(gate_id).cloned())
            .ok_or_else(|| CoreError::UnknownWaslGate(format!("{session_id}/{gate_id}")))?;
        let gate = native_gate_permission(gate_id, &detail);

        let gd = match &decision {
            marsad_gates::Decision::Approve => marsad_gates::approve(session_id, &gate),
            marsad_gates::Decision::Reject { reason } => {
                marsad_gates::reject(session_id, &gate, reason)
            }
        };

        // Idempotency: identical to answer_gate — seen → silent no-op, no
        // duplicate IPC. `decision_id` is content-stable over (session_id,
        // command+cwd), so approve-then-deny of the same gate is ALSO deduped.
        if self.ledger.seen(&gd.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        self.ledger.record(&gd).unwrap();

        let (outcome, reason) = match gd.decision {
            marsad_gates::Decision::Approve => ("approved".to_string(), None),
            marsad_gates::Decision::Reject { reason } => ("denied".to_string(), Some(reason)),
        };
        self.send_ipc(&IpcMsg::AnswerWaslNativeGate {
            session_id: session_id.to_string(),
            gate_id: gate_id.to_string(),
            outcome,
            reason,
        })?;

        // Optimistically clear the pending gate now so the card disappears the
        // instant the human answers (mirrors answer_gate). The Wasl observe
        // stream stays the source of truth (a follow-on status/event re-drives
        // phase/gate).
        // NOTE: the gate DETAIL stays in `wasl_native_gates` on purpose — a
        // second answer for the same gate must reconstruct the same DecisionId so
        // the ledger dedup makes it a silent no-op (rather than erroring as an
        // unknown gate). Only the UI-facing gate slot is cleared.
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(session_id, self.sessions.get(session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn run_slash(
        &mut self,
        id: &str,
        cmd: marsad_viewmodel::SlashCmd,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::SlashCmd {
            session_id: id.to_string(),
            cmd: slash_cmd_name(&cmd).to_string(),
        })
    }

    pub fn set_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.active_ws = Some(id.to_string());
        // Persist the active workspace if it exists in our list; best-effort.
        if self.workspaces.iter().any(|w| w.id == id) {
            let _ = self.store.set_active_workspace(id);
        }
        // Best-effort advisory send; ignore errors.
        let line = encode_ipc(&IpcMsg::SetWorkspace {
            workspace_id: id.to_string(),
        });
        let _ = self.transport.send_line(&line);
        Ok(vec![self.workspaces_update()])
    }

    /// Load persisted sessions from the store into the in-memory list (call once
    /// at startup, AFTER `load_hosts` so host labels resolve). Without this the
    /// dashboard comes up empty after a full app exit even though the sessions
    /// were persisted — the user perceives them as "lost". Call order matters:
    /// already-tracked ids (a live session this Core launched) are left untouched.
    ///
    /// Each restored session is marked `Stale` (not live): its claude process
    /// may or may not have survived the exit, so we present it as resumable
    /// (via `restart_session`) rather than claim a liveness we cannot verify. If
    /// the agent re-attaches a still-live local session, its next stream event
    /// flips the phase. Transcript history is loaded from the on-disk JSONL for
    /// local sessions; remote transcripts live on the remote host and are not
    /// fetched here, so a restored remote session shows an empty transcript until
    /// it streams again.
    pub fn load_sessions(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore sessions: {e:?}")))?;
        // X6 (Wasl Backend, §6.1): restore the per-session Wasl runtime bindings
        // so a WaslManaged session reloads as `WaslManaged` (routing the `*Wasl*`
        // family) rather than degrading to `MarsadManaged`. Stored as their own
        // records so this is fully additive to the existing session schema.
        let wasl_bindings = self.store.wasl_bindings().unwrap_or_default();
        let mut updates = Vec::new();
        for s in restored.sessions {
            if self.sessions.contains_key(&s.id) {
                continue;
            }
            // Recover the runtime owner (and the last observed seq) from the
            // persisted binding, if any. Absent binding → MarsadManaged (default).
            let (runtime_owner, wasl_last_seq) = match wasl_bindings.get(&s.id) {
                Some(b) => (
                    RuntimeOwner::WaslManaged {
                        binding: WaslSessionBinding {
                            target_id: b.target_id.clone(),
                            host_id: b.host_id.clone(),
                            provider_key: b.provider_key.clone(),
                        },
                    },
                    b.last_seq,
                ),
                None => (RuntimeOwner::MarsadManaged, 0),
            };
            // opencode owns no claude `~/.claude` transcript — its history comes
            // from the ACP stream. Reading the project's claude transcript here
            // would splice a CO-LOCATED claude session's history under an opencode
            // session (opencode header + claude transcript). Matches the guard in
            // the agent reader's remote backfill.
            let transcript = if s.driver == "opencode" {
                Vec::new()
            } else {
                match &s.session_id {
                    Some(sid) if !sid.is_empty() => self.transcript.session(&s.project, sid),
                    _ => self.transcript.most_recent(&s.project),
                }
            };
            let mut state = SessionState::initial();
            // Two-axis model: the ACTIVITY of a restored session is idle → Done
            // (it isn't mid-turn from our POV). "Not running" is carried by the
            // separate liveness axis (`live=false` below → Paused) until the agent
            // reconnects and proves it live. (Previously this was `Stale`, which
            // conflated activity with liveness.)
            state.phase = Phase::Done;
            state.model = Some(s.model.clone());
            state.session_id = s.session_id.clone();
            // Restore the persisted mode so the Config tab shows the correct
            // mode even when the live claude process does NOT re-emit system/init
            // (reconnect after restart). Normalize legacy Posture enum-name values
            // (e.g. "AutoApproveSafe" → "acceptEdits") from old records.
            state.permission_mode = marsad_store::normalize_session_mode(s.mode.clone());
            // Restore slash_commands, tools, and mcp_servers from the last
            // system/init so Config/Composer are pre-populated on attach/reconnect
            // without waiting for a fresh system/init.
            state.slash_commands = s.slash_commands.clone();
            state.tools = s.tools.clone();
            state.mcp_servers = s.mcp_servers.clone();
            // Restore the custom name: PersistedSession.name is the marsad id
            // when no custom name was set (agent default). Treat that as None so
            // the view falls back to the project basename.
            let restored_name = if s.name != s.id {
                Some(s.name.clone())
            } else {
                None
            };
            // Probe git changes immediately for LOCAL sessions so the Changes tab
            // is populated without waiting for the first tool_use/result event.
            // Remote sessions get their changes from the agent's ChangesUpdate IPC.
            let is_local = self.host_is_local(&s.host_id);
            let initial_changes = if is_local {
                self.git.changes(&s.project)
            } else {
                Vec::new()
            };
            // Seed token/cost totals for LOCAL sessions from the session JSONL so
            // the Usage tab shows the whole-conversation total immediately on attach
            // rather than zero. Remote sessions get their usage from the agent's
            // UsageUpdate IPC (remote_usage_script). The first live `result` event
            // will authoritatively replace these estimates via reduce_result.
            if is_local && s.driver != "opencode" {
                if let Some(sid) = s.session_id.as_deref().filter(|s| !s.is_empty()) {
                    let usage_agg = self.transcript.session_usage(&s.project, sid);
                    if !usage_agg.is_empty() {
                        let total_tokens: u64 = usage_agg.values().map(|u| {
                            u.input_tokens
                                .saturating_add(u.output_tokens)
                                .saturating_add(u.cache_read_input_tokens)
                                .saturating_add(u.cache_creation_input_tokens)
                        }).sum();
                        if total_tokens > 0 {
                            state.tokens = total_tokens;
                            let window = state.model.as_deref()
                                .map(marsad_events::SessionState::context_window_for)
                                .unwrap_or(200_000);
                            let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                            state.context_pct = pct.min(100.0);
                        }
                        for (model, summary) in usage_agg {
                            state.model_usage.insert(
                                model,
                                marsad_proto::ModelUsage {
                                    input_tokens: summary.input_tokens,
                                    output_tokens: summary.output_tokens,
                                    cost_usd: summary.cost_usd,
                                    estimated: true,
                                },
                            );
                        }
                    }
                }
            }
            self.session_order.push(s.id.clone());
            self.sessions.insert(
                s.id.clone(),
                SessionRecord {
                    project: s.project.clone(),
                    host_id: s.host_id.clone(),
                    state,
                    changes: initial_changes,
                    transcript,
                    seen_msg_ids: std::collections::HashSet::new(),
                    name: restored_name,
                    shell_running: false,
                    driver: s.driver.clone(),
                    // Restored from the store: we do NOT yet know if the process
                    // survived on its host. Start Paused; the agent flips it to
                    // Resumed via a stream event / backfill / SessionLiveness once
                    // it actually reconnects. No optimism — truth on confirmation.
                    live: false,
                    conn_health: marsad_proto::ConnHealth::Connected,
                    runtime_owner,
                    wasl_last_seq,
                    wasl_seen_seqs: std::collections::HashSet::new(),
                    wasl_open_assistant: None,
                    open_assistant: None,
                    wasl_turn_closed: false,
                    wasl_native_gates: HashMap::new(),
                },
            );
            let sv = self.make_session_view(&s.id, self.sessions.get(&s.id).unwrap());
            updates.push(VmUpdate::Session(sv));
        }
        let snap = self.snapshot();
        updates.push(VmUpdate::Overview(marsad_viewmodel::group_overview(&snap)));
        Ok(updates)
    }
}
