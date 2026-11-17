//! Workstream & profile commands (B12) — node projection, workstream CRUD,
//! connections, profiles, and session assignment. Extracted from `lib.rs`
//! verbatim; `use super::*` brings crate types + root private plumbing.

use super::*;

impl Core {
    // -----------------------------------------------------------------------
    // Workstream commands (B12 — additive)
    // -----------------------------------------------------------------------

    // (gate_headline is a free fn — see below node projection helpers.)

    /// Project one session into a `marsad_workstream::Node`, mirroring
    /// `make_session_view`. Caller guarantees `id` has a membership entry.
    fn node_of(&self, id: &str, record: &SessionRecord) -> marsad_workstream::Node {
        let ws = self.membership.get(id).cloned().unwrap_or_default();
        let lead = self.ws_leads.get(&ws).map(String::as_str) == Some(id);
        // `record.driver` is a DISPLAY marker (routing keys on RuntimeOwner, not
        // this string). A WaslManaged session stores its NATIVE protocol here
        // ("claude"/"codex"/"opencode") so the graph shows the native name — the
        // user never sees "wasl". A legacy record still stamped bare "wasl" (or
        // anything unknown) falls back to Claude.
        let driver = match record.driver.as_str() {
            "codex" => marsad_workstream::Driver::Codex,
            "opencode" => marsad_workstream::Driver::Opencode,
            _ => marsad_workstream::Driver::Claude,
        };
        marsad_workstream::Node {
            id: id.to_string(),
            ws,
            name: record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, id)),
            driver,
            host: record.host_id.clone(),
            state: record.state.phase.clone(),
            lead,
            branch: String::new(),
            live: record.live,
            end_reason: record.state.end_reason,
            context_pct: record.state.context_pct.round().clamp(0.0, 100.0) as u8,
            // Surface a concise pending-gate summary so the workstream board's
            // attention card can show WHAT needs the human, not just the name.
            status: record
                .state
                .gate
                .as_ref()
                .map(gate_headline)
                .unwrap_or_default(),
        }
    }

    /// One node per session that HAS a membership entry, in `session_order`.
    pub fn nodes(&self) -> Vec<marsad_workstream::Node> {
        self.session_order
            .iter()
            .filter(|id| self.membership.contains_key(id.as_str()))
            .filter_map(|id| self.sessions.get(id).map(|record| self.node_of(id, record)))
            .collect()
    }

    pub(crate) fn workstreams_update(&self) -> VmUpdate {
        VmUpdate::Workstreams(self.workstreams.clone())
    }

    /// Current workstream list for bridge commands that need to push a
    /// Workstreams VmUpdate after mutating state (e.g. remove_session).
    pub fn workstreams(&self) -> &[marsad_workstream::Workstream] {
        &self.workstreams
    }

    /// Restore workstream state from the store, then run the (idempotent)
    /// legacy-workspace / unassigned-session migration.
    ///
    /// Mirrors `load_workspaces`/`load_sessions`/`load_hosts`: the initial
    /// `restore()` is the hard-error path (propagated as `CoreError::Agent`);
    /// every migration write after that is best-effort (mirrors
    /// `set_workspace`'s `set_active_workspace`) so the in-memory Core state
    /// — and therefore the returned `VmUpdate`s and subsequent `board()`/
    /// `nodes()` calls — stays correct even if an underlying persistence
    /// write fails.
    pub fn load_workstreams(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore workstreams: {e:?}")))?;
        self.workstreams = restored.workstreams;
        self.connections = restored.connections;
        self.profiles = restored.profiles;
        self.membership = restored.membership;
        // Rebuild ws_leads from the restored workstreams' single-node fast lanes
        // is not possible from persisted data alone (lead is a Core-only concept
        // not persisted separately); migration below re-derives leads for any
        // newly-created fast-lane workstreams. Pre-existing leads are inferred
        // from membership: a session that is the ONLY member of its workstream
        // and was previously marked lead by a prior migration run remains so
        // because ws_leads itself is only ever populated by Core, in-memory, and
        // migration is idempotent per the checks below.

        // 1. Seed profiles from legacy workspaces if none exist yet.
        if self.profiles.is_empty() {
            let store_workspaces: Vec<StoreWorkspace> =
                self.workspaces.iter().map(Self::vm_ws_to_store).collect();
            let seeded = marsad_store::profiles_from_workspaces(&store_workspaces);
            for p in &seeded {
                let _ = self.store.upsert_profile(p);
            }
            self.profiles = seeded;
        }

        // 2. Every session with no membership entry becomes a single-node
        //    fast-lane workstream, led by itself.
        let unassigned: Vec<String> = self
            .session_order
            .iter()
            .filter(|id| !self.membership.contains_key(id.as_str()))
            .cloned()
            .collect();
        for id in unassigned {
            // `id` comes from `session_order`, which always has a matching
            // `sessions` entry (the two are only ever mutated together).
            let record = self
                .sessions
                .get(&id)
                .expect("session_order entries always have a sessions record");
            let name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id);
        }
        let _ = self.store.set_membership(&self.membership);

        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Create (or overwrite, upsert semantics) a workstream.
    pub fn create_workstream(
        &mut self,
        id: String,
        name: String,
        goal: String,
        profile: Option<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let ws = marsad_workstream::Workstream {
            id: id.clone(),
            name,
            goal,
            profile,
        };
        self.store
            .upsert_workstream(&ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        if let Some(pos) = self.workstreams.iter().position(|w| w.id == id) {
            self.workstreams[pos] = ws;
        } else {
            self.workstreams.push(ws);
        }
        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Pause (stop) all sessions in a workstream. Mirrors `stop_session` for
    /// each member: sends `StopSession` IPC + marks phase Stale in memory.
    /// Sessions remain resumable. Absent workstream or no members → no-op `Ok`.
    pub fn pause_workstream(&mut self, ws_id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // Collect member session ids (cloned to avoid borrowing self).
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == ws_id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut all_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            // stop_session is require_session-guarded: skip unknown sessions gracefully.
            if let Ok(updates) = self.stop_session(&sid) {
                all_updates.extend(updates);
            }
        }
        Ok(all_updates)
    }

    /// Resume all sessions in a workstream. Mirrors `resume_session` for each
    /// member. Absent workstream or no members → no-op `Ok`.
    pub fn resume_workstream(&mut self, ws_id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == ws_id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut all_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            if let Ok(updates) = self.resume_session(&sid) {
                all_updates.extend(updates);
            }
        }
        Ok(all_updates)
    }

    /// Delete a workstream: first kills all member sessions (remove_session for
    /// each — StopSession IPC + drops record + removes from store so they don't
    /// reappear on relaunch), THEN deletes the workstream record, cascades its
    /// connections and membership/lead entries. Deleting an absent id is a no-op `Ok`.
    pub fn delete_workstream(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // 1. Kill + remove all member sessions first (so they don't orphan a
        //    running process, and so deleted sessions vanish from the dashboard).
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut session_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            // remove_session is require_session-guarded: skip unknown sessions gracefully.
            if let Ok(updates) = self.remove_session(&sid) {
                session_updates.extend(updates);
            }
        }

        // 2. Delete the workstream record and its connections.
        let _ = self.store.delete_workstream(id);
        self.workstreams.retain(|w| w.id != id);

        let conn_ids: Vec<String> = self
            .connections
            .iter()
            .filter(|c| c.ws == id)
            .map(|c| c.id.clone())
            .collect();
        for cid in conn_ids {
            let _ = self.store.delete_connection(&cid);
        }
        self.connections.retain(|c| c.ws != id);

        self.membership.retain(|_, ws| ws != id);
        let _ = self.store.set_membership(&self.membership);
        self.ws_leads.remove(id);

        // 3. Return session-removed events + board updates so the UI stays consistent.
        let mut updates = session_updates;
        updates.push(self.workstreams_update());
        updates.push(VmUpdate::Board(self.board()));
        Ok(updates)
    }

    /// Validate and add a connection between two nodes in the same workstream.
    pub fn add_connection(
        &mut self,
        id: String,
        ws: String,
        from: String,
        to: String,
        bi: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let conn = marsad_workstream::Connection {
            id,
            ws,
            from,
            to,
            bi,
            gate: None,
        };
        marsad_workstream::validate_connection(&conn, &self.nodes(), &self.connections)
            .map_err(|e| CoreError::Connection(e.to_string()))?;
        self.store
            .upsert_connection(&conn)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;

        // Dynamic peer registration: register the `from` node as a peer of the
        // `to` node on the Wasl runtime. Best-effort — a send error does NOT fail
        // the connection op (the persist+graph update above is already committed).
        self.emit_register_wasl_peer(&conn.from, &conn.to);
        // For bidirectional connections also register `to` as a peer of `from`.
        if conn.bi {
            self.emit_register_wasl_peer(&conn.to, &conn.from);
        }

        self.connections.push(conn);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    /// Remove a connection by id. Absent id is a no-op `Ok`.
    pub fn remove_connection(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // Capture the connection's (from, to) BEFORE removing it so we can emit
        // the unregister IPC. Best-effort — missing id is a no-op.
        if let Some(conn) = self.connections.iter().find(|c| c.id == id) {
            // Clone fields before the mutable retain below.
            let from = conn.from.clone();
            let to = conn.to.clone();
            let bi = conn.bi;
            self.emit_unregister_wasl_peer(&from, &to);
            if bi {
                self.emit_unregister_wasl_peer(&to, &from);
            }
        }
        self.store
            .delete_connection(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.connections.retain(|c| c.id != id);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    // -----------------------------------------------------------------------
    // Private helpers for dynamic Wasl peer registration
    // -----------------------------------------------------------------------

    /// Look up the host_id for `session_id` from the sessions map. Returns
    /// `None` when the session is unknown (e.g. not yet launched).
    fn session_host_id(&self, session_id: &str) -> Option<String> {
        self.sessions.get(session_id).map(|r| r.host_id.clone())
    }

    /// Best-effort: emit `RegisterWaslPeer` to the agent so the `to_session`'s
    /// node learns about `from_session` as a peer. Silently logs on error.
    fn emit_register_wasl_peer(&mut self, from_session: &str, to_session: &str) {
        let from_host = match self.session_host_id(from_session) {
            Some(h) => h,
            None => {
                eprintln!(
                    "marsad-core: emit_register_wasl_peer: unknown session '{from_session}' \
                     (from) — peer registration skipped"
                );
                return;
            }
        };
        let to_host = match self.session_host_id(to_session) {
            Some(h) => h,
            None => {
                eprintln!(
                    "marsad-core: emit_register_wasl_peer: unknown session '{to_session}' \
                     (to) — peer registration skipped"
                );
                return;
            }
        };

        // The peer id Wasl uses for the `from` node is `marsad-<from_host>`.
        let peer_id = format!("marsad-{from_host}");
        // The A2A endpoint for the `from` node — built from the same formula as
        // the node spawn config (deterministic FNV-1a, port window [48000,48999])
        // so it matches the node's advertised address.
        let peer_endpoint = Some(format!(
            "http://{}",
            marsad_wasl_session::wasl_a2a_bind(&from_host)
        ));

        let msg = IpcMsg::RegisterWaslPeer {
            host_id: to_host.clone(),
            peer_id: peer_id.clone(),
            peer_host: from_host.clone(),
            peer_endpoint,
        };
        if let Err(e) = self.send_ipc(&msg) {
            eprintln!(
                "marsad-core: RegisterWaslPeer send failed (best-effort) \
                 from={from_session} to={to_session}: {e}"
            );
        }
    }

    /// Best-effort: emit `UnregisterWaslPeer` so the `to_session`'s node
    /// removes `from_session` from its peer list.
    fn emit_unregister_wasl_peer(&mut self, from_session: &str, to_session: &str) {
        let from_host = match self.session_host_id(from_session) {
            Some(h) => h,
            None => {
                eprintln!(
                    "marsad-core: emit_unregister_wasl_peer: unknown session '{from_session}' \
                     (from) — peer unregistration skipped"
                );
                return;
            }
        };
        let to_host = match self.session_host_id(to_session) {
            Some(h) => h,
            None => {
                eprintln!(
                    "marsad-core: emit_unregister_wasl_peer: unknown session '{to_session}' \
                     (to) — peer unregistration skipped"
                );
                return;
            }
        };

        let peer_id = format!("marsad-{from_host}");
        let msg = IpcMsg::UnregisterWaslPeer {
            host_id: to_host.clone(),
            peer_id: peer_id.clone(),
        };
        if let Err(e) = self.send_ipc(&msg) {
            eprintln!(
                "marsad-core: UnregisterWaslPeer send failed (best-effort) \
                 from={from_session} to={to_session}: {e}"
            );
        }
    }

    /// Create (or overwrite, upsert semantics) a profile.
    pub fn create_profile(&mut self, id: String, name: String) -> Result<Vec<VmUpdate>, CoreError> {
        let profile = marsad_workstream::Profile {
            id: id.clone(),
            name,
        };
        self.store
            .upsert_profile(&profile)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        // Upsert: drop any existing entry with the same id, then push — a
        // single straight-line path regardless of whether `id` was already
        // present (matches store upsert semantics without a branch).
        self.profiles.retain(|p| p.id != id);
        self.profiles.push(profile);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    /// Delete a profile: un-sets it on member workstreams and clears
    /// `active_profile` if it pointed at the deleted profile. Absent id is a
    /// no-op `Ok`.
    pub fn delete_profile(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let _ = self.store.delete_profile(id);
        self.profiles.retain(|p| p.id != id);

        let affected: Vec<String> = self
            .workstreams
            .iter()
            .filter(|w| w.profile.as_deref() == Some(id))
            .map(|w| w.id.clone())
            .collect();
        for ws_id in affected {
            let ws = self
                .workstreams
                .iter_mut()
                .find(|w| w.id == ws_id)
                .expect("id collected from self.workstreams above");
            ws.profile = None;
            let _ = self.store.upsert_workstream(ws);
        }

        if self.active_profile.as_deref() == Some(id) {
            self.active_profile = None;
        }

        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Assign a session to a workstream, optionally making it the lead.
    pub fn assign_session(
        &mut self,
        session_id: &str,
        ws_id: &str,
        lead: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(session_id)?;
        if !self.workstreams.iter().any(|w| w.id == ws_id) {
            return Err(CoreError::Connection(format!(
                "unknown workstream: {ws_id}"
            )));
        }
        // `launch_session_with_id` auto-creates a single-node fast-lane
        // workstream `ws-<id>` for every fresh launch. When the caller (NodeModal
        // build-it-out) then assigns the session to a DIFFERENT workstream, that
        // auto one is orphaned — it would linger as an empty second card on the
        // board. Remove it here so a build-it-out launch shows exactly one card.
        let auto_ws_id = format!("ws-{session_id}");
        if auto_ws_id != ws_id && self.workstreams.iter().any(|w| w.id == auto_ws_id) {
            let _ = self.store.delete_workstream(&auto_ws_id);
            self.workstreams.retain(|w| w.id != auto_ws_id);
            self.ws_leads.remove(&auto_ws_id);
        }
        self.membership
            .insert(session_id.to_string(), ws_id.to_string());
        if lead {
            self.ws_leads
                .insert(ws_id.to_string(), session_id.to_string());
        }
        self.store
            .set_membership(&self.membership)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Set the board's active profile filter. `None` shows all workstreams.
    pub fn set_active_profile(&mut self, profile: Option<String>) -> Vec<VmUpdate> {
        self.active_profile = profile;
        vec![VmUpdate::Board(self.board())]
    }
}

// ---------------------------------------------------------------------------
// Peer topology helper — pure, no I/O
// ---------------------------------------------------------------------------

/// The peers of a node derived from the workstream connections.
///
/// Direction is the only edge axis (post-#7 — no "kind"):
///   * Directed A→B (`bi == false`): A drives B — A initiates, B only answers.
///   * Consult  A↔B (`bi == true`) : mutual — both can initiate.
///
/// `outbound` = nodes `node_id` can SEND to (initiate toward):
///              `to` of every edge where `from == node_id`, PLUS `from` of every
///              CONSULT edge where `to == node_id` (the reverse leg of a two-way wire).
/// `inbound`  = nodes that can send to `node_id`:
///              `from` of every edge where `to == node_id`, PLUS `to` of every
///              CONSULT edge where `from == node_id`.
///
/// Both slices are de-duplicated and sorted ascending for determinism.
/// A self-edge (`from == to == node_id`) is ignored — a node is not its own peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePeers {
    pub outbound: Vec<String>,
    pub inbound: Vec<String>,
}

/// Return the peer topology for `node_id` derived from the given connection
/// slice. Pure function: no state mutations, no I/O.
pub fn peers_for_node(node_id: &str, connections: &[marsad_workstream::Connection]) -> NodePeers {
    let mut outbound: Vec<String> = Vec::new();
    let mut inbound: Vec<String> = Vec::new();

    for c in connections {
        if c.from == node_id && c.to == node_id {
            continue; // self-edge
        }
        if c.from == node_id {
            outbound.push(c.to.clone());
            if c.bi {
                inbound.push(c.to.clone()); // consult: target can initiate back
            }
        }
        if c.to == node_id {
            inbound.push(c.from.clone());
            if c.bi {
                outbound.push(c.from.clone()); // consult: we can initiate toward source
            }
        }
    }

    outbound.sort();
    outbound.dedup();
    inbound.sort();
    inbound.dedup();

    NodePeers { outbound, inbound }
}
