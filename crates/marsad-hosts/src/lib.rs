//! marsad-hosts — host registry, ~/.ssh/config parsing, and host health.
//!
//! See `AGENTS.md` and `docs/specs/hosts.md` for the full contract.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use thiserror::Error;

// ---------------------------------------------------------------------------
// HostEntry — parsed ~/.ssh/config entry
// ---------------------------------------------------------------------------

/// One `Host` block parsed from an SSH config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostEntry {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
}

/// Parse `~/.ssh/config` text into a list of [`HostEntry`] values.
///
/// Honors `Host`, `HostName`, `User`, and `IdentityFile` directives.
/// Comment lines (starting with `#`) and unrecognised keys are ignored.
/// Wildcard patterns (e.g. `*.internal`) are kept as-is.
pub fn parse_ssh_config(text: &str) -> Vec<HostEntry> {
    const KNOWN: &[&str] = &["host", "hostname", "user", "identityfile"];

    // Keep only non-comment, non-blank, keyword+value lines whose keyword is one
    // of the four recognised directives.  Chained filters guarantee that
    // `split_once` below always succeeds (no `?` branch needed).
    let meaningful: Vec<(String, String)> = text
        .lines()
        .map(str::trim)
        .filter(|t| {
            !t.is_empty() && !t.starts_with('#') && t.contains(|c: char| c.is_ascii_whitespace())
        })
        .map(|t| {
            // Safe: the filter above guarantees at least one ASCII whitespace.
            let (kw, val) = t.split_once(|c: char| c.is_ascii_whitespace()).unwrap();
            (kw.to_ascii_lowercase(), val.trim().to_string())
        })
        .filter(|(kw, _)| KNOWN.contains(&kw.as_str()))
        .collect();

    let mut entries: Vec<HostEntry> = Vec::new();
    let mut current: Option<HostEntry> = None;

    for (keyword, value) in meaningful {
        if keyword == "host" {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(HostEntry {
                alias: value,
                hostname: None,
                user: None,
                identity_file: None,
            });
        } else {
            // keyword is guaranteed to be "hostname", "user", or "identityfile"
            // (KNOWN filter above) and current is always Some when a sub-directive
            // follows a Host line in well-formed SSH configs.
            #[allow(clippy::option_map_unit_fn)]
            current.as_mut().map(|entry| match keyword.as_str() {
                "hostname" => entry.hostname = Some(value),
                "user" => entry.user = Some(value),
                _ => entry.identity_file = Some(value),
            });
        }
    }

    // Flush the last block.
    if let Some(entry) = current.take() {
        entries.push(entry);
    }

    entries
}

// ---------------------------------------------------------------------------
// HostStatus
// ---------------------------------------------------------------------------

/// The current health/connection state of a host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostStatus {
    Connected,
    Reconnecting,
    Unreachable,
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/// A managed host entry in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub label: String,
    /// `user@host` for remote hosts; `"this machine"` for localhost.
    pub target: String,
    pub status: HostStatus,
    pub session_count: u32,
    /// `true` only for the automatically seeded localhost entry.
    pub is_local: bool,
    /// `false` for the localhost entry; `true` for all remote hosts.
    pub removable: bool,
}

// ---------------------------------------------------------------------------
// HostError
// ---------------------------------------------------------------------------

/// All errors that can be returned by [`HostRegistry`] operations.
#[derive(Debug, Error)]
pub enum HostError {
    #[error("a host with label '{0}' already exists")]
    DuplicateLabel(String),
    #[error("the localhost entry cannot be removed")]
    CannotRemoveLocalhost,
    #[error("malformed target '{0}': expected user@host")]
    MalformedTarget(String),
    #[error("host '{0}' is unreachable")]
    Unreachable(String),
    #[error("no host found with id '{0}'")]
    NotFound(String),
}

// ---------------------------------------------------------------------------
// SshRunner trait
// ---------------------------------------------------------------------------

/// Abstraction over running a command on a remote host over SSH.
pub trait SshRunner {
    fn run(&self, target: &str, cmd: &str) -> Result<String, HostError>;
}

// ---------------------------------------------------------------------------
// FakeSshRunner — in-crate scripted fake for tests
// ---------------------------------------------------------------------------

/// A scripted fake [`SshRunner`] for unit tests.
///
/// Call [`FakeSshRunner::respond`] to enqueue results per target; they are
/// consumed in FIFO order when [`SshRunner::run`] is invoked.
pub struct FakeSshRunner {
    responses: RefCell<HashMap<String, VecDeque<Result<String, HostError>>>>,
}

#[allow(clippy::new_without_default)]
impl FakeSshRunner {
    /// Create a new, empty [`FakeSshRunner`].
    pub fn new() -> Self {
        Self {
            responses: RefCell::new(HashMap::new()),
        }
    }

    /// Enqueue a scripted response for `target`.
    pub fn respond(&mut self, target: &str, out: Result<String, HostError>) {
        self.responses
            .borrow_mut()
            .entry(target.to_string())
            .or_default()
            .push_back(out);
    }
}

impl SshRunner for FakeSshRunner {
    /// Return the next scripted result for `target`.
    ///
    /// Panics if no response was scripted for `target` — callers must always
    /// call `respond` before `run` in tests.
    fn run(&self, target: &str, _cmd: &str) -> Result<String, HostError> {
        self.responses
            .borrow_mut()
            .get_mut(target)
            .and_then(|q| q.pop_front())
            .expect("FakeSshRunner: no scripted response for target")
    }
}

// ---------------------------------------------------------------------------
// HelperDescriptor
// ---------------------------------------------------------------------------

