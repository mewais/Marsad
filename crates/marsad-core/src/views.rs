//! Read-model projections — workstreams board, dependency graph, cost breakdown,
//! and the capabilities snapshot / refresh commands. Extracted from `lib.rs`
//! verbatim; `use super::*` brings crate types + root private plumbing.

use super::*;

impl Core {
    /// Recompute the workstreams board.
    pub fn board(&self) -> marsad_viewmodel::BoardView {
        marsad_viewmodel::board_view(
            &self.workstreams,
            &self.nodes(),
            &self.connections,
            self.hosts.list(),
            &[],
            self.active_profile.as_deref(),
        )
    }

    /// Recompute the Level-2 dependency graph for one workstream.
    pub fn graph(&self, ws_id: &str) -> Option<marsad_viewmodel::GraphView> {
        marsad_viewmodel::graph_view(ws_id, &self.workstreams, &self.nodes(), &self.connections)
    }

    /// Recompute the Usage tab's cost breakdown ("By host" / "By node") for
    /// one workstream's sessions, or `None` if `ws_id` does not match any
    /// workstream.
    pub fn cost_breakdown(&self, ws_id: &str) -> Option<marsad_viewmodel::WorkstreamCostBreakdown> {
        let sessions: Vec<marsad_viewmodel::SessionView> = self
            .session_order
            .iter()
            .filter_map(|id| {
                self.sessions
                    .get(id)
                    .map(|record| self.make_session_view(id, record))
            })
            .collect();
        marsad_viewmodel::workstream_cost_breakdown(
            ws_id,
            &self.workstreams,
            &self.nodes(),
            &sessions,
        )
    }

    // -----------------------------------------------------------------------
    // U1 dynamic capability discovery (docs/specs/dynamic-capabilities.md §3.5)
    // -----------------------------------------------------------------------

    /// Cloned snapshot of every (driver, host) capability entry seen so far
    /// (lock-only, no IO — mirrors `board()`).
    pub fn capabilities(&self) -> Vec<marsad_viewmodel::CapabilitiesView> {
        self.capabilities
            .values()
            .cloned()
            .map(marsad_viewmodel::CapabilitiesView::from)
            .collect()
    }

    /// Ask the agent to (re)probe capabilities: a single IPC line write, same
    /// cost class as [`Core::interrupt`] — never waits for a reply. Results
    /// arrive later as `CapabilitiesUpdate` broadcasts via `on_agent_line`.
    pub fn refresh_capabilities(
        &mut self,
        host_id: Option<String>,
        driver: Option<String>,
        project: Option<String>,
    ) {
        // Resolve the host's launch config from the registry so the agent's probe
        // runs setup-first over the RIGHT connection with the EXACT per-driver
        // command a launch would use (real discovery, not a local guess). A
        // missing host / the local host resolves to empty config + `None` target
        // (the agent then discovers locally, setup still honored). An unknown
        // host id (never registered) leaves everything defaulted — honest.
        let (host_target, setup, agent_commands, claude_bin) = host_id
            .as_deref()
            .and_then(|hid| self.hosts.list().iter().find(|h| h.id == hid))
            .map(|h| {
                (
                    // Only remote hosts carry a shell target; the local host runs
                    // discovery in-process (target `None`).
                    if h.is_local {
                        None
                    } else {
                        Some(h.target.clone())
                    },
                    h.setup.clone(),
                    h.agent_commands.clone(),
                    h.claude_bin.clone(),
                )
            })
            .unwrap_or((None, None, vec![], None));
        let _ = self.send_ipc(&IpcMsg::RefreshCapabilities {
            host_id,
            driver,
            project,
            host_target,
            setup,
            agent_commands,
            claude_bin,
        });
    }

    /// Connect-time discovery (deliverable #4a): kick off a full capabilities
    /// refresh (all drivers) for EVERY registered host, so capabilities are
    /// TRULY populated the moment Marsad comes up — not only when the frontend
    /// asks. Called once at startup after `load_hosts`. Each host is a single
    /// fire-and-forget `RefreshCapabilities` line (no probe IO on this path —
    /// the agent runs the probes on background threads, §4), so this is O(hosts)
    /// cheap sends with no blocking on the Core/agent lock. The agent's own
    /// bounded-probe deadline handles an unreachable host (reports
    /// `cli_available:false` + an error, never a hang).
    pub fn refresh_capabilities_all_hosts(&mut self) {
        let host_ids: Vec<String> = self.hosts.list().iter().map(|h| h.id.clone()).collect();
        for host_id in host_ids {
            self.refresh_capabilities(Some(host_id), None, None);
        }
    }

    /// A clone of the current host registry (read-only snapshot). Lets callers
    /// map a host `target` back to its `id` (e.g. the desktop's
    /// `connect_host_interactive` resolving which host to discover after an
    /// interactive login) without exposing the registry itself.
    pub fn hosts_snapshot(&self) -> Vec<marsad_hosts::Host> {
        self.hosts.list().to_vec()
    }
}
