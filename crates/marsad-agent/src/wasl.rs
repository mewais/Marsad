//! X2 — Wasl connector wiring: config loading, `WaslEvent` -> IPC-line mapping,
//! and the connector registry/factory test seams.
//!
//! Authoritative spec: `docs/specs/wasl-connector.md` §4.

use marsad_proto::{encode_ipc, IpcMsg};

use super::Agent;

// ---------------------------------------------------------------------------
// §4.1 — load_wasl_config
// ---------------------------------------------------------------------------

/// Validate (and normalize) a `WaslOperatorEntry.url` against the exact shape
/// `marsad_wasl::WaslClientConfig::base_url` requires: `http://ip:port`, no
/// path, no trailing slash (`marsad-wasl`'s `wire::parse_base_url` `.expect()`s
/// that shape — a value that fails to parse as `SocketAddr` PANICS the caller,
/// not returns an `Err`).
///
/// Found live (2026-07-03, x6_wasl_live): `docs/specs/node-config.md` pins the
/// trailing-slash form (`http://127.0.0.1:4242/`) as "the existing convention"
/// for Wasl peer URLs — exactly the value an operator configuring `wasl.json`
/// from a node's own `[operator] bind` would naturally copy in. Left
/// unvalidated, that convention-shaped value sails through `load_wasl_config`
/// untouched, into `WaslClientConfig.base_url`, and panics the first time it's
/// actually used. For the streaming connector (`WaslOperatorClient::run`,
/// started on its own background thread) that only silently kills the
/// connector — no more Wasl visibility for that host, no crash. For a human
/// decision (`Core::answer_wasl_gate` -> `handle_answer_wasl_gate` ->
/// `connector.decide()` -> `post_decision` -> this same parse) it is FAR
/// worse: `decide()` runs synchronously on the agent's single
/// command-processing thread, which `main.rs` drives directly on the process's
/// main thread — an unhandled panic there takes down the entire daemon on the
/// first Wasl gate a human ever answers, not just the one malformed host.
///
/// `marsad-wasl`'s own frozen suite never catches this: it always constructs
/// `base_url` from `MockServer`'s own precisely-formatted address, never a
/// hand-typed config value. This is exactly the class of defect `load_wasl_config`
/// already exists to firewall (see its own "config problems must never take
/// the agent down" contract for malformed JSON / duplicate host ids) — so a
/// bad url is normalized (strip one trailing slash, the documented-convention
/// case) or, if still not `http://ip:port` after that, dropped with a loud
/// stderr line, the same disposition as every other malformed-entry case
/// below.
fn normalize_operator_url(url: &str) -> Option<String> {
    let trimmed = url.strip_suffix('/').unwrap_or(url);
    let host_port = trimmed.strip_prefix("http://")?;
    host_port.parse::<std::net::SocketAddr>().ok()?;
    Some(trimmed.to_string())
}

/// Read `<data_root>/wasl.json`. Absent file -> `None` (the inert path).
/// Malformed JSON / empty `operators` -> `None` + one stderr line (config
/// problems must never take the agent down). Duplicate `host_id` -> first
/// entry wins + stderr line. A malformed `url` (anything that isn't
/// `http://ip:port`, allowing one trailing slash) is normalized or, failing
/// that, dropped + stderr line — see `normalize_operator_url`. If every entry
/// is filtered out this way, `None` (same "nothing to wire" disposition as an
/// empty `operators` list).
pub fn load_wasl_config(data_root: &str) -> Option<marsad_wasl::WaslOperatorsConfig> {
    let path = std::path::Path::new(data_root).join("wasl.json");
    let text = std::fs::read_to_string(&path).ok()?;

    let parsed: marsad_wasl::WaslOperatorsConfig = match serde_json::from_str(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "marsad-agent: malformed wasl config {}: {e}",
                path.display()
            );
            return None;
        }
    };

    if parsed.operators.is_empty() {
        eprintln!(
            "marsad-agent: wasl config {} has no operators",
            path.display()
        );
        return None;
    }

    let mut seen = std::collections::HashSet::new();
    let mut operators = Vec::with_capacity(parsed.operators.len());
    for mut entry in parsed.operators {
        if !seen.insert(entry.host_id.clone()) {
            eprintln!(
                "marsad-agent: duplicate wasl operator host_id {:?} in {} — first entry wins",
                entry.host_id,
                path.display()
            );
            continue;
        }
        match normalize_operator_url(&entry.url) {
            Some(normalized) => {
                entry.url = normalized;
                operators.push(entry);
            }
            None => {
                eprintln!(
                    "marsad-agent: malformed wasl operator url {:?} for host {:?} in {} \
                     (must be http://ip:port, e.g. \"http://127.0.0.1:4243\") — operator skipped",
                    entry.url,
                    entry.host_id,
                    path.display()
                );
            }
        }
    }

    if operators.is_empty() {
        eprintln!(
            "marsad-agent: wasl config {} has no usable operators after validation",
            path.display()
        );
        return None;
    }

    Some(marsad_wasl::WaslOperatorsConfig { operators })
}

