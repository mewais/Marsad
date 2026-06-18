# B07 — `marsad-hosts`

**Tier:** L1. **Mode:** sync API. **Coverage:** 100% line+region (via `SshRunner` fake).

## Purpose

Host registry and health: the local machine plus remote Linux hosts reachable over SSH. Parses the
user's existing `~/.ssh/config` (honor it; require no inbound ports), models host health, and
describes the small helper installed on first connect. SSH execution is behind a `SshRunner` trait
so the crate is fully testable with a fake.

## Public API (authoritative)

```rust
pub struct HostEntry { pub alias: String, pub hostname: Option<String>, pub user: Option<String>, pub identity_file: Option<String> }
pub fn parse_ssh_config(text: &str) -> Vec<HostEntry>;     // honor ~/.ssh/config Host/HostName/User/IdentityFile

pub enum HostStatus { Connected, Reconnecting, Unreachable }
pub struct Host {
    pub id: String,
    pub label: String,
    pub target: String,        // user@host  (or "this machine" for local)
    pub status: HostStatus,
    pub session_count: u32,
    pub is_local: bool,        // localhost — protected
    pub removable: bool,       // false for localhost
}

pub trait SshRunner { fn run(&self, target: &str, cmd: &str) -> Result<String, HostError>; }
pub struct FakeSshRunner { /* scripted responses */ }     // in-crate fake
impl FakeSshRunner { pub fn new() -> Self; pub fn respond(&mut self, target: &str, out: Result<String, HostError>); }

pub struct HostRegistry { /* ... */ }
impl HostRegistry {
    pub fn new(runner: Box<dyn SshRunner>) -> Self;       // seeds the protected localhost entry
    pub fn add(&mut self, label: &str, target: &str) -> Result<&Host, HostError>;
    pub fn remove(&mut self, id: &str) -> Result<(), HostError>;
    pub fn list(&self) -> &[Host];
    pub fn health_check(&mut self, id: &str) -> HostStatus; // uses SshRunner; maps failure -> Reconnecting/Unreachable
    pub fn helper_install_descriptor(&self) -> HelperDescriptor;  // what gets installed remotely; disclosable & removable
}

pub struct HelperDescriptor { pub path: String, pub purpose: String, pub removable: bool }
pub enum HostError { DuplicateLabel(String), CannotRemoveLocalhost, MalformedTarget(String), Unreachable(String), NotFound(String) }
```

Derive `Debug, Clone, PartialEq` (+ serde) on data types.

## Behavior notes

- `parse_ssh_config` handles multiple `Host` blocks, `HostName`, `User`, `IdentityFile`, comments,
  and wildcard host patterns (kept as-is). Honor the real format; `i06` parses a real config.
- Localhost is seeded by `new`, `is_local=true`, `removable=false`; `remove` on it →
  `CannotRemoveLocalhost`.
- `add` rejects a duplicate label and a malformed target (must look like `user@host` or a known ssh
  alias). New hosts start `Reconnecting` until a successful `health_check`.
- `health_check` runs a cheap remote probe via `SshRunner`; success → `Connected`. The failure split
  is keyed on the `HostError` the runner returns: `HostError::Unreachable(_)` → **hard** →
  `Unreachable`; any other `HostError` (or a non-empty/garbled but non-fatal result) → **transient**
  → `Reconnecting`. (This keeps "host unreachable" distinct from session state, per README
  resilience.)
- `health_check` on an **unknown id**: since the return type is `HostStatus` (not `Result`), an
  unknown id yields `HostStatus::Unreachable` (there is nothing to reach). Callers that need to
  distinguish "no such host" use `list()`/lookup first; `add`/`remove` are the `Result`-returning
  ops that surface `NotFound`.

## Test buckets (`tests/`)

- `happy.rs` — parse a multi-host config; add/list/remove a remote; health_check → Connected.
- `errors.rs` — `DuplicateLabel`, `CannotRemoveLocalhost`, `MalformedTarget`, `NotFound`,
  `Unreachable`.
- `branches.rs` — config with comments/wildcards/missing fields; status transitions; localhost
  protection flags; helper descriptor.
- `fakes.rs` — `SshRunner` contract via `FakeSshRunner` (scripted ok + error).

## Edit scope

Code-agent: write `crates/marsad-hosts/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
