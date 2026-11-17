//! Store-backed catalog CRUD — hosts, workspaces, and settings, plus their
//! startup loaders and the per-session file-change setter. These all read/write
//! the persistence `Store` and project `VmUpdate`s. Extracted from `lib.rs`
//! verbatim; `use super::*` brings crate types + root private plumbing.

use super::*;

impl Core {
    // -----------------------------------------------------------------------
    // Workspace helpers
    // -----------------------------------------------------------------------

    fn store_ws_to_vm(ws: &StoreWorkspace) -> Workspace {
        Workspace {
            id: ws.id.clone(),
            name: ws.name.clone(),
            session_ids: ws.session_ids.clone(),
            saved_at: ws.saved_at.clone(),
        }
    }

    pub(crate) fn vm_ws_to_store(ws: &Workspace) -> StoreWorkspace {
        StoreWorkspace {
            id: ws.id.clone(),
            name: ws.name.clone(),
            session_ids: ws.session_ids.clone(),
            saved_at: ws.saved_at.clone(),
        }
    }

    pub(crate) fn workspaces_update(&self) -> VmUpdate {
        VmUpdate::Workspaces(self.workspaces.clone())
    }

    // -----------------------------------------------------------------------
    // Host commands
    // -----------------------------------------------------------------------

    /// Load persisted hosts from the store into the registry (call once at
    /// startup). Returns a `Hosts` update for the now-complete list.
    pub fn load_hosts(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Host(format!("restore hosts: {e:?}")))?;
        for h in restored.hosts {
            // localhost is seeded by the registry; its persisted launch config
            // (claude_bin/setup/env) is applied via update so the seeded entry
            // carries it, rather than appending a duplicate.
            if h.is_local {
                let _ = self
                    .hosts
                    .update(&h.id, h.claude_bin, h.setup, h.env, None, h.agent_commands);
            } else {
                self.hosts.load_remote(
                    &h.id,
                    &h.label,
                    &h.target,
                    h.claude_bin,
                    h.setup,
                    h.env,
                    h.agent_commands,
                );
            }
        }
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    // -----------------------------------------------------------------------
    // Workspace commands
    // -----------------------------------------------------------------------

    /// Load persisted workspaces and active_ws from the store (call once at
    /// startup). Returns a `Workspaces` update for the now-complete list.
    pub fn load_workspaces(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore workspaces: {e:?}")))?;
        self.workspaces = restored
            .workspaces
            .iter()
            .map(Self::store_ws_to_vm)
            .collect();
        if let Some(id) = restored.active_ws {
            self.active_ws = Some(id);
        }
        Ok(vec![self.workspaces_update()])
    }