// ---------------------------------------------------------------------------
// X6 Wasl **Backend** — load_wasl_session_config + select_wasl_session_runtime
// (docs/design/wasl-backend-plan.md §6.1/§7). The DIRECT analogue of
// `load_wasl_config`, applied VERBATIM to the new, independent `session_runtimes`
// key of the SAME `<data_root>/wasl.json` file: the X1 `operators[].url` reaches
// the node's operator API; `session_runtimes[].base_url` reaches its
// runtime-control API — related-but-distinct endpoints on the same node. The two
// keys are fully independent (both `#[serde(default)]`): an X1-only `wasl.json`
// is inert for the backend, and a backend-only `wasl.json` is inert for X1.
// ---------------------------------------------------------------------------

/// Read `<data_root>/wasl.json` and return its `session_runtimes` block. Same
/// error-handling conventions as `load_wasl_config`, VERBATIM: absent file ->
/// `None` (the inert path); malformed JSON -> `None` + one stderr line (config
/// problems must never take the agent down); an absent/empty `session_runtimes`
/// key -> `None` (same "nothing to wire" disposition as an empty list, and the
/// independence guarantee for an X1-operators-only file); duplicate `host_id` ->
/// first entry wins + stderr line (a Wasl node is per-host, so a daemon reaches
/// ONE runtime-control endpoint per host); a `base_url` that isn't
/// `http://ip:port` (one trailing slash tolerated + stripped, per
/// `normalize_operator_url`) is dropped + stderr line, not fatal; if every entry
/// drops out this way, `None`.
pub fn load_wasl_session_config(
    data_root: &str,
) -> Option<marsad_wasl_session::WaslSessionRuntimesConfig> {
    let path = std::path::Path::new(data_root).join("wasl.json");
    let text = std::fs::read_to_string(&path).ok()?;

    let parsed: marsad_wasl_session::WaslSessionRuntimesConfig = match serde_json::from_str(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "marsad-agent: malformed wasl config {}: {e}",
                path.display()
            );
            return None;
        }
    };

    if parsed.session_runtimes.is_empty() {
        // Absent/empty `session_runtimes` -> inert. An X1-operators-only
        // `wasl.json` lands here (its `session_runtimes` defaults to empty).
        return None;
    }

    let mut seen = std::collections::HashSet::new();
    let mut runtimes = Vec::with_capacity(parsed.session_runtimes.len());
    for mut entry in parsed.session_runtimes {
        if !seen.insert(entry.host_id.clone()) {
            eprintln!(
                "marsad-agent: duplicate wasl session_runtime host_id {:?} in {} — first entry wins",
                entry.host_id,
                path.display()
            );
            continue;
        }
        match normalize_operator_url(&entry.base_url) {
            Some(normalized) => {
                entry.base_url = normalized;
                runtimes.push(entry);
            }
            None => {
                eprintln!(
                    "marsad-agent: malformed wasl session_runtime base_url {:?} for host {:?} in {} \
                     (must be http://ip:port, e.g. \"http://127.0.0.1:8788\") — runtime skipped",
                    entry.base_url,
                    entry.host_id,
                    path.display()
                );
            }
        }
    }

    if runtimes.is_empty() {
        eprintln!(
            "marsad-agent: wasl config {} has no usable session_runtimes after validation",
            path.display()
        );
        return None;
    }

    Some(marsad_wasl_session::WaslSessionRuntimesConfig {
        session_runtimes: runtimes,
    })
}

/// Pure endpoint selector (unit-testable without sockets): pick the
/// runtime-control entry for `host_id` — the serving host, local-first (§11).
/// An unconfigured `host_id` -> `None` (never a default/wrong endpoint): the
/// factory is never set for it, so a `LaunchWaslSession` for that host fails
/// gracefully with the existing typed `Protocol` error, never a panic and never
/// a silent wrong endpoint.
pub fn select_wasl_session_runtime(
    cfg: &marsad_wasl_session::WaslSessionRuntimesConfig,
    host_id: &str,
) -> Option<marsad_wasl_session::WaslSessionRuntimeEntry> {
    cfg.session_runtimes
        .iter()
        .find(|entry| entry.host_id == host_id)
        .cloned()
}

// ---------------------------------------------------------------------------
// B1 scaffold — WaslNode managed-runtime config + port helpers
// (docs/design/wasl-backend-plan.md §node-spawn)
// ---------------------------------------------------------------------------

