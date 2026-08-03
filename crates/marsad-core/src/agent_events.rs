//! Inbound agent-line handling — the central testable seam. `on_agent_line`
//! parses one IPC message and dispatches to the per-kind `handle_*` methods,
//! including the WASL gate/observation handlers. Extracted from `lib.rs`
//! verbatim; `impl Core` is reopened here and `use super::*` brings the crate's
//! types and the root module's private plumbing (`send_ipc`, `make_session_view`,
//! `board`, …) into scope (child modules can call crate-root-private items).

use super::*;

impl Core {
    // -----------------------------------------------------------------------
    // Inbound IPC parsing — central testable seam
    // -----------------------------------------------------------------------

    pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let msg = match marsad_proto::parse_ipc(line) {
            Ok(m) => m,
            // Tolerant skip: a FUTURE inbound type (newer agent) must not error
            // the reader loop. The raw line is preserved via stderr for
            // observability.
            //
            // SPEC CHANGE (docs/specs/typed-ipc.md §7, "Tests that MUST
            // change"): crates/marsad-core/tests/errors.rs:359-369
            // (`on_agent_line_unknown_ipc_type_returns_protocol_error`) was
            // rewritten to `on_agent_line_unknown_ipc_type_is_tolerant_noop`,
            // asserting `Ok(vec![])` here, per §4's unknown-type disposition
            // table.
            Err(marsad_proto::ProtoError::UnknownDiscriminant(t)) => {
                eprintln!("marsad-core: on_agent_line: skipping unknown IPC type '{t}': {line}");
                return Ok(vec![]);
            }
            // Malformed JSON / missing type / wrong-typed known fields stay
            // HARD errors.
            Err(e) => return Err(CoreError::Protocol(e.to_string())),
        };
        match msg {
            IpcMsg::AgentEvent { session_id, event } => self.handle_agent_event(session_id, event),
            IpcMsg::TranscriptBackfill { session_id, jsonl } => {
                self.handle_transcript_backfill(session_id, jsonl)
            }
            IpcMsg::ChangesUpdate {
                session_id,
                changes,
            } => self.handle_changes_update(session_id, changes),
            IpcMsg::UsageUpdate { session_id, usage } => {
                self.handle_usage_update(session_id, usage)
            }
            // Two-axis lifecycle: authoritative Resumed/Paused from the agent.
            IpcMsg::SessionLiveness { session_id, live } => {
                self.handle_session_liveness(&session_id, live)
            }
            // Per-session connection health (Connected/Reconnecting/Unstable) from
            // the agent's reader — drives the disconnected UI + composer hard-block.
            IpcMsg::SessionConn { session_id, health } => {
                self.handle_session_conn(&session_id, health)
            }
            // U1: one (driver, host) snapshot — always a COMPLETE replace, never
            // a merge (dynamic-capabilities.md §2/§3.5).
            IpcMsg::CapabilitiesUpdate {
                host_id,
                driver,
                mut capabilities,
            } => {
                // Merge sessions by project so a path-scoped probe and the
                // connect-time capability probe (which runs concurrently and
                // carries an EMPTY session list) don't clobber each other. The
                // incoming `session_scope` says what this update authoritatively
                // covers:
                //   * AllKnown  → a capability probe: keep the sessions we already
                //     had (its own list is empty; overwriting would flip the picker
                //     to "none discovered" right after a real result landed).
                //   * Projects(p) → replace exactly project p's rows with this
                //     update's rows (an empty result correctly clears p), leaving
                //     every other project's rows intact.
                let key = (driver.clone(), host_id.clone());
                if let Some(existing) = self.capabilities.get(&key) {
                    match &capabilities.session_scope {
                        Some(marsad_proto::SessionScope::Projects(ps)) => {
                            let replacing: std::collections::HashSet<&str> =
                                ps.iter().map(|p| p.as_str()).collect();
                            let mut merged: Vec<marsad_proto::DiscoveredSession> = existing
                                .sessions
                                .iter()
                                .filter(|s| {
                                    s.project
                                        .as_deref()
                                        .map(|p| !replacing.contains(p))
                                        .unwrap_or(false)
                                })
                                .cloned()
                                .collect();
                            merged.extend(capabilities.sessions.iter().cloned());
                            capabilities.sessions = merged;
                        }
                        _ => {
                            capabilities.sessions = existing.sessions.clone();
                        }
                    }
                }
                self.capabilities.insert(key, capabilities.clone());
                let mut updates = vec![VmUpdate::Capabilities(capabilities.into())];
                // Record this driver as discovered for the host. Once every probed
                // driver has landed, a host that was Connecting (reachable, discovery
                // in flight) is promoted to Connected — agents + versioned models are
                // now loaded and the launch modals will show the real truth.
                self.caps_loaded
                    .entry(host_id.clone())
                    .or_default()
                    .insert(driver);
                if self.host_caps_fully_loaded(&host_id) {
                    let is_connecting = self
                        .hosts
                        .list()
                        .iter()
                        .find(|h| h.id == host_id)
                        .map(|h| h.status == HostStatus::Connecting)
                        .unwrap_or(false);
                    if is_connecting {
                        self.hosts.set_status(&host_id, HostStatus::Connected);
                        updates.push(VmUpdate::Hosts(self.hosts.list().to_vec()));
                    }
                }
                Ok(updates)
            }
            // X3 (docs/specs/wasl-connector.md §5.2): a Wasl gate surfaced (or
            // re-surfaced via resnapshot reconcile).
            IpcMsg::WaslGateRequested {
                host_id,
                gate_id,
                decidable,
                request,
                requested_at,
                deadline,
                ..
            } => self.handle_wasl_gate_requested(
                host_id,
                gate_id,
                decidable,
                request,
                requested_at,
                deadline,
            ),
            // X3: a Wasl gate resolved.
            IpcMsg::WaslGateDecided {
                host_id,
                gate_id,
                outcome,
                decided_by,
                reason,
                ..
            } => self.handle_wasl_gate_decided(host_id, gate_id, outcome, decided_by, reason),
            // X3: one audit-feed entry (the observation feed).
            IpcMsg::WaslObservation { host_id, entry } => {
                self.handle_wasl_observation(host_id, entry)
            }
            // X3: serve-loop ack for `AnswerWaslGate` — joins the existing
            // ack no-op list.
            IpcMsg::WaslGateAnswered { .. } => Ok(vec![]),
            // X6 (Wasl Backend, §6.3/§7): a WaslManaged session's advisory status
            // changed — project it onto the existing Phase (NO new variant).
            IpcMsg::WaslSessionStatus { session_id, status } => {
                self.handle_wasl_session_status(session_id, status)
            }
            // X6 (§7/§8/§10.4): one normalized observe event — fold it into the
            // transcript + native-gate projection.
            IpcMsg::WaslSessionEvent {
                session_id,
                seq,
                event,
            } => self.handle_wasl_session_event(session_id, seq, event),
            // X6 (§10.4): a prompt receipt — surface queued/rejected outcomes
            // visibly (not a silent drop).
            IpcMsg::WaslPromptReceipt {
                session_id,
                receipt,
            } => self.handle_wasl_prompt_receipt(session_id, receipt),
            // X6: serve-loop ack for `AnswerWaslNativeGate` — no-op (the follow-on
            // status/event is authoritative), joining the ack no-op list.
            IpcMsg::WaslNativeGateAnswered { .. } => Ok(vec![]),
            // Command acks legitimately arrive on the subscriber connection.
            IpcMsg::SessionLaunched { .. }
            | IpcMsg::SessionResumed { .. }
            | IpcMsg::SessionAttached { .. }
            | IpcMsg::SessionStopped { .. }
            | IpcMsg::PromptSent { .. }
            | IpcMsg::Interrupted { .. }
            | IpcMsg::GateAnswered { .. }
            | IpcMsg::QuestionAnswered { .. }
            | IpcMsg::Subscribed { .. }
            | IpcMsg::CapabilitiesRefreshQueued { .. }
            | IpcMsg::Error { .. } => Ok(vec![]),
            // A GUI→agent message arriving inbound is a routing bug: hard error
            // (same observable class as today's "unknown IPC type" Protocol error).
            other => Err(CoreError::Protocol(format!(
                "misdirected IPC message on agent channel: {other:?}"
            ))),
        }
    }

    /// X6 (§6.3/§7): project a WaslManaged session's advisory status onto the
    /// existing `Phase` (NO new variant) and refresh liveness. Unknown session /
    /// malformed status → tolerant no-op.
    fn handle_wasl_session_status(
        &mut self,
        session_id: String,
        status: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            return Ok(vec![]);
        }
        let st: marsad_wasl_session::WaslTargetStatus = match serde_json::from_value(status) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("marsad-core: WaslSessionStatus for {session_id}: bad status: {e}");
                return Ok(vec![]);
            }
        };
        let phase = marsad_wasl_session::project_status_to_phase(&st);
        let live = marsad_wasl_session::status_is_live(&st);
        let record = self.sessions.get_mut(&session_id).unwrap();
        record.state.phase = phase;
        record.live = live;
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Fold ONE normalized Wasl observe event into the session's transcript +
    /// native-gate projection (§7/§8/§10.4). Handles every §10.4 edge case:
    /// dedup by `seq`, delta-append to one entry, single terminal close,
    /// post-terminal delta ignore, and native-gate → `Gate::Permission`. Unknown
    /// session / malformed event / duplicate seq → tolerant no-op.
    fn handle_wasl_session_event(
        &mut self,
        session_id: String,
        seq: u64,
        event: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            // An event for a session core never launched: tolerant no-op (no
            // panic, no cross-session corruption).
            return Ok(vec![]);
        }
        let ev: marsad_wasl_session::WaslSessionEvent = match serde_json::from_value(event) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("marsad-core: WaslSessionEvent for {session_id}: bad event: {e}");
                return Ok(vec![]);
            }
        };

        // Dedup by seq: a replay overlap after an ObserveLost/reattach must fold
        // exactly once. A duplicate seq is a silent no-op (no updates).
        {
            let record = self.sessions.get_mut(&session_id).unwrap();
            if !record.wasl_seen_seqs.insert(seq) {
                return Ok(vec![]);
            }
            // Advance the "last observed seq" (§6.1 step 2) and persist it so a
            // reconnect re-attaches from here. Best-effort persist.
            if seq > record.wasl_last_seq {
                record.wasl_last_seq = seq;
            }
        }
        self.persist_wasl_last_seq(&session_id);

        use marsad_wasl_session::WaslSessionEventKind as K;
        let record = self.sessions.get_mut(&session_id).unwrap();
        match ev.kind {
            K::TurnAccepted { .. } => {
                // A new turn begins: it is now in-flight (Working), a fresh
                // assistant entry will open on the first delta, and the
                // post-terminal guard is cleared.
                record.state.phase = Phase::Working;
                record.wasl_turn_closed = false;
                record.wasl_open_assistant = None;
            }
            K::AssistantDelta { text, .. } => {
                // Post-terminal stray (a delta after this turn's TurnCompleted /
                // TurnFailed with no newer TurnAccepted) → ignored, never folded.
                if !record.wasl_turn_closed {
                    match record.wasl_open_assistant {
                        Some(idx) => {
                            if let Some(marsad_events::TranscriptEntry::Assistant {
                                text: existing,
                                ..
                            }) = record.transcript.get_mut(idx)
                            {
                                existing.push_str(&text);
                            }
                        }
                        None => {
                            record
                                .transcript
                                .push(marsad_events::TranscriptEntry::Assistant {
                                    text,
                                    at: Some(now_ms()),
                                });
                            record.wasl_open_assistant = Some(record.transcript.len() - 1);
                        }
                    }
                }
            }
            K::ToolStarted { name, .. } => {
                record
                    .transcript
                    .push(marsad_events::TranscriptEntry::Tool {
                        name,
                        input: String::new(),
                        at: Some(now_ms()),
                    });
            }
            K::ToolFinished { .. } => {
                // Tool completion carries no new transcript content Marsad folds;
                // the ToolStarted row already recorded the activity.
            }
            K::NativeGateRequested { turn_id, detail } => {
                // Map to the EXISTING Gate::Permission pipeline (§8), session
                // scoped. Per-occurrence identity is (turn_id, seq) (§8, v3): the
                // real detail has no gate_id, and turn_id alone is NOT unique per
                // gate (two native gates can fire in one turn), so the event's own
                // `seq` is combined with turn_id via native_gate_key. Store the
                // detail keyed by that gate_key so answer_wasl_native_gate can
                // reconstruct the same gate (→ stable+unique DecisionId).
                let gate_key = marsad_wasl_session::native_gate_key(&turn_id, seq);
                let gate = native_gate_permission(&gate_key, &detail);
                record.wasl_native_gates.insert(gate_key, detail);
                record.state.gate = Some(gate);
                record.state.phase = Phase::Needs;
            }
            K::TurnCompleted { reply, .. } => {
                close_wasl_turn(record, &reply);
                // A completed turn is not awaiting input.
                record.state.phase = Phase::Done;
            }
            K::TurnFailed { error, .. } => {
                close_wasl_turn(record, &error);
                record.state.phase = Phase::Error;
            }
            // Informational lifecycle/correlation events Marsad folds no
            // transcript content from (§4.4, v3): SessionReady is the connect
            // ping; TurnQueued/StatusChanged/NativeGateResolved/TurnCancelled/
            // TurnExpired are correlation/status signals whose UI effect (queued
            // indicator, phase) is already driven by the WaslSessionStatus
            // projection and the receipt path, so folding them here would
            // double-count. No-op (never a transcript row, never a panic).
            K::SessionReady { .. }
            | K::TurnQueued { .. }
            | K::NativeGateResolved { .. }
            | K::TurnCancelled { .. }
            | K::TurnExpired { .. }
            | K::StatusChanged { .. } => {}
            K::SessionExited { .. } => {
                record.live = false;
                record.state.phase = Phase::Done;
            }
            K::ObserveLost { .. } => {
                record.state.phase = Phase::Detached;
            }
            K::Unknown(tag) => {
                eprintln!("marsad-core: WaslSessionEvent for {session_id}: unknown kind '{tag}'");
            }
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Persist the WaslManaged session's advanced `wasl_last_seq` into its store
    /// binding so a reconnect re-attaches from the last observed seq (§6.1).
    /// Best-effort — a store error never fails event folding.
    fn persist_wasl_last_seq(&mut self, session_id: &str) {
        // Caller (`handle_wasl_session_event`) verified the session exists
        // immediately before calling, and the single-threaded `&mut self` call
        // path removes nothing in between — so the session is always present.
        let r = self
            .sessions
            .get(session_id)
            .expect("persist_wasl_last_seq called with a session verified present by the caller");
        let (binding, last_seq) = match &r.runtime_owner {
            RuntimeOwner::WaslManaged { binding } => (binding.clone(), r.wasl_last_seq),
            RuntimeOwner::MarsadManaged => return,
        };
        let _ = self
            .store
            .upsert_wasl_binding(&marsad_store::PersistedWaslBinding {
                session_id: session_id.to_string(),
                target_id: binding.target_id,
                host_id: binding.host_id,
                provider_key: binding.provider_key,
                last_seq,
            });
    }

    /// Surface a Wasl prompt receipt VISIBLY (§10.4): a `Queued`/`RejectedBusy`
    /// receipt is a toast + session update, not a silent drop. A `Queued` prompt
    /// is NOT shown as an in-flight (Working) turn — only `TurnAccepted`/`Started`
    /// makes it in-flight. Unknown session / malformed receipt → tolerant no-op.
    fn handle_wasl_prompt_receipt(
        &mut self,
        session_id: String,
        receipt: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            return Ok(vec![]);
        }
        let r: marsad_wasl_session::WaslTurnReceipt = match serde_json::from_value(receipt) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("marsad-core: WaslPromptReceipt for {session_id}: bad receipt: {e}");
                return Ok(vec![]);
            }
        };
        use marsad_wasl_session::WaslTurnReceipt as R;
        // v3: a `Duplicate { original }` is a benign resent idempotency key — Wasl
        // returned the ORIGINAL receipt wrapped verbatim with no second injection.
        // The wrapper is transparent to disposition: unwrap it (recursively, in
        // case of nesting) and surface whatever the original was (a Started stays
        // Working, a RejectedBusy still surfaces the busy signal, …), never a
        // spurious error.
        let effective = unwrap_duplicate_receipt(&r);
        let (toast, phase): (Option<(String, String)>, Option<Phase>) = match effective {
            R::Started { .. } => (None, Some(Phase::Working)),
            R::Queued { position, .. } => (
                Some((
                    "⏳".to_string(),
                    format!("Prompt queued (position {position})"),
                )),
                // A merely-queued prompt is NOT in-flight — leave the calm bucket.
                Some(Phase::Done),
            ),
            R::RejectedBusy { .. } => (
                Some((
                    "⚠".to_string(),
                    "Session busy — prompt rejected".to_string(),
                )),
                Some(Phase::Done),
            ),
            R::RejectedUnavailable { .. } => (
                Some((
                    "⚠".to_string(),
                    "Session unavailable — prompt rejected".to_string(),
                )),
                Some(Phase::Error),
            ),
            // `unwrap_duplicate_receipt` already peeled every Duplicate layer, so
            // the effective receipt is never a Duplicate here. Keep the match
            // exhaustive without a real branch.
            R::Duplicate { .. } => (None, None),
        };
        if let (Some(p), Some(record)) = (phase, self.sessions.get_mut(&session_id)) {
            record.state.phase = p;
        }
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let mut updates = vec![VmUpdate::Session(sv)];
        if let Some((icon, text)) = toast {
            updates.push(VmUpdate::Toast { icon, text });
        }
        Ok(updates)
    }

    /// Apply a remote transcript backfill: parse the agent-shipped `.jsonl` into
    /// transcript entries and prepend them to the session's transcript (history
    /// first, ahead of any live entries). This is how a resumed/restored REMOTE
    /// session gets its prior conversation — the file lives on the remote host,
    /// so the agent reads it over the session's connection and forwards the text.
    /// Also aggregates per-model token usage (including subagent turns recorded as
    /// `isSidechain: true` entries in the JSONL) and merges it into the session's
    /// `model_usage` map (marked `estimated: true`) so the Usage tab shows a full
    /// multi-model breakdown. U4: this is a BACKFILL-ONLY estimate — it never
    /// overwrites or deletes a row a live `result` event already set for the
    /// same or a different model (see the merge logic below).
    /// Unknown session or empty history+usage → no-op (no update).
    fn handle_transcript_backfill(
        &mut self,
        session_id: String,
        jsonl_text: String,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let history = discovery::parse_transcript(&jsonl_text);
        // Aggregate per-model usage from the full JSONL (includes subagent turns).
        let model_usage_agg = discovery::aggregate_model_usage(&jsonl_text);

        if history.is_empty() && model_usage_agg.is_empty() {
            return Ok(vec![]);
        }
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]),
        };
        // Re-derive activity on reconnect: a session we just re-attached to, which
        // has prior history and no active turn, is idle → Done. Backfill runs
        // before any live stream events (is_first_attach), so a truly-working
        // reconnected session's next assistant/result event re-drives the phase.
        // Only supersede a restored/unknown phase (Starting/Stale/Detached) — never
        // clobber an already-active Working/Needs/Error.
        if matches!(
            record.state.phase,
            Phase::Starting | Phase::Stale | Phase::Detached
        ) {
            record.state.phase = Phase::Done;
        }
        // Prepend history so it precedes any live entries already streamed.
        if !history.is_empty() {
            let mut combined = history;
            combined.append(&mut record.transcript);
            record.transcript = combined;
        }
        // Populate model_usage from JSONL aggregation. U4 cost truthfulness: this
        // is a BACKFILL-ONLY ESTIMATE (recomputed from the local litellm/hand
        // pricing table by `discovery::aggregate_model_usage`) — it must never
        // clobber a row a live `result` event already set (agent-reported,
        // authoritative). So: drop only the PREVIOUSLY estimated rows (a fresh
        // backfill supersedes a stale one), keep every live row untouched, and
        // skip inserting an estimated row for any model that still has a live
        // row after that.
        if !model_usage_agg.is_empty() {
            // Seed total tokens from the JSONL aggregation if no live `result`
            // event has set them yet (tokens == 0). This ensures counters show
            // the accumulated total from the transcript history immediately on
            // attach/reconnect, and then GROW as new result events arrive
            // (reduce_result replaces state.tokens with the authoritative value).
            if record.state.tokens == 0 {
                let total_tokens: u64 = model_usage_agg.values().map(|s| {
                    s.input_tokens
                        .saturating_add(s.output_tokens)
                        .saturating_add(s.cache_read_input_tokens)
                        .saturating_add(s.cache_creation_input_tokens)
                }).sum();
                if total_tokens > 0 {
                    record.state.tokens = total_tokens;
                    // Also seed cost_usd (used directly in make_session_view when
                    // cost_estimated is false — but here cost_estimated will be true
                    // since we're only setting estimated rows, so make_session_view
                    // will sum the model_usage rows instead; leave cost_usd = 0.0).
                    // Recompute context_pct from the seeded token total.
                    let window = record.state.model.as_deref()
                        .map(marsad_events::SessionState::context_window_for)
                        .unwrap_or(200_000);
                    let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                    record.state.context_pct = pct.min(100.0);
                }
            }
            record.state.model_usage.retain(|_, mu| !mu.estimated);
            for (model, summary) in model_usage_agg {
                if record.state.model_usage.contains_key(&model) {
                    // A live row survived the retain above — never overwrite it
                    // with an estimate.
                    continue;
                }
                record.state.model_usage.insert(
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
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Apply a remote changes update: parse the agent-shipped file-change list
    /// into `Vec<FileChange>` and replace the session's changes list. This is
    /// how a REMOTE session's Changes tab stays populated — the file lives on
    /// the remote host, so the agent probes git after each result event and
    /// ships the file list here. Unknown session or empty list → no-op.
    fn handle_changes_update(
        &mut self,
        session_id: String,
        changes_val: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let parsed: Vec<FileChange> = serde_json::from_value(changes_val).unwrap_or_default();
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]),
        };
        record.changes = parsed;
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Apply a project-wide usage update from the agent's python3 aggregation
    /// script.  The `usage` field is a JSON object keyed by model name with
    /// `{input, output, cache_read, cache_creation}` integer values. U4 cost
    /// truthfulness: the cost figure here is recomputed via the local
    /// litellm/hand pricing table (`discovery::model_cost_usd`) — an ESTIMATE,
    /// not agent-reported. It supersedes any previously ESTIMATED row (backfill
    /// or an earlier usage probe) but must never overwrite or delete a row a
    /// live `result` event already set for the same or a different model.
    ///
    /// Unknown session or missing session_id → error / no-op.
    /// Two-axis lifecycle: apply an authoritative liveness signal from the
    /// agent's reader thread. `true` = Resumed (process alive on its host),
    /// `false` = Paused (host reboot / OOM / scheduler kill / crash / claude
    /// exit / reconnect gave up). This is the truthful source of the
    /// Paused-on-host-death transition — never inferred from the turn phase.
    /// Unknown session or no change → no-op (avoid board churn).
    fn handle_session_liveness(
        &mut self,
        session_id: &str,
        live: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        match self.sessions.get_mut(session_id) {
            Some(rec) if rec.live != live => {
                rec.live = live;
                // Connection health is meaningful only while the session is running.
                // Reset it to Connected on ANY liveness flip: coming back Resumed
                // starts clean (the reader emits Connected on attach right after);
                // going Paused/finalized must not linger on a stale
                // "Reconnecting"/"Unstable"/"Disconnected" hint.
                rec.conn_health = marsad_proto::ConnHealth::Connected;
                // Pausing kills the process → any pending gate/question is wasted
                // (unanswerable until a cold resume). Discard it so it neither
                // lingers in state nor resurfaces on resume.
                if !live {
                    rec.state.drop_gates_on_pause();
                }
            }
            _ => return Ok(vec![]),
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

    /// Per-session connection health from the agent's reader (real stream events,
    /// not the host poll). Mirrors `handle_session_liveness`. A no-op when the
    /// value is unchanged so it doesn't churn the board.
    fn handle_session_conn(
        &mut self,
        session_id: &str,
        health: marsad_proto::ConnHealth,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        match self.sessions.get_mut(session_id) {
            Some(rec) if rec.conn_health != health => rec.conn_health = health,
            _ => return Ok(vec![]),
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

    fn handle_usage_update(
        &mut self,
        session_id: String,
        usage_val: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if usage_val.is_null() {
            return Ok(vec![]); // no usage data → no-op
        }
        let usage_obj = match usage_val.as_object() {
            Some(o) => o,
            None => return Ok(vec![]), // malformed → no-op
        };

        // Parse into ModelUsageRow vec, skipping "<synthetic>" and zero-usage rows.
        let mut rows: Vec<marsad_viewmodel::ModelUsageRow> = Vec::new();
        for (model, val) in usage_obj {
            if model == "<synthetic>" {
                continue;
            }
            let input = val.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
            let output = val.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_read = val.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_creation = val
                .get("cache_creation")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // Skip zero-usage rows.
            if input == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
                continue;
            }
            let cost = discovery::model_cost_usd(model, input, output, cache_read, cache_creation);
            rows.push(marsad_viewmodel::ModelUsageRow {
                model: model.clone(),
                input_tokens: input,
                output_tokens: output,
                cost_usd: cost,
                estimated: true,
            });
        }
        rows.sort_by(|a, b| a.model.cmp(&b.model));

        // Guard: an empty rows vec means the probe returned no real usage data
        // (e.g. an empty project dir, or the script exited early). Do NOT clear
        // the existing model_usage — that would wipe good data from a previous
        // result-event or backfill. Return no update so the UI is unchanged.
        if rows.is_empty() {
            return Ok(vec![]);
        }

        // Find the session and update its model_usage.
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]), // unknown session → no-op
        };
        // Seed total tokens from the usage probe if no live `result` event has
        // set them yet (tokens == 0). The usage probe gives the CUMULATIVE
        // per-project total — use it as the token baseline so counters show
        // the accumulated total on attach rather than starting from zero.
        // A subsequent live `result` event will authoritatively replace this.
        if record.state.tokens == 0 {
            let total_tokens: u64 = rows.iter().map(|r| {
                r.input_tokens.saturating_add(r.output_tokens)
            }).sum();
            if total_tokens > 0 {
                record.state.tokens = total_tokens;
                let window = record.state.model.as_deref()
                    .map(marsad_events::SessionState::context_window_for)
                    .unwrap_or(200_000);
                let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                record.state.context_pct = pct.min(100.0);
            }
        }
        // Merge the estimated rows into the in-state model_usage map so
        // make_session_view picks it up. U4: drop only the PREVIOUSLY estimated
        // rows (this probe supersedes a stale one) and never touch — or
        // overwrite with an estimate — a row a live `result` event set.
        record.state.model_usage.retain(|_, mu| !mu.estimated);
        for row in &rows {
            if record.state.model_usage.contains_key(&row.model) {
                // A live row survived the retain above — never overwrite it
                // with an estimate.
                continue;
            }
            record.state.model_usage.insert(
                row.model.clone(),
                marsad_proto::ModelUsage {
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cost_usd: row.cost_usd,
                    estimated: true,
                },
            );
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    // -----------------------------------------------------------------------
    // X3 Wasl inbound handlers (docs/specs/wasl-connector.md §5.2)
    // -----------------------------------------------------------------------

    /// Snapshot `wasl_gates` as a deterministically ordered `Vec` (the
    /// `BTreeMap` key is `(host_id, gate_id)`, so iteration order is
    /// lexicographic).
    fn wasl_gates_list(&self) -> Vec<WaslGateView> {
        self.wasl_gates.values().cloned().collect()
    }

    /// A Wasl gate surfaced (or re-surfaced via resnapshot reconcile).
    /// `decidable == false` (observe-only) is a tolerant no-op — those gates
    /// are stream-visible but never pending/answerable. A malformed `request`
    /// Value is also a tolerant skip (observational data must never wedge the
    /// reader loop, same disposition as `ChangesUpdate`).
    fn handle_wasl_gate_requested(
        &mut self,
        host_id: String,
        gate_id: String,
        decidable: bool,
        request: serde_json::Value,
        requested_at: String,
        deadline: String,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !decidable {
            return Ok(vec![]);
        }
        let req: marsad_wasl::GateRequest = match serde_json::from_value(request) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "marsad-core: malformed WaslGateRequested.request for {host_id}/{gate_id}: {e}"
                );
                return Ok(vec![]);
            }
        };
        self.wasl_gates.insert(
            (host_id.clone(), gate_id.clone()),
            WaslGateView {
                host_id,
                gate_id,
                kind: req.kind,
                from: req.from,
                summary: req.summary,
                preview: req.preview,
                requested_at,
                deadline,
            },
        );
        Ok(vec![VmUpdate::WaslGates(self.wasl_gates_list())])
    }

    /// A Wasl gate resolved. Removal is unconditional-attempt: if the gate
    /// was tracked, the pending list update is also emitted; an
    /// already-cleared/unknown gate still gets the `WaslGateDecided` event
    /// (silent-consistent with a race the node itself already resolved).
    fn handle_wasl_gate_decided(
        &mut self,
        host_id: String,
        gate_id: String,
        outcome: String,
        decided_by: String,
        reason: Option<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let removed = self
            .wasl_gates
            .remove(&(host_id.clone(), gate_id.clone()))
            .is_some();
        let mut updates = vec![VmUpdate::WaslGateDecided {
            host_id,
            gate_id,
            outcome,
            decided_by,
            reason,
        }];
        if removed {
            updates.push(VmUpdate::WaslGates(self.wasl_gates_list()));
        }
        Ok(updates)
    }

    /// One audit-feed entry. A malformed `entry` Value is a tolerant skip
    /// (same disposition as the malformed-request case above).
    fn handle_wasl_observation(
        &mut self,
        host_id: String,
        entry: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let parsed: marsad_wasl::AuditEntry = match serde_json::from_value(entry) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("marsad-core: malformed WaslObservation.entry for {host_id}: {e}");
                return Ok(vec![]);
            }
        };
        Ok(vec![VmUpdate::WaslObservation {
            host_id,
            entry: parsed,
        }])
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn handle_agent_event(
        &mut self,
        session_id: String,
        event: marsad_proto::AgentEvent,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        // An agent_event for a session this core never registered (e.g. one the
        // agent supervises but we have not launched/adopted) is an UnknownSession,
        // not a panic.
        self.require_session(&session_id)?;

        // Evidence of life: a live stream event proves the process is running, so
        // mark it Resumed. This is the robust fallback for the SessionLiveness
        // one-shot (e.g. if core subscribed just after the reader emitted it):
        // any inbound event flips a restored/Paused session to Resumed. The
        // SessionView built below picks this up.
        if let Some(rec) = self.sessions.get_mut(&session_id) {
            rec.live = true;
        }

        // Extract transcript entries from the typed event BEFORE reduce consumes it.
        // Pair each entry with a dedup key; the append loop below inserts only
        // newly-seen keys.
        //
        // AssistantDelta → TranscriptEntry::Assistant, dedup key = "<msg_id>\0a\0<text>"
        // ToolCallStarted → TranscriptEntry::Tool, dedup key = call_id (globally unique)
        // ThinkingDelta → SKIP (preserves old behaviour: thinking blocks are not transcribed)
        // Compaction → appended AFTER reduce (in sequence, below)
        // everything else → no transcript entry
        let transcript_entries: Vec<(String, marsad_events::TranscriptEntry)> = match &event {
            marsad_proto::AgentEvent::AssistantDelta { text, message_id } => {
                let t = text.trim();
                if t.is_empty() {
                    vec![]
                } else {
                    let key = format!(
                        "{}\u{0}a\u{0}{}",
                        message_id.as_deref().unwrap_or(""),
                        t
                    );
                    vec![(
                        key,
                        marsad_events::TranscriptEntry::Assistant {
                            text: t.to_string(),
                            at: Some(now_ms()),
                        },
                    )]
                }
            }
            marsad_proto::AgentEvent::ToolCallStarted {
                name,
                call_id,
                input,
                ..
            } => {
                let input_str = serde_json::to_string(input).unwrap_or_default();
                vec![(
                    call_id.clone(),
                    marsad_events::TranscriptEntry::Tool {
                        name: name.clone(),
                        input: input_str,
                        at: Some(now_ms()),
                    },
                )]
            }
            // ThinkingDelta: skip (old code skipped thinking blocks)
            marsad_proto::AgentEvent::ThinkingDelta { .. } => vec![],
            _ => vec![],
        };

        // Capture compaction metadata BEFORE event is consumed by reduce, so
        // the live transcript gets an in-sequence Compaction entry (mirrors what
        // parse_transcript does for the backfill/JSONL path).
        let compact_entry: Option<marsad_events::TranscriptEntry> =
            if let marsad_proto::AgentEvent::Compaction { info } = &event {
                Some(marsad_events::TranscriptEntry::Compaction {
                    trigger: info.trigger.clone(),
                    dropped_tokens: info.cumulative_dropped_tokens,
                    at: Some(now_ms()),
                })
            } else {
                None
            };

        // Detect a SessionInit BEFORE consuming the event — we need to
        // know if slash_commands are about to be set so we can persist them.
        let is_system_init = matches!(&event, marsad_proto::AgentEvent::SessionInit { .. });
        // Detect a TurnCompleted (the new Result) BEFORE consuming event — after
        // reduce we re-aggregate usage from the full JSONL for LOCAL sessions so
        // the displayed total reflects the whole conversation, not just the current
        // run (which is all TurnCompleted.usage.cost_usd contains for a resumed session).
        let is_result = matches!(&event, marsad_proto::AgentEvent::TurnCompleted { .. });

        // Reduce the event, then decide whether the working tree may have changed.
        // Safe: require_session above guarantees the entry exists.
        let (project, host_id, should_probe, board_changed) = {
            let record = self.sessions.get_mut(&session_id).unwrap();
            let old_state = record.state.clone();
            let old_last_action = old_state.last_action.clone();
            let was_finished = matches!(old_state.phase, Phase::Done | Phase::Error);
            let old_phase = old_state.phase.clone();
            let old_gate_present = old_state.gate.is_some();
            record.state = reduce(old_state, Event::Stream(event));
            // Re-probe git when a tool just ran (files may have changed) or the
            // session just reached a terminal phase — not on every token.
            let tool_ran = record.state.last_action != old_last_action;
            let just_finished =
                matches!(record.state.phase, Phase::Done | Phase::Error) && !was_finished;
            // The workstream board's attention card derives from phase + pending
            // gate; refresh it only when one of those actually changed (a gate
            // appeared/cleared or the phase moved) — never on every token.
            let board_changed = record.state.phase != old_phase
                || record.state.gate.is_some() != old_gate_present;
            (
                record.project.clone(),
                record.host_id.clone(),
                tool_ran || just_finished,
                board_changed,
            )
        };

        // Persist the claude session UUID, slash_commands, tools, and mcp_servers
        // after a SystemInit so that on the next attach/reconnect (when send_init
        // is false) the restored session shows them without waiting for another
        // system/init.
        //
        // The UUID is the critical one: a Fresh launch persists session_id=None
        // (the real claude conversation id is unknown until system/init arrives),
        // and that id lives ONLY in memory (`state.session_id`) unless we write it
        // back here. Without this, on the next app restart two sessions in the SAME
        // directory both load session_id=None and fall back to "most recent
        // .jsonl" (remote_transcript_script `ls -t | head -1`) → they COLLAPSE onto
        // the same claude session. Persisting the id makes each session resume /
        // backfill its OWN conversation. Guarded to the claude driver: codex's
        // synthesized system/init carries the codex thread_id in `session_id`,
        // which is NOT a `claude --resume` target and must not be persisted there.
        // Best-effort: a store error here must not fail the event processing.
        if is_system_init {
            let (claude_sid, slash_cmds, tools, mcp_servers) = self
                .sessions
                .get(&session_id)
                .map(|r| (
                    r.state.session_id.clone(),
                    r.state.slash_commands.clone(),
                    r.state.tools.clone(),
                    r.state.mcp_servers.clone(),
                ))
                .unwrap_or_default();
            let has_sid = claude_sid.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
            if has_sid || !slash_cmds.is_empty() || !tools.is_empty() || !mcp_servers.is_empty() {
                // Load the persisted session, update fields, write back.
                if let Ok(restored) = self.store.restore() {
                    if let Some(mut ps) = restored.sessions.into_iter().find(|s| s.id == session_id) {
                        if has_sid && ps.driver == "claude" && ps.session_id.as_deref() != claude_sid.as_deref() {
                            ps.session_id = claude_sid;
                        }
                        ps.slash_commands = slash_cmds;
                        ps.tools = tools;
                        ps.mcp_servers = mcp_servers;
                        let _ = self.store.upsert_session(&ps);
                    }
                }
            }
        }

        // After a Result event, re-aggregate usage from the full session JSONL for
        // LOCAL sessions. reduce_result sets cost/tokens from result.total_cost_usd
        // which covers only the CURRENT run, not prior resumed runs — re-aggregating
        // here ensures the displayed total reflects the whole conversation history.
        // Remote sessions are left untouched (they seed via UsageUpdate IPC).
        if is_result && self.host_is_local(&host_id) {
            let record = self.sessions.get_mut(&session_id).unwrap();
            if let Some(sid) = record.state.session_id.as_deref().filter(|s| !s.is_empty()) {
                let usage_agg = self.transcript.session_usage(&project, sid);
                if !usage_agg.is_empty() {
                    let total_tokens: u64 = usage_agg.values().map(|u| {
                        u.input_tokens
                            .saturating_add(u.output_tokens)
                            .saturating_add(u.cache_read_input_tokens)
                            .saturating_add(u.cache_creation_input_tokens)
                    }).sum();
                    if total_tokens > 0 {
                        record.state.tokens = total_tokens;
                        let window = record.state.model.as_deref()
                            .map(marsad_events::SessionState::context_window_for)
                            .unwrap_or(200_000);
                        let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                        record.state.context_pct = pct.min(100.0);
                        record.state.cost_usd = usage_agg.values().map(|u| u.cost_usd).sum();
                        record.state.model_usage.clear();
                        for (model, summary) in usage_agg {
                            record.state.model_usage.insert(
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
        }

        // Only probe the LOCAL working tree for LOCAL sessions. REMOTE sessions
        // get their changes from the agent's `ChangesUpdate` IPC — a local `git
        // status` on a remote project path returns nothing and would CLOBBER the
        // real (remote) changes on the next tool_use/result event.
        if should_probe && self.host_is_local(&host_id) {
            let changes = self.git.changes(&project);
            // Safe: entry still exists (single-threaded, no removal in between).
            self.sessions.get_mut(&session_id).unwrap().changes = changes;
        }

        // Append this event's NEW transcript entries. Deduped by the key
        // computed above (AssistantDelta: "<msg_id>\0a\0<text>"; ToolCallStarted:
        // call_id). Only entries whose key was NOT already in seen_msg_ids are
        // pushed — preserving replay-safety exactly as before.
        if !transcript_entries.is_empty() {
            let record = self.sessions.get_mut(&session_id).unwrap();
            for (key, entry) in transcript_entries {
                if record.seen_msg_ids.insert(key) {
                    record.transcript.push(entry);
                }
            }
        }

        // Append a Compaction entry in sequence for a live compact_boundary event
        // so the transcript reflects the chronological position (mirrors what
        // parse_transcript does for the JSONL backfill path).
        if let Some(entry) = compact_entry {
            self.sessions
                .get_mut(&session_id)
                .unwrap()
                .transcript
                .push(entry);
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        let mut updates = vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)];
        // Refresh the workstream board's attention card when this event changed a
        // board-relevant field (gate appeared/cleared, phase moved) so a needs-you
        // card shows the pending gate live instead of staying stale from startup.
        if board_changed {
            updates.push(VmUpdate::Board(self.board()));
        }
        Ok(updates)
    }
}