    /// Save the current session list as a new named workspace, persist it,
    /// and return the updated workspace list.
    pub fn save_workspace(
        &mut self,
        name: &str,
        session_ids: Vec<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let id = format!("ws-{}", name.to_lowercase().replace(' ', "-"));
        let saved_at = chrono_now();
        let ws = Workspace {
            id,
            name: name.to_string(),
            session_ids,
            saved_at,
        };
        let store_ws = Self::vm_ws_to_store(&ws);
        self.store
            .snapshot_workspace(&store_ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        // Upsert into in-memory list (overwrite if same id).
        if let Some(pos) = self.workspaces.iter().position(|w| w.id == ws.id) {
            self.workspaces[pos] = ws;
        } else {
            self.workspaces.push(ws);
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Duplicate a workspace by id, persist the copy, and return the updated list.
    pub fn duplicate_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let original = self
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| CoreError::Agent(format!("workspace not found: {id}")))?
            .clone();
        let new_id = format!("{}-copy", original.id);
        let copy = Workspace {
            id: new_id,
            name: format!("{} (copy)", original.name),
            session_ids: original.session_ids.clone(),
            saved_at: chrono_now(),
        };
        let store_ws = Self::vm_ws_to_store(&copy);
        self.store
            .snapshot_workspace(&store_ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.workspaces.push(copy);
        Ok(vec![self.workspaces_update()])
    }

    /// Delete a workspace by id, remove from persistence, and return the updated list.
    pub fn delete_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.store
            .delete_workspace(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.workspaces.retain(|w| w.id != id);
        // If the deleted workspace was active, clear active_ws.
        if self.active_ws.as_deref() == Some(id) {
            self.active_ws = None;
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Export a workspace as a `.marsad.json` string (no VmUpdate; returns JSON).
    pub fn export_workspace(&self, id: &str) -> Result<String, CoreError> {
        self.store
            .export_workspace(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    /// Import a workspace from a `.marsad.json` string, persist it, and return
    /// the updated list.
    pub fn import_workspace(&mut self, json: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let store_ws = self
            .store
            .import_workspace(json)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        let ws = Self::store_ws_to_vm(&store_ws);
        // Upsert into in-memory list.
        if let Some(pos) = self.workspaces.iter().position(|w| w.id == ws.id) {
            self.workspaces[pos] = ws;
        } else {
            self.workspaces.push(ws);
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Add a remote host (`label`, `user@host`), persist it, and return the
    /// updated host list. The host starts `Reconnecting` until a health check.
    #[allow(clippy::too_many_arguments)]
    pub fn add_host(
        &mut self,
        label: &str,
        target: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        agent_commands: Vec<(String, String)>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .add(label, target, claude_bin, setup, env, agent_commands)
            .map_err(|e| CoreError::Host(e.to_string()))?
            .clone();
        let persisted = PersistedHost {
            id: host.id,
            label: host.label,
            target: host.target,
            is_local: host.is_local,
            claude_bin: host.claude_bin,
            setup: host.setup,
            env: host.env,
            agent_commands: host.agent_commands,
        };
        self.store
            .upsert_host(&persisted)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Update an existing host's launch configuration (`claude_bin` / `setup` /
    /// `env` / `agent_commands`), and optionally its `target`. Works for any host
    /// id INCLUDING the protected `localhost` (pass `None` for `target` to leave
    /// the target unchanged — localhost's stays `"this machine"`). Persists the
    /// change and returns the updated host list.
    #[allow(clippy::too_many_arguments)]
    pub fn update_host(
        &mut self,
        id: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        target: Option<String>,
        agent_commands: Vec<(String, String)>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .update(id, claude_bin, setup, env, target, agent_commands)
            .map_err(|e| CoreError::Host(e.to_string()))?
            .clone();
        let persisted = PersistedHost {
            id: host.id,
            label: host.label,
            target: host.target,
            is_local: host.is_local,
            claude_bin: host.claude_bin,
            setup: host.setup,
            env: host.env,
            agent_commands: host.agent_commands,
        };
        self.store
            .upsert_host(&persisted)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Remove a host by id, delete it from the store, and return the updated list.
    pub fn remove_host(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.hosts
            .remove(id)
            .map_err(|e| CoreError::Host(e.to_string()))?;
        self.store
            .delete_host(id)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Apply an externally-computed health result for a host (runtime status, not
    /// persisted). Returns the updated host list.
    /// The drivers Marsad probes on every host (mirrors the agent's probe set).
    /// A host is only `Connected` once ALL of these have reported their caps.
    const PROBED_DRIVERS: [&'static str; 2] = ["claude", "codex"];

    /// True once every probed driver's capability discovery has landed for `id`
    /// (agents + versioned models loaded) since the host last came online.
    pub(crate) fn host_caps_fully_loaded(&self, id: &str) -> bool {
        match self.caps_loaded.get(id) {
            Some(seen) => Self::PROBED_DRIVERS.iter().all(|d| seen.contains(*d)),
            None => false,
        }
    }

    pub fn set_host_status(&mut self, id: &str, status: HostStatus) -> Vec<VmUpdate> {
        // The health worker reports raw reachability (Connected / Unreachable).
        // We translate that into the truthful UI status: a reachable host stays
        // `Connecting` until its capability discovery has fully landed (agents +
        // versioned models), then flips to `Connected`. No optimism, no timeouts
        // surfaced — just "connecting while the background load happens".
        match status {
            HostStatus::Connected => {
                let fully = self.host_caps_fully_loaded(id);
                let effective = if fully {
                    HostStatus::Connected
                } else {
                    HostStatus::Connecting
                };
                self.hosts.set_status(id, effective);
                // Kick off a FRESH all-driver discovery exactly once per online
                // session, on the first Connected — even when `fully` is already
                // true from PERSISTED caps restored on restart. Those persisted
                // caps still display immediately (the host flips Connected with no
                // wait); this background probe freshens them so a CLI installed
                // since the last run is picked up (the WSL/SSH staleness bug). We
                // gate on `discovery_triggered` — NOT `caps_loaded`, which the
                // persisted replay also fills — so a restart genuinely re-probes.
                // `insert` returns true only the first time, so repeated health
                // ticks don't re-probe. Cleared on Unreachable so reconnect re-runs.
                if self.discovery_triggered.insert(id.to_string()) {
                    self.refresh_capabilities(Some(id.to_string()), None, None);
                }
            }
            HostStatus::Unreachable => {
                // Went offline → forget its discovery so a reconnect re-probes
                // cleanly (and the host correctly returns to Connecting first).
                self.caps_loaded.remove(id);
                self.discovery_triggered.remove(id);
                self.hosts.set_status(id, HostStatus::Unreachable);
            }
            other => {
                self.hosts.set_status(id, other);
            }
        }
        vec![VmUpdate::Hosts(self.hosts.list().to_vec())]
    }

    // -----------------------------------------------------------------------
    // Settings commands
    // -----------------------------------------------------------------------

    /// Default sensible settings returned when nothing has been persisted yet.
    fn default_settings() -> PersistedSettings {
        use marsad_store::NotifPref;
        use std::collections::HashMap;

        let mut notif_prefs: HashMap<String, NotifPref> = HashMap::new();
        for (ev, enabled, priority, sound, group, dnd) in [
            ("gate_needs_you", true, "urgent", true, false, false),
            ("gate_error", true, "urgent", true, false, false),
            ("session_done", true, "normal", false, true, true),
            ("session_started", false, "low", false, true, true),
            ("host_unreachable", true, "urgent", true, false, false),
        ] {
            notif_prefs.insert(
                ev.to_string(),
                NotifPref {
                    enabled,
                    priority: priority.to_string(),
                    sound,
                    group,
                    dnd,
                },
            );
        }
        PersistedSettings {
            notif_prefs,
            play_sound: true,
            group_alerts: true,
            do_not_disturb: false,
            background: "Gradient".to_string(),
            restore_on_launch: true,
            confirm_before_stop: true,
            default_model: "claude-sonnet-4-6".to_string(),
            default_driver: "claude".to_string(),
            default_models: Vec::new(),
            default_modes: Vec::new(),
        }
    }

    /// Return the stored settings, or sensible defaults if nothing has been persisted yet.
    pub fn get_settings(&self) -> PersistedSettings {
        self.store
            .restore()
            .ok()
            .and_then(|r| r.settings)
            .unwrap_or_else(Self::default_settings)
    }

    /// Persist the app settings blob.
    pub fn set_settings(&mut self, s: &PersistedSettings) -> Result<(), CoreError> {
        self.store
            .upsert_settings(s)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    // -----------------------------------------------------------------------
    // Changes commands
    // -----------------------------------------------------------------------

    /// Overwrite the working-tree change list for a session (e.g. after a
    /// commit that cleared some of the files) and return the usual pair of
    /// `[VmUpdate::Session, VmUpdate::Overview]`.
    ///
    /// Returns an empty vec (no-op) for an unknown session id so that callers
    /// never need to guard against the session having been removed concurrently.
    pub fn set_session_changes(&mut self, id: &str, changes: Vec<FileChange>) -> Vec<VmUpdate> {
        let Some(record) = self.sessions.get_mut(id) else {
            return vec![];
        };
        record.changes = changes;
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)]
    }
}