/// Configuration for rendering a `wasl-node` TOML config file. Pure and
/// always compiled (no IO) so the node-spawn path can be tested without real
/// sockets or OS processes.
pub struct WaslNodeConfig<'a> {
    /// Unique per-host agent identity (`marsad-<host_id>`).  Placed under
    /// `[identity]` so the node and its peers can distinguish Marsad-spawned
    /// nodes by agent_id.
    pub agent_id: &'a str,
    /// Host identity — the `host_id` string this daemon is serving.
    pub host_id: &'a str,
    /// Bare socket address (`ip:port`) for the node's A2A listener (`[a2a]`).
    /// Deterministic per host; bare because the node parses with `SocketAddr::parse`.
    pub a2a_bind: &'a str,
    /// Bare socket address (`ip:port`) for the node's operator listener (`[operator]`).
    /// Deterministic per host; bare because the node parses with `SocketAddr::parse`.
    pub operator_bind: &'a str,
    /// The `control_bind` address for the node's runtime-control HTTP endpoint.
    /// May be passed WITH an `http://` scheme (that's what `wasl_control_bind`
    /// returns, since the client/factory dials the node); the rendered TOML
    /// strips the scheme to the bare `ip:port` that wasl-mcpd's `SocketAddr`
    /// parser requires. Deterministic per host so a durable node is
    /// rediscoverable after a restart.
    pub control_bind: &'a str,
    /// The default A2A protocol the node advertises (e.g. `"claude"`).
    pub default_protocol: &'a str,
    /// Absolute path to the `claude` binary, if known. `None` → the node
    /// picks the binary from its own `PATH`.
    pub claude_bin: Option<&'a str>,
    /// Absolute path to the `opencode` binary, if known.
    pub opencode_bin: Option<&'a str>,
    /// Absolute path to the `codex` binary, if known.
    pub codex_bin: Option<&'a str>,
}

/// Normalise any bind value to a bare socket address by stripping an optional
/// `http://` scheme prefix and trailing `/`. The node parses all bind addresses
/// with `SocketAddr::parse`; an `http://…` value would silently fail that parse.
fn bare_socket(s: &str) -> &str {
    let s = s.strip_prefix("http://").unwrap_or(s);
    s.strip_suffix('/').unwrap_or(s)
}

/// Render a `wasl-node` TOML config from `cfg`. Pure: no filesystem IO, always
/// compiled. Generates the `[identity]`, `[a2a]`, `[operator]`, and `[runtime]`
/// blocks that `wasl-mcpd --config <path>` reads.
///
/// TOML shape (Phase-C, full config):
///
/// ```toml
/// [identity]
/// agent_id = "<agent_id>"
/// host_id = "<host_id>"
///
/// [a2a]
/// bind = "<ip:port>"    # bare socket addr, deterministic per host
///
/// [operator]
/// bind = "<ip:port>"    # bare socket addr, deterministic per host
///
/// [runtime]
/// kind = "managed"
/// control_bind = "<ip:port>"   # scheme-stripped from cfg.control_bind
/// default_protocol = "<default_protocol>"
/// claude_bin = "<path>"      # only when Some
/// opencode_bin = "<path>"    # only when Some
/// ```
pub fn render_wasl_node_toml(cfg: &WaslNodeConfig) -> String {
    let mut out = String::new();
    // [identity] block — agent + host identity.
    out.push_str("[identity]\n");
    out.push_str(&format!("agent_id = {:?}\n", cfg.agent_id));
    out.push_str(&format!("host_id = {:?}\n", cfg.host_id));
    out.push('\n');
    // [a2a] block — the A2A listener address (bare socket addr).
    out.push_str("[a2a]\n");
    out.push_str(&format!("bind = {:?}\n", bare_socket(cfg.a2a_bind)));
    out.push('\n');
    // [operator] block — the operator listener address (bare socket addr).
    out.push_str("[operator]\n");
    out.push_str(&format!("bind = {:?}\n", bare_socket(cfg.operator_bind)));
    out.push('\n');
    // [runtime] block — the managed-runtime kind + control endpoint + protocol defaults.
    out.push_str("[runtime]\n");
    out.push_str("kind = \"managed\"\n");
    // `control_bind` in the node's config is a BARE socket address: wasl-mcpd
    // parses it with `SocketAddr::parse` (an `http://…` value fails to parse and
    // the runtime-control API silently never starts). `wasl_control_bind` carries
    // an `http://` scheme for the CLIENT/factory side (it dials the node), so
    // strip the scheme (and any trailing slash) here for the node's own bind.
    out.push_str(&format!("control_bind = {:?}\n", bare_socket(cfg.control_bind)));
    out.push_str(&format!("default_protocol = {:?}\n", cfg.default_protocol));
    if let Some(bin) = cfg.claude_bin {
        out.push_str(&format!("claude_bin = {:?}\n", bin));
    }
    if let Some(bin) = cfg.opencode_bin {
        out.push_str(&format!("opencode_bin = {:?}\n", bin));
    }
    if let Some(bin) = cfg.codex_bin {
        out.push_str(&format!("codex_bin = {:?}\n", bin));
    }
    out
}

