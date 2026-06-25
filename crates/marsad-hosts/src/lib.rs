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
    /// First-time connection attempt (never connected yet).
    Connecting,
    /// Re-establishing after having been connected.
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
    /// The `claude` executable to run on this host (a wrapper like `genie` or an
    /// absolute path). `None` → the default `"claude"`. Applies to local, WSL, and
    /// SSH/plink hosts alike. `#[serde(default)]` keeps older serialized hosts working.
    #[serde(default)]
    pub claude_bin: Option<String>,
    /// A shell command run BEFORE claude on this host (e.g. `module load cuda`).
    /// `None` → no setup. `#[serde(default)]` keeps older serialized hosts working.
    #[serde(default)]
    pub setup: Option<String>,
    /// Environment variables exported before claude on this host, as `(key, value)`
    /// pairs. `#[serde(default)]` keeps older serialized hosts working.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl Host {
    /// `true` when this host runs sessions inside a WSL (Windows Subsystem for
    /// Linux) distro, i.e. its [`Host::target`] is of the form `wsl:<distro>`.
    ///
    /// The distro encoding lives entirely in the target string, so this is a
    /// small derived helper rather than a stored field.
    pub fn is_wsl(&self) -> bool {
        wsl_distro(&self.target).is_some()
    }

    /// Returns the WSL distro name for a `wsl:<distro>` host, or `None` for any
    /// other target (`user@host`, a bare alias, or `this machine`).
    pub fn wsl_distro(&self) -> Option<&str> {
        wsl_distro(&self.target)
    }
}

/// Extract the distro name from a `wsl:<distro>` target string.
///
/// Returns `None` when `target` does not start with the `wsl:` prefix or when
/// the distro part is empty (`wsl:`).
fn wsl_distro(target: &str) -> Option<&str> {
    target
        .strip_prefix("wsl:")
        .filter(|distro| !distro.is_empty())
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
// WslRunner trait — distro discovery seam
// ---------------------------------------------------------------------------

/// Abstraction over listing the WSL distros installed on a Windows machine.
///
/// Mirrors [`SshRunner`]: the real implementation shells out to `wsl.exe`
/// (Windows only), while tests inject a [`FakeWslRunner`] so the discovery
/// logic stays fully covered without a live WSL host.
pub trait WslRunner {
    /// Return the raw stdout of `wsl.exe -l -q` (one distro name per line).
    fn list(&self) -> Result<String, HostError>;
}

/// Production [`WslRunner`] backed by the `wsl.exe` CLI.
///
/// WSL is a Windows-only concept, so the real implementation is
/// `#[cfg(windows)]`. Under coverage (and on non-Windows targets) it is a stub
/// that reports no distros, keeping the crate buildable and 100%-covered on
/// Linux CI; the live behaviour is verified on Windows.
pub struct RealWslRunner;

impl WslRunner for RealWslRunner {
    #[cfg(all(windows, not(coverage)))]
    fn list(&self) -> Result<String, HostError> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        // CREATE_NO_WINDOW: never flash a console window when probing distros.
        let output = Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| HostError::Unreachable(format!("wsl.exe: {e}")))?;
        if !output.status.success() {
            return Err(HostError::Unreachable(format!(
                "wsl.exe -l -q exited: {}",
                output.status
            )));
        }
        // `wsl.exe` emits UTF-16LE; decode lossily into UTF-8.
        let wide: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&wide))
    }

    #[cfg(not(all(windows, not(coverage))))]
    fn list(&self) -> Result<String, HostError> {
        // Non-Windows / coverage builds: no WSL host is available.
        Ok(String::new())
    }
}

// ---------------------------------------------------------------------------
// FakeWslRunner — in-crate scripted fake for tests
// ---------------------------------------------------------------------------

/// A scripted fake [`WslRunner`] for unit tests.
///
/// Call [`FakeWslRunner::respond`] to set the result returned by the next
/// [`WslRunner::list`] call; responses are consumed in FIFO order.
pub struct FakeWslRunner {
    responses: RefCell<VecDeque<Result<String, HostError>>>,
}

#[allow(clippy::new_without_default)]
impl FakeWslRunner {
    /// Create a new, empty [`FakeWslRunner`].
    pub fn new() -> Self {
        Self {
            responses: RefCell::new(VecDeque::new()),
        }
    }