/// Describes the small helper installed on a remote host on first connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperDescriptor {
    /// Absolute path where the helper will be installed on the remote host.
    pub path: String,
    /// Human-readable description of what the helper does.
    pub purpose: String,
    /// Whether the user can uninstall the helper (always `true`).
    pub removable: bool,
}

// ---------------------------------------------------------------------------
// HostRegistry
// ---------------------------------------------------------------------------

/// Registry of managed hosts.
pub struct HostRegistry {
    hosts: Vec<Host>,
    runner: Box<dyn SshRunner>,
    next_id: u64,
}

impl HostRegistry {
    /// Create a new registry; seeds the protected localhost entry automatically.
    pub fn new(runner: Box<dyn SshRunner>) -> Self {
        let localhost = Host {
            id: "localhost".to_string(),
            label: "localhost".to_string(),
            target: "this machine".to_string(),
            status: HostStatus::Connected,
            session_count: 0,
            is_local: true,
            removable: false,
        };
        Self {
            hosts: vec![localhost],
            runner,
            next_id: 1,
        }
    }

    /// Add a remote host.
    ///
    /// `target` must be `user@host` with both parts non-empty.
    /// The new host starts in [`HostStatus::Reconnecting`] until a successful
    /// [`HostRegistry::health_check`].
    ///
    /// # Errors
    /// - [`HostError::MalformedTarget`] — `target` is not valid `user@host`.
    /// - [`HostError::DuplicateLabel`] — a host with that label already exists.
    pub fn add(&mut self, label: &str, target: &str) -> Result<&Host, HostError> {
        // A valid SSH target is either `user@host` OR a bare host / `~/.ssh/config`
        // alias (e.g. `etx`), which `ssh <alias>` resolves. Reject only empty,
        // whitespace-containing, or half-formed `user@`/`@host` values.
        let t = target.trim();
        let half_at = target
            .split_once('@')
            .is_some_and(|(u, h)| u.is_empty() || h.is_empty());
        if t.is_empty() || target.split_whitespace().count() != 1 || half_at {
            return Err(HostError::MalformedTarget(target.to_string()));
        }

        // Reject duplicate labels.
        if self.hosts.iter().any(|h| h.label == label) {
            return Err(HostError::DuplicateLabel(label.to_string()));
        }

        let id = format!("host-{}", self.next_id);
        self.next_id += 1;

        self.hosts.push(Host {
            id,
            label: label.to_string(),
            target: target.to_string(),
            status: HostStatus::Reconnecting,
            session_count: 0,
            is_local: false,
            removable: true,
        });

        Ok(self.hosts.last().unwrap())
    }

    /// Remove a remote host by id.
    ///
    /// # Errors
    /// - [`HostError::NotFound`] — no host with that id.
    /// - [`HostError::CannotRemoveLocalhost`] — the id belongs to localhost.
    pub fn remove(&mut self, id: &str) -> Result<(), HostError> {
        let pos = self
            .hosts
            .iter()
            .position(|h| h.id == id)
            .ok_or_else(|| HostError::NotFound(id.to_string()))?;

        if self.hosts[pos].is_local {
            return Err(HostError::CannotRemoveLocalhost);
        }

        self.hosts.remove(pos);
        Ok(())
    }

    /// Return all registered hosts, including localhost.
    pub fn list(&self) -> &[Host] {
        &self.hosts
    }

    /// Re-insert a previously-persisted remote host (no `user@host` validation;
    /// the id is preserved). Starts `Reconnecting` until the next health check.
    /// Keeps `next_id` ahead of any `host-N` id so fresh `add`s never collide.
    pub fn load_remote(&mut self, id: &str, label: &str, target: &str) {
        if let Some(n) = id.strip_prefix("host-").and_then(|s| s.parse::<u64>().ok()) {
            if n >= self.next_id {
                self.next_id = n + 1;
            }
        }
        self.hosts.push(Host {
            id: id.to_string(),
            label: label.to_string(),
            target: target.to_string(),
            status: HostStatus::Reconnecting,
            session_count: 0,
            is_local: false,
            removable: true,
        });
    }

    /// Set a host's status directly (used to apply an externally-computed health
    /// result). Unknown ids are a no-op.
    pub fn set_status(&mut self, id: &str, status: HostStatus) {
        if let Some(h) = self.hosts.iter_mut().find(|h| h.id == id) {
            h.status = status;
        }
    }

    /// Run a health probe for the host with the given `id` and update its status.
    ///
    /// Returns the new [`HostStatus`] (never a `Result`):
    /// - Unknown id → [`HostStatus::Unreachable`].
    /// - Runner `Ok(_)` → [`HostStatus::Connected`].
    /// - Runner `Err(HostError::Unreachable(_))` → hard failure → [`HostStatus::Unreachable`].
    /// - Runner any other `Err` → transient failure → [`HostStatus::Reconnecting`].
    pub fn health_check(&mut self, id: &str) -> HostStatus {
        let pos = match self.hosts.iter().position(|h| h.id == id) {
            Some(p) => p,
            None => return HostStatus::Unreachable,
        };

        let target = self.hosts[pos].target.clone();
        let result = self.runner.run(&target, "echo ping");

        let new_status = match result {
            Ok(_) => HostStatus::Connected,
            Err(HostError::Unreachable(_)) => HostStatus::Unreachable,
            Err(_) => HostStatus::Reconnecting,
        };

        self.hosts[pos].status = new_status.clone();
        new_status
    }

    /// Describe the small helper binary that gets installed on remote hosts.
    pub fn helper_install_descriptor(&self) -> HelperDescriptor {
        HelperDescriptor {
            path: "~/.marsad/helper".to_string(),
            purpose: "Lightweight agent that relays Claude Code session I/O over SSH".to_string(),
            removable: true,
        }
    }
}