/// Compute a deterministic loopback `control_bind` address for `host_id`
/// (`http://127.0.0.1:<port>`). Deterministic so a durable wasl-node on a
/// host is always reachable at the same address after a process restart (the
/// Marsad agent can re-register the factory without re-spawning the node).
///
/// Port derivation: FNV-1a hash of `host_id` mapped into the range
/// `[47000, 47999]` (1000-port window, far from well-known services,
/// distinct per host with high probability).
pub fn wasl_control_bind(host_id: &str) -> String {
    // FNV-1a 32-bit hash — simple, stable, dependency-free.
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in host_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Map into [47000, 47999]: 1000-port window.
    let port = 47_000u32 + (hash % 1_000);
    format!("http://127.0.0.1:{port}")
}

/// Compute a deterministic bare A2A bind address for `host_id`
/// (`127.0.0.1:<port>`), using the same FNV-1a technique as
/// [`wasl_control_bind`] but in the port window `[48000, 48999]`.
///
/// Returns a **bare** `ip:port` (no `http://` scheme) because the node
/// parses `[a2a] bind` with `SocketAddr::parse`, not as a URL.
pub fn wasl_a2a_bind(host_id: &str) -> String {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in host_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Map into [48000, 48999]: 1000-port window, distinct from control (47xxx).
    let port = 48_000u32 + (hash % 1_000);
    format!("127.0.0.1:{port}")
}

/// Compute a deterministic bare operator bind address for `host_id`
/// (`127.0.0.1:<port>`), using the same FNV-1a technique as
/// [`wasl_control_bind`] but in the port window `[49000, 49999]`.
///
/// Returns a **bare** `ip:port` (no `http://` scheme) because the node
/// parses `[operator] bind` with `SocketAddr::parse`, not as a URL.
pub fn wasl_operator_bind(host_id: &str) -> String {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in host_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Map into [49000, 49999]: 1000-port window, distinct from control (47xxx)
    // and A2A (48xxx).
    let port = 49_000u32 + (hash % 1_000);
    format!("127.0.0.1:{port}")
}

// ---------------------------------------------------------------------------
// Remote-host Wasl node: the three loopback bind KINDS a node exposes, and the
// LOCAL tunnel-endpoint allocator for reaching a REMOTE node over `ssh -L`.
//
// For a LOCAL node the daemon dials the node's own loopback bind directly
// (`wasl_control_bind`/`wasl_operator_bind`/`wasl_a2a_bind`). For a REMOTE node
// those SAME binds live on the REMOTE loopback; the daemon reaches them through
// per-host `ssh -L <lport>:127.0.0.1:<remote_port>` tunnels and must dial the
// LOCAL tunnel endpoint (`http://127.0.0.1:<lport>`), never the remote bind.
//
// The local tunnel ports live in a DISTINCT window from the node's own bind
// windows so a remote node's tunnel endpoint can never collide with a
// simultaneously-running LOCAL node's real bind on this machine.
// ---------------------------------------------------------------------------

/// Which of a node's three loopback endpoints a bind/tunnel refers to. Pure
/// value; drives the deterministic local-tunnel-port allocation so the three
/// tunnels for one host never collide with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaslBindKind {
    Control,
    Operator,
    A2a,
}

impl WaslBindKind {
    /// A small per-kind salt folded into the FNV hash so the three tunnels for
    /// one host land on three distinct ports.
    fn salt(self) -> u32 {
        match self {
            WaslBindKind::Control => 0,
            WaslBindKind::Operator => 1,
            WaslBindKind::A2a => 2,
        }
    }
}

/// The bare `ip:port` a REMOTE node binds for a given endpoint kind. Identical
/// to the local bind helpers' bare form (`wasl-mcpd` on the remote parses these
/// with `SocketAddr::parse`), so a durable remote node is rediscoverable at the
/// same address after a restart. Returned bare (no `http://`) — that is what the
/// remote node's TOML `bind`/`control_bind` keys require.
pub fn wasl_remote_bind(host_id: &str, kind: WaslBindKind) -> String {
    match kind {
        // control_bind is the only one whose local helper carries an http://
        // scheme; strip it to the bare form the node's own TOML needs.
        WaslBindKind::Control => bare_socket(&wasl_control_bind(host_id)).to_string(),
        WaslBindKind::Operator => wasl_operator_bind(host_id),
        WaslBindKind::A2a => wasl_a2a_bind(host_id),
    }
}