    /// Enqueue a scripted response for the next [`WslRunner::list`] call.
    pub fn respond(&mut self, out: Result<String, HostError>) {
        self.responses.borrow_mut().push_back(out);
    }
}

impl WslRunner for FakeWslRunner {
    /// Return the next scripted result.
    ///
    /// Panics if no response was scripted — callers must always call `respond`
    /// before `list` in tests.
    fn list(&self) -> Result<String, HostError> {
        self.responses
            .borrow_mut()
            .pop_front()
            .expect("FakeWslRunner: no scripted response")
    }
}

/// List the WSL distros installed on this machine as host targets.
///
/// Runs the injected [`WslRunner`] (which wraps `wsl.exe -l -q` in production),
/// then parses each non-empty line into a `wsl:<distro>` target string ready to
/// pass to [`HostRegistry::add`]. Blank lines and surrounding whitespace
/// (including the stray `\r` left by `wsl.exe`'s CRLF output) are stripped.
///
/// # Errors
/// Propagates any [`HostError`] returned by the runner.
pub fn list_wsl_distros(runner: &dyn WslRunner) -> Result<Vec<String>, HostError> {
    let raw = runner.list()?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|distro| format!("wsl:{distro}"))
        .collect())
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
            claude_bin: None,
            setup: None,
            env: Vec::new(),
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
    pub fn add(
        &mut self,
        label: &str,
        target: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
    ) -> Result<&Host, HostError> {
        // A valid target is one of:
        //   - `wsl:<distro>`  — a WSL distro (distro must be non-empty),
        //   - `user@host`     — an SSH destination,
        //   - a bare host / `~/.ssh/config` alias (e.g. `etx`), resolved by `ssh`.
        // Reject empty, whitespace-containing, half-formed `user@`/`@host`, and
        // the distro-less `wsl:` prefix.
        let t = target.trim();
        let half_at = target
            .split_once('@')
            .is_some_and(|(u, h)| u.is_empty() || h.is_empty());
        // `wsl:` with no distro is malformed; `wsl:<distro>` is always accepted.
        let empty_wsl = t == "wsl:";
        if t.is_empty() || target.split_whitespace().count() != 1 || half_at || empty_wsl {
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
            status: HostStatus::Connecting,
            session_count: 0,
            is_local: false,
            removable: true,
            claude_bin,
            setup,
            env,
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
    #[allow(clippy::too_many_arguments)]
    pub fn load_remote(
        &mut self,
        id: &str,
        label: &str,
        target: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
    ) {
        if let Some(n) = id.strip_prefix("host-").and_then(|s| s.parse::<u64>().ok()) {
            if n >= self.next_id {
                self.next_id = n + 1;
            }
        }
        self.hosts.push(Host {
            id: id.to_string(),
            label: label.to_string(),
            target: target.to_string(),
            status: HostStatus::Connecting,
            session_count: 0,
            is_local: false,
            removable: true,
            claude_bin,
            setup,
            env,
        });
    }

    /// Update an existing host's launch configuration (`claude_bin` / `setup` /
    /// `env`), and optionally its `target`. Works for ANY host id, including the
    /// protected localhost (whose `target` stays `"this machine"` — pass `None`
    /// for `target` to leave it unchanged). Returns a mutable reference to the
    /// updated host.
    ///
    /// # Errors
    /// - [`HostError::NotFound`] — no host with that id.
    pub fn update(
        &mut self,
        id: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        target: Option<String>,
    ) -> Result<&Host, HostError> {
        let host = self
            .hosts
            .iter_mut()
            .find(|h| h.id == id)
            .ok_or_else(|| HostError::NotFound(id.to_string()))?;
        host.claude_bin = claude_bin;
        host.setup = setup;
        host.env = env;
        if let Some(t) = target {
            host.target = t;
        }
        Ok(host)
    }

    /// Set a host's status directly (used to apply an externally-computed health
    /// result). Unknown ids are a no-op.
    pub fn set_status(&mut self, id: &str, status: HostStatus) {
        if let Some(h) = self.hosts.iter_mut().find(|h| h.id == id) {
            // A host that has never connected stays in `Connecting`, not
            // `Reconnecting`, while its first attempt keeps failing transiently —
            // "Reconnecting" only makes sense once it has connected at least once.
            let keep_connecting =
                h.status == HostStatus::Connecting && status == HostStatus::Reconnecting;
            if !keep_connecting {
                h.status = status;
            }
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