/// Compute the deterministic LOCAL loopback tunnel endpoint (`127.0.0.1:<lport>`)
/// the daemon dials to reach a REMOTE node's endpoint of `kind` for `host_id`.
///
/// Port window `[45000, 46999]` — a 2000-port range BELOW the node's own bind
/// windows (control 47xxx, a2a 48xxx, operator 49xxx) so a remote node's tunnel
/// endpoint on this machine can never collide with a LOCAL node's real bind.
/// Deterministic per `(host_id, kind)` so the tunnel is re-openable at the same
/// local port after a restart, and the per-`kind` salt keeps the three tunnels
/// for one host on three distinct ports.
///
/// Returned BARE (`127.0.0.1:<lport>`); the caller adds an `http://` scheme when
/// dialing the control/operator HTTP endpoints.
pub fn wasl_local_tunnel_bind(host_id: &str, kind: WaslBindKind) -> String {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in host_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Fold the kind salt in so the three tunnels for one host differ.
    hash ^= kind.salt();
    hash = hash.wrapping_mul(FNV_PRIME);
    // Map into [45000, 46999]: 2000-port window below the node bind windows.
    let port = 45_000u32 + (hash % 2_000);
    format!("127.0.0.1:{port}")
}

/// Bare port (`u16`) of a local tunnel endpoint — the number `ssh -L` needs as
/// the local side of the forward. Thin wrapper over [`wasl_local_tunnel_bind`].
pub fn wasl_local_tunnel_port(host_id: &str, kind: WaslBindKind) -> u16 {
    wasl_local_tunnel_bind(host_id, kind)
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("wasl_local_tunnel_bind always ends in :<port>")
}

/// Build the `ssh -L` tunnel argv that forwards a LOCAL loopback port to a
/// REMOTE node's loopback endpoint. Reuses [`crate::remote::exec_prefix_for`] so
/// the SAME connection options (BatchMode, ControlMaster, keepalive) as every
/// other remote command apply — no parallel SSH layer.
///
/// `-N` (no remote command) + a foreground child we can `kill()` on teardown
/// (NOT `-f`, which would background the process out of our reach). The forward
/// is `<local_port>:127.0.0.1:<remote_port>`, binding the LOCAL side to loopback
/// only. `None` for a local/unknown host (no tunnel is needed for a local node).
pub fn wasl_tunnel_ssh_argv(
    host_target: &str,
    local_port: u16,
    remote_port: u16,
) -> Option<Vec<String>> {
    let mut argv = crate::remote::exec_prefix_for(Some(host_target))?;
    // `exec_prefix_for` puts the ssh DESTINATION last. The `-N`/`-L` forward
    // options MUST precede the destination — ssh treats anything after the
    // destination as the remote command, which would silently break the tunnel
    // (connect + run `-N -L …` as a command instead of forwarding). So pop the
    // destination, append the no-command port-forward flags, then re-append it.
    let dest = argv.pop()?;
    argv.push("-N".to_string());
    argv.push("-L".to_string());
    argv.push(format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"));
    argv.push(dest);
    Some(argv)
}

/// Build the shell script that durably launches a REMOTE `wasl-mcpd` managed
/// node, holding its stdin open so the stdio-MCP control task does not die on
/// EOF (the crux recipe). Run via `run_remote_script` (`bash -l -s`, login PATH).
///
/// Steps:
///   1. Write the node's `WASL_CONFIG` TOML to `~/.marsad/wasl-node-<host_id>.toml`.
///   2. If a `wasl-mcpd` is already durably running for this host (a live pid in
///      `~/.marsad/wasl-node-<host_id>.pid`), adopt it — do NOT spawn a second.
///   3. Otherwise verify `wasl-mcpd` is on the login PATH; if absent, print a
///      clear `MARSAD_WASL_NODE:NO_BINARY` sentinel and exit (the caller surfaces
///      an actionable error rather than a silent failure).
///   4. Durably spawn `wasl-mcpd` holding stdin open:
///        `nohup bash -lc "sleep 100000 | WASL_CONFIG=<toml> wasl-mcpd" >log 2>&1 &`
///      record its pid in `.pid`, and print `MARSAD_WASL_NODE:SPAWNED <pid>`.
///
/// `toml` is embedded via a single-quoted heredoc marker so `$`/`\` inside it are
/// not expanded. Deterministic + pure (no local IO): the ONLY side effects are on
/// the remote, performed by the returned script.
pub fn remote_wasl_node_launch_script(host_id: &str, toml: &str) -> String {
    // Home-relative remote paths; `$HOME` expands on the remote login shell.
    let cfg = format!("$HOME/.marsad/wasl-node-{host_id}.toml");
    let log = format!("$HOME/.marsad/wasl-node-{host_id}.log");
    let pid = format!("$HOME/.marsad/wasl-node-{host_id}.pid");
    format!(
        r#"set -e
mkdir -p "$HOME/.marsad"
cfg={cfg}
log={log}
pidf={pid}
cat > "$cfg" << 'MARSAD_WASL_NODE_TOML'
{toml}
MARSAD_WASL_NODE_TOML
# Adopt an already-running durable node for this host.
if [ -f "$pidf" ]; then
  old=$(cat "$pidf" 2>/dev/null || echo "")
  if [ -n "$old" ] && kill -0 "$old" 2>/dev/null; then
    echo "MARSAD_WASL_NODE:ADOPTED $old"
    exit 0
  fi
fi
# Require the node binary on the login PATH (installed like the agent CLIs).
if ! command -v wasl-mcpd >/dev/null 2>&1; then
  echo "MARSAD_WASL_NODE:NO_BINARY"
  exit 0
fi
# Durable spawn holding stdin open so the stdio-MCP control task survives EOF.
nohup bash -lc 'sleep 100000 | WASL_CONFIG="'"$cfg"'" wasl-mcpd' </dev/null >>"$log" 2>&1 &
np=$!
echo "$np" > "$pidf"
sleep 0.4
echo "MARSAD_WASL_NODE:SPAWNED $np"
"#,
        cfg = cfg,
        log = log,
        pid = pid,
        toml = toml,
    )
}

/// Build the shell script that tears down a durable REMOTE `wasl-mcpd` node for
/// `host_id`: SIGTERM (then SIGKILL) the pid in `~/.marsad/wasl-node-<host_id>.pid`
/// and remove the pid/config/log files. Idempotent — a missing pid/dir is a no-op.
pub fn remote_wasl_node_kill_script(host_id: &str) -> String {
    let cfg = format!("$HOME/.marsad/wasl-node-{host_id}.toml");
    let log = format!("$HOME/.marsad/wasl-node-{host_id}.log");
    let pid = format!("$HOME/.marsad/wasl-node-{host_id}.pid");
    format!(
        r#"pidf={pid}
if [ -f "$pidf" ]; then
  p=$(cat "$pidf" 2>/dev/null || echo "")
  [ -n "$p" ] && kill "$p" 2>/dev/null || true
  sleep 0.2
  [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true
fi
rm -f "$pidf" {cfg} {log}
echo "MARSAD_WASL_NODE:KILLED"
"#,
        pid = pid,
        cfg = cfg,
        log = log,
    )
}

/// Parse the `MARSAD_WASL_NODE:` sentinel out of the remote launch script's
/// stdout into a definitive outcome. Pure — unit-tested. Scans ALL lines so a
/// login banner / MOTD cannot mask the verdict; no sentinel line at all →
/// `Unreachable` (the command couldn't run / connection failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaslNodeSpawn {
    /// A fresh durable node was spawned (carries the remote pid).
    Spawned(String),
    /// An already-running durable node was adopted (carries the remote pid).
    Adopted(String),
    /// `wasl-mcpd` is not installed on the remote login PATH.
    NoBinary,
    /// No sentinel — the remote command didn't run / connection failed.
    Unreachable,
}

/// Parse remote launch-script stdout into a [`WaslNodeSpawn`] verdict.
pub fn parse_wasl_node_spawn(output: Option<&str>) -> WaslNodeSpawn {
    let out = match output {
        Some(o) => o,
        None => return WaslNodeSpawn::Unreachable,
    };
    for line in out.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("MARSAD_WASL_NODE:") {
            let mut parts = rest.trim().splitn(2, char::is_whitespace);
            let tag = parts.next().unwrap_or("");
            let arg = parts.next().unwrap_or("").trim().to_string();
            return match tag {
                "SPAWNED" => WaslNodeSpawn::Spawned(arg),
                "ADOPTED" => WaslNodeSpawn::Adopted(arg),
                "NO_BINARY" => WaslNodeSpawn::NoBinary,
                _ => WaslNodeSpawn::Unreachable,
            };
        }
    }
    WaslNodeSpawn::Unreachable
}

// ---------------------------------------------------------------------------
// Phase-D — Wasl worker MCP config builders
//
// When Marsad launches a Wasl-managed worker session (a claude or opencode
// process whose A2A peers are other Wasl nodes), the worker's inner agent
// binary needs an `--mcp-config` flag pointing at a TOML file that configures
// `wasl-mcpd` as an MCP server. These PURELY FUNCTIONAL builders (no IO,
// always compiled, unit-testable) produce that TOML and the compact JSON
// `SpawnContext.mcp_config` string that `handle_launch_wasl_session` injects
// into the target key's context before calling `get_or_start`.
// ---------------------------------------------------------------------------

/// A Wasl A2A peer that a worker session should be able to reach.
///
/// Built by `handle_launch_wasl_session` from the `context.peers` array in
/// the `WaslTargetKey`, then passed to `render_wasl_worker_toml` to produce
/// the per-session TOML config that `wasl-mcpd --config <path>` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaslWorkerPeer {
    pub agent_id: String,
    pub host_id: String,
    /// Absolute A2A URL (`http://127.0.0.1:<port>`).  Built from
    /// `wasl_a2a_bind(peer.host_id)` by the caller.
    pub url: String,
}

/// Render a **tools-client** `wasl-mcpd` TOML config for a Wasl worker.
///
/// The rendered file has only `[identity]` + zero or more `[[peers]]` blocks.
/// No `[runtime]` block is emitted: its absence tells `wasl-mcpd` this is an
/// ephemeral tools client, not a managed runtime node.
///
/// Keys are exactly `identity.agent_id`, `identity.host_id`,
/// `peers[].agent_id`, `peers[].host_id`, `peers[].url` (the `deny_unknown_fields`
/// TOML parse in `wasl-mcpd` rejects any other key). String values are rendered
/// with `{:?}` so Rust's `Debug` format quotes and escapes them correctly for
/// TOML.
///
/// The output is deterministic (peers in the given order) and pure (no IO).
pub fn render_wasl_worker_toml(agent_id: &str, host_id: &str, peers: &[WaslWorkerPeer]) -> String {
    let mut out = String::new();
    // [identity] block
    out.push_str("[identity]\n");
    out.push_str(&format!("agent_id = {:?}\n", agent_id));
    out.push_str(&format!("host_id = {:?}\n", host_id));
    // [[peers]] blocks — one per peer, in the given order.
    for peer in peers {
        out.push('\n');
        out.push_str("[[peers]]\n");
        out.push_str(&format!("agent_id = {:?}\n", peer.agent_id));
        out.push_str(&format!("host_id = {:?}\n", peer.host_id));
        out.push_str(&format!("url = {:?}\n", peer.url));
    }
    out
}

/// Build the compact JSON `mcp_config` string for a Wasl worker.
///
/// The returned string is the value to place in `SpawnContext.mcp_config` (a
/// `--mcp-config` argument applied verbatim to the worker's inner agent
/// binary). It configures `wasl-mcpd` as the `"wasl"` MCP server, pointing at
/// `config_path` (the TOML file written by `handle_launch_wasl_session`).
///
/// Built with `serde_json` so the path is correctly JSON-escaped even if it
/// contains special characters.
///
/// # Example output
/// ```text
/// {"mcpServers":{"wasl":{"command":"wasl-mcpd","args":["--config","/data/wasl-worker-sess.toml"]}}}
/// ```
pub fn wasl_worker_mcp_config(config_path: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            "wasl": {
                "command": "wasl-mcpd",
                "args": ["--config", config_path]
            }
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// §4.2 — wasl_event_lines (pure mapping)
// ---------------------------------------------------------------------------

/// Pure mapping (unit-testable without sockets): `WaslEvent` -> zero or more
/// `encode_ipc`'d broadcast lines.
///
/// - `Connected{..}`         -> `[]` (stderr log only)
/// - `GateRequested(g)`      -> `[WaslGateRequested{ host_id, ...g }]`
/// - `GateDecided(d)`        -> `[WaslGateDecided{ host_id, ...d }]`
/// - `Audit(e)`              -> `[WaslObservation{ host_id, entry: to_value(e) }]`
/// - `GateVanished{gate_id}` -> `[WaslGateDecided{ host_id, gate_id,
///   outcome:"unknown", decided_by:"resync", reason:None, decided_at:"" }]`
/// - `Disconnected{..}`      -> `[]` (stderr log only)
pub fn wasl_event_lines(host_id: &str, ev: &marsad_wasl::WaslEvent) -> Vec<String> {
    match ev {
        marsad_wasl::WaslEvent::Connected { hello } => {
            eprintln!(
                "marsad-agent: wasl connector for host {host_id} connected (agent_id={}, last_seq={})",
                hello.agent_id, hello.last_seq
            );
            vec![]
        }
        marsad_wasl::WaslEvent::Disconnected { error } => {
            eprintln!("marsad-agent: wasl connector for host {host_id} disconnected: {error}");
            vec![]
        }
        marsad_wasl::WaslEvent::GateRequested(g) => vec![encode_ipc(&IpcMsg::WaslGateRequested {
            host_id: host_id.to_string(),
            gate_id: g.gate_id.clone(),
            mode: g.mode.clone(),
            decidable: g.decidable,
            request: serde_json::to_value(&g.request).expect("GateRequest always serializes"),
            requested_at: g.requested_at.clone(),
            timeout_ms: g.timeout_ms,
            deadline: g.deadline.clone(),
        })],
        marsad_wasl::WaslEvent::GateDecided(d) => vec![encode_ipc(&IpcMsg::WaslGateDecided {
            host_id: host_id.to_string(),
            gate_id: d.gate_id.clone(),
            outcome: d.outcome.clone(),
            decided_by: d.decided_by.clone(),
            reason: d.reason.clone(),
            decided_at: d.decided_at.clone(),
        })],
        marsad_wasl::WaslEvent::Audit(entry) => vec![encode_ipc(&IpcMsg::WaslObservation {
            host_id: host_id.to_string(),
            entry: serde_json::to_value(entry).expect("AuditEntry always serializes"),
        })],
        marsad_wasl::WaslEvent::GateVanished { gate_id } => {
            vec![encode_ipc(&IpcMsg::WaslGateDecided {
                host_id: host_id.to_string(),
                gate_id: gate_id.clone(),
                outcome: "unknown".to_string(),
                decided_by: "resync".to_string(),
                reason: None,
                decided_at: String::new(),
            })]
        }
    }
}

// ---------------------------------------------------------------------------
// §4.2/§4.3 — connector registry + factory test seams
// ---------------------------------------------------------------------------

/// The `set_wasl_connector_factory` parameter type, pulled out to a named
/// alias so clippy's `type_complexity` lint doesn't fire on the signature
/// (structurally identical for callers — Rust doesn't care about alias names
/// at the call site).
pub type WaslConnectorFactoryFn =
    Box<dyn Fn(&marsad_wasl::WaslOperatorEntry) -> Box<dyn marsad_wasl::WaslConnector> + Send>;

impl Agent {
    /// Additive test seam (also the direct registration seam used by
    /// `serve()`'s real-IO wiring): register a connector for `host_id`,
    /// making it reachable from `handle_ipc`'s `AnswerWaslGate` arm.
    pub fn register_wasl_connector(
        &mut self,
        host_id: &str,
        connector: Box<dyn marsad_wasl::WaslConnector>,
    ) {
        // Boxed -> Arc at the registry boundary (review finding #1: see
        // `WaslConnectorMap`'s doc comment in `lib.rs`) — callers keep
        // passing an owned `Box` (unchanged call sites/signature); only the
        // registry's OWN storage needs to be shareable-without-the-map-lock.
        self.wasl_connectors
            .0
            .lock()
            .unwrap()
            .insert(host_id.to_string(), std::sync::Arc::from(connector));
    }

    /// Additive test seam; default = the real client factory (supplied
    /// lazily by `serve()`, since it is only ever needed from real-IO code).
    /// Setter so no constructor changes.
    pub fn set_wasl_connector_factory(&mut self, f: WaslConnectorFactoryFn) {
        self.wasl_connector_factory.0 = Some(f);
    }

    /// X4b (docs/specs/wasl-connector.md §X4b): adopt-or-spawn the HOST's
    /// receive-side `wasl-mcpd` — the node that binds `[a2a]` (the stable
    /// endpoint peers reach) + `[operator]` (so Marsad observes it, per
    /// X1/X2). Mirrors `apps/desktop`'s `adopt_or_spawn_agent` shape
    /// (try-connect-else-spawn), but reuses the SAME `Container` seam
    /// `Agent` already uses for claude sessions — no new fake/trait/
    /// constructor-arg. No-op (zero container calls) when `spec.wasl` is
    /// false: nothing on this host needs a `wasl-mcpd` for a launch that
    /// never requested one.
    ///
    /// "Marsad launches it, never speaks through it" is structural: the
    /// `Container` trait exposes only exists/attach/spawn/kill — no query/
    /// consult method exists to call, so no A2A traffic can originate from
    /// this fn (or anywhere else in the agent) via this seam.
    pub fn adopt_or_spawn_wasl_mcpd(&mut self, spec: &marsad_proto::LaunchSpec) -> bool {
        if !spec.wasl {
            return false;
        }
        let name = format!("wasl-mcpd-{}", spec.host_id);
        if self.active_container().exists(&name) {
            self.active_container().attach(&name).is_ok()
        } else {
            self.active_container().spawn(&name, "wasl-mcpd").is_ok()
        }
    }
}
