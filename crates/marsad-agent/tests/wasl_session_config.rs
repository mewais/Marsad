//! wasl_session_config.rs — Wasl **Backend** PRODUCTION config-loading + factory
//! wiring bucket (the final piece: wiring the REAL `RealWaslSessionProvider`
//! into a real Marsad daemon's `serve()`).
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3) §6.1 (agent
//! holds the `WaslSessionProvider`, built from the factory on first launch), §7
//! ("Replacing `FakeWaslSessionClient` with `RealWaslSessionClient` changes
//! **only** the provider factory registered in the agent's `serve()`, behind a
//! `set_wasl_session_provider_factory` seam"), and §11 (local-first, no
//! cross-host). This file is the DIRECT analogue of the X1 config-loading +
//! default-factory tests in `tests/wasl.rs` (`load_wasl_config` + the
//! `set_wasl_connector_factory` smoke test) — but for the Wasl-**Backend**
//! (Wasl-managed session) side, whose real client is `RealWaslSessionProvider`.
//!
//! It runs ENTIRELY without sockets: it exercises the pure config-loading /
//! endpoint-selection functions and the always-compiled provider seam, exactly
//! as `tests/wasl.rs` exercises `load_wasl_config` without ever driving the
//! real-IO `serve()` loop (which is `#[cfg(not(coverage))]` — real sockets, loops
//! forever, unreachable from a unit test). The actual `TcpListener`/thread-spawn
//! glue in `serve()` is out of scope for this crate's own test regime, mirroring
//! how the X1 operator wiring in `serve.rs` is coverage-excluded; what is proven
//! here is every PURE seam `serve()` composes.
//!
//! ── NOTE(test-agent): DESIGN DECISION + INVENTED / ASSUMED SURFACE ────────────
//! `docs/design/wasl-backend-plan.md` §6/§7/§9 pin the BEHAVIOR (the agent builds
//! its `WaslSessionProvider` from a factory; the real client swaps in behind that
//! factory in `serve()` with zero IPC/core/gate change) but do NOT specify how a
//! Wasl-managed session's runtime-control endpoint is configured in production —
//! §7's swappability note is the only anchor. Following the X1 pattern as closely
//! as possible, this file pins the following PRODUCTION config shape (chosen for
//! the reasons documented in the task's design report, summarized here):
//!
//!   • REUSE the EXISTING `<data_root>/wasl.json` file, adding a NEW, independent
//!     top-level key `session_runtimes` — a list of
//!     `WaslSessionRuntimeEntry { host_id, base_url }`. Both `wasl.json` concepts
//!     answer the same operational question ("how does THIS Marsad daemon reach a
//!     Wasl node's HTTP API for this host?"): the X1 `operators[].url` reaches the
//!     node's **operator** API; the new `session_runtimes[].base_url` reaches the
//!     **runtime-control** API — related-but-distinct endpoints, very likely a
//!     different port/path on the SAME physical Wasl node. Keeping both in one
//!     file mirrors X1's config-location/validation discipline and gives one file
//!     to find + validate. The two keys are FULLY INDEPENDENT (both
//!     `#[serde(default)]`): an X1-only `wasl.json` (no `session_runtimes`) is
//!     inert for the backend, and a backend-only `wasl.json` (no `operators`) is
//!     inert for X1 — neither perturbs the other (load-bearing optionality, §2).
//!
//!   • `WaslSessionRuntimeEntry { host_id: String, base_url: String }` and
//!     `WaslSessionRuntimesConfig { session_runtimes: Vec<WaslSessionRuntimeEntry> }`
//!     live in `marsad-wasl-session` (the L1 crate), EXACTLY as
//!     `marsad_wasl::WaslOperatorEntry`/`WaslOperatorsConfig` live in the X1 L1
//!     crate `marsad-wasl`. `base_url` is the `http://ip:port` shape
//!     `marsad_wasl_session::WaslSessionClientConfig::base_url` requires (the same
//!     shape `RealWaslSessionProvider::new` consumes).
//!
//!   • `marsad_agent::load_wasl_session_config(data_root: &str)
//!         -> Option<marsad_wasl_session::WaslSessionRuntimesConfig>`
//!     — the DIRECT analogue of `load_wasl_config`, re-exported at the crate root
//!     like `load_wasl_config`/`wasl_event_lines`. Same error-handling
//!     conventions VERBATIM: absent file OR absent/empty `session_runtimes` key →
//!     `None` (the inert path); malformed JSON → `None` + one stderr line;
//!     duplicate `host_id` → first entry wins (+ stderr line); a `base_url` that
//!     isn't `http://ip:port` (one trailing slash tolerated + stripped) is dropped
//!     (+ stderr line), and if every entry drops out, `None` (same "nothing to
//!     wire" disposition as an empty list).
//!
//!   • `marsad_agent::select_wasl_session_runtime(
//!         cfg: &marsad_wasl_session::WaslSessionRuntimesConfig, host_id: &str)
//!         -> Option<marsad_wasl_session::WaslSessionRuntimeEntry>`
//!     — the pure endpoint-selection helper `serve()` uses to pick the
//!     runtime-control entry for the daemon's serving host (local-first). An
//!     unconfigured host_id → `None` (→ the factory is never set for it → a
//!     `LaunchWaslSession` for that host fails gracefully with the EXISTING typed
//!     `Protocol` error, never a panic, never a wrong/default endpoint). This is
//!     the pure core of "given N config entries, the right provider is available
//!     under the right host_id" — proven WITHOUT the real-IO `serve()` glue.
//!
//! If the code-agent lands differently-shaped seams (e.g. a per-host provider
//! map instead of the single zero-arg provider slot the shipped `Agent` has
//! today, or the config types in a different module), it should adjust these call
//! sites — the BEHAVIOR pinned here (reuse `wasl.json` with an independent
//! `session_runtimes` key; identical `load_wasl_config` error conventions;
//! host_id→endpoint selection; absent config == inert == graceful typed failure)
//! is the contract, not the exact names.
//!
//! This file WILL fail to compile/pass until the code-agent lands
//! `load_wasl_session_config`, `select_wasl_session_runtime`, the
//! `WaslSessionRuntimeEntry`/`WaslSessionRuntimesConfig` config types (in
//! `marsad-wasl-session`), and the production `serve()` wiring that composes
//! them behind `set_wasl_session_provider_factory`. That is the expected
//! tests-first compile-failure (RED) signal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use marsad_agent::{
    load_wasl_session_config, select_wasl_session_runtime, Agent, AgentConfig, ListenSpec,
};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl_session::{WaslSessionRuntimeEntry, WaslSessionRuntimesConfig};

// ---------------------------------------------------------------------------
// Shared temp-dir + agent helpers (mirror `tests/wasl.rs`'s idioms — each file
// under tests/ compiles as an independent crate, so the helpers are duplicated
// per house convention; no `tempfile` dev-dependency).
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
    fn str(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempdir() -> TempDir {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "marsad-agent-wasl-session-config-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    TempDir(path)
}

fn write_wasl_json(dir: &TempDir, body: &[u8]) {
    std::fs::write(dir.path().join("wasl.json"), body).unwrap();
}

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-session-config-agent".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn launch_wasl_line(session_id: &str, host_id: &str, key: serde_json::Value) -> String {
    encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: session_id.to_string(),
        host_id: host_id.to_string(),
        wasl_target_key: key,
        name: None,
    })
}

// ===========================================================================
// Part 1 — `load_wasl_session_config` (the `load_wasl_config` analogue, §4.1
// error-handling conventions applied VERBATIM to the new `session_runtimes`
// key)
// ===========================================================================

/// Absent `wasl.json` → `None` (the inert path). Mirrors
/// `load_wasl_config_absent_file_is_none`.
#[test]
fn load_wasl_session_config_absent_file_is_none() {
    let tmp = tempdir();
    assert_eq!(load_wasl_session_config(tmp.str()), None);
}

/// A `wasl.json` that exists but carries NO `session_runtimes` key (e.g. an
/// X1-only operator config) → `None`: the backend must be inert, not error, when
/// only the OTHER Wasl concept is configured. This is the independence guarantee
/// between the two `wasl.json` keys.
#[test]
fn load_wasl_session_config_operators_only_file_is_none() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787"}]}"#,
    );
    assert_eq!(
        load_wasl_session_config(tmp.str()),
        None,
        "a wasl.json with only the X1 `operators` key must leave the backend inert (None)"
    );
}

/// Malformed JSON → `None` (config problems must never take the agent down).
/// Mirrors `load_wasl_config_malformed_json_is_none`.
#[test]
fn load_wasl_session_config_malformed_json_is_none() {
    let tmp = tempdir();
    write_wasl_json(&tmp, b"{ not valid json");
    assert_eq!(load_wasl_session_config(tmp.str()), None);
}

/// An empty `session_runtimes` list → `None` (same "nothing to wire" disposition
/// as X1's empty `operators`). Mirrors `load_wasl_config_empty_operators_is_none`.
#[test]
fn load_wasl_session_config_empty_runtimes_is_none() {
    let tmp = tempdir();
    write_wasl_json(&tmp, br#"{"session_runtimes":[]}"#);
    assert_eq!(load_wasl_session_config(tmp.str()), None);
}

/// A valid single entry returns the exact typed config. `base_url` points at the
/// RUNTIME-CONTROL api (distinct port from the X1 operator api on the same node).
#[test]
fn load_wasl_session_config_valid_single_entry_returns_exact() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[{"host_id":"local","base_url":"http://127.0.0.1:8788"}]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("valid file must load");
    assert_eq!(
        cfg,
        WaslSessionRuntimesConfig {
            session_runtimes: vec![WaslSessionRuntimeEntry {
                host_id: "local".to_string(),
                base_url: "http://127.0.0.1:8788".to_string(),
            }],
        }
    );
}

/// Multiple entries preserve their file order. Mirrors
/// `load_wasl_config_multiple_entries_preserve_order`.
#[test]
fn load_wasl_session_config_multiple_entries_preserve_order() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[
            {"host_id":"local","base_url":"http://127.0.0.1:8788"},
            {"host_id":"hpc","base_url":"http://10.0.0.5:8788"}
        ]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("valid file must load");
    assert_eq!(
        cfg.session_runtimes,
        vec![
            WaslSessionRuntimeEntry {
                host_id: "local".to_string(),
                base_url: "http://127.0.0.1:8788".to_string(),
            },
            WaslSessionRuntimeEntry {
                host_id: "hpc".to_string(),
                base_url: "http://10.0.0.5:8788".to_string(),
            },
        ]
    );
}

/// Duplicate `host_id` → the FIRST entry wins (a Wasl node is per-host, so a
/// daemon reaches ONE runtime-control endpoint per host). Mirrors
/// `load_wasl_config_duplicate_host_id_first_wins`.
#[test]
fn load_wasl_session_config_duplicate_host_id_first_wins() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[
            {"host_id":"local","base_url":"http://127.0.0.1:8788"},
            {"host_id":"local","base_url":"http://127.0.0.1:9999"}
        ]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("valid file must load");
    assert_eq!(
        cfg.session_runtimes.len(),
        1,
        "duplicate host_id must collapse to a single entry"
    );
    assert_eq!(
        cfg.session_runtimes[0].base_url, "http://127.0.0.1:8788",
        "the FIRST entry for a duplicate host_id must win"
    );
}

/// A trailing-slash `base_url` is normalized (stripped), matching X1's
/// `load_wasl_config_trailing_slash_url_is_normalized`: the node-config
/// convention writes `http://ip:port/`, but `WaslSessionClientConfig::base_url`
/// (and the real client's request path building) wants the no-trailing-slash
/// `http://ip:port` shape.
#[test]
fn load_wasl_session_config_trailing_slash_base_url_is_normalized() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[{"host_id":"local","base_url":"http://127.0.0.1:8788/"}]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("valid file must load");
    assert_eq!(
        cfg.session_runtimes,
        vec![WaslSessionRuntimeEntry {
            host_id: "local".to_string(),
            base_url: "http://127.0.0.1:8788".to_string(),
        }],
        "a trailing slash must be stripped, not left to break the real client later"
    );
}

/// A malformed `base_url` entry is dropped; well-formed entries survive. Mirrors
/// `load_wasl_config_malformed_url_entry_is_dropped_valid_entries_survive`.
#[test]
fn load_wasl_session_config_malformed_base_url_entry_dropped_valid_survives() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[
            {"host_id":"bad-scheme","base_url":"https://127.0.0.1:8788"},
            {"host_id":"bad-host","base_url":"http://not-an-ip:8788"},
            {"host_id":"bad-shape","base_url":"not a url at all"},
            {"host_id":"local","base_url":"http://127.0.0.1:8788"}
        ]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("one valid entry must load");
    assert_eq!(
        cfg.session_runtimes,
        vec![WaslSessionRuntimeEntry {
            host_id: "local".to_string(),
            base_url: "http://127.0.0.1:8788".to_string(),
        }],
        "only the well-formed http://ip:port entry must survive"
    );
}

/// If EVERY entry has a malformed `base_url`, the result is `None` (same as an
/// empty list). Mirrors `load_wasl_config_all_entries_malformed_url_is_none`.
#[test]
fn load_wasl_session_config_all_entries_malformed_base_url_is_none() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[{"host_id":"local","base_url":"not-a-url"}]}"#,
    );
    assert_eq!(
        load_wasl_session_config(tmp.str()),
        None,
        "zero usable runtimes after base_url validation must be None, same as an empty list"
    );
}

/// The two `wasl.json` keys coexist independently: a file carrying BOTH
/// `operators` (X1) and `session_runtimes` (backend) loads the backend key
/// correctly, ignoring the operator key entirely. (`load_wasl_config`'s own
/// operator reading is proven separately in `tests/wasl.rs`.)
#[test]
fn load_wasl_session_config_coexists_with_operators_key() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{
            "operators":[{"host_id":"local","url":"http://127.0.0.1:8787"}],
            "session_runtimes":[{"host_id":"local","base_url":"http://127.0.0.1:8788"}]
        }"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("the session_runtimes key must load");
    assert_eq!(
        cfg.session_runtimes,
        vec![WaslSessionRuntimeEntry {
            host_id: "local".to_string(),
            base_url: "http://127.0.0.1:8788".to_string(),
        }],
        "the runtime-control endpoint must be read from `session_runtimes`, not the operator `url`"
    );
    assert_ne!(
        cfg.session_runtimes[0].base_url, "http://127.0.0.1:8787",
        "the backend must NOT pick up the X1 operator url as its runtime-control endpoint — \
         these are distinct endpoints (operator api vs runtime-control api) on the same node"
    );
}

/// An unknown extra field on an entry is tolerated (forward-compat, no
/// `deny_unknown_fields` — the X1/§4.4 convention).
#[test]
fn load_wasl_session_config_tolerates_unknown_entry_fields() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"session_runtimes":[
            {"host_id":"local","base_url":"http://127.0.0.1:8788","x_future":"ignored"}
        ]}"#,
    );
    let cfg = load_wasl_session_config(tmp.str()).expect("an unknown field must not fail parsing");
    assert_eq!(cfg.session_runtimes[0].host_id, "local");
    assert_eq!(cfg.session_runtimes[0].base_url, "http://127.0.0.1:8788");
}

// ===========================================================================
// Part 2 — `select_wasl_session_runtime` (host_id → runtime entry): the pure
// core of `serve()`'s "given N config entries, the right provider is available
// under the right host_id" behavior — no sockets, no threads.
// ===========================================================================

fn cfg_with(entries: &[(&str, &str)]) -> WaslSessionRuntimesConfig {
    WaslSessionRuntimesConfig {
        session_runtimes: entries
            .iter()
            .map(|(h, u)| WaslSessionRuntimeEntry {
                host_id: h.to_string(),
                base_url: u.to_string(),
            })
            .collect(),
    }
}

/// A configured host_id selects exactly its own runtime-control entry.
#[test]
fn select_wasl_session_runtime_returns_the_matching_host_entry() {
    let cfg = cfg_with(&[
        ("local", "http://127.0.0.1:8788"),
        ("hpc", "http://10.0.0.5:8788"),
    ]);
    assert_eq!(
        select_wasl_session_runtime(&cfg, "hpc"),
        Some(WaslSessionRuntimeEntry {
            host_id: "hpc".to_string(),
            base_url: "http://10.0.0.5:8788".to_string(),
        }),
        "each host_id must resolve to ITS OWN endpoint — no cross-routing between hosts"
    );
    assert_eq!(
        select_wasl_session_runtime(&cfg, "local"),
        Some(WaslSessionRuntimeEntry {
            host_id: "local".to_string(),
            base_url: "http://127.0.0.1:8788".to_string(),
        })
    );
}

/// An UNCONFIGURED host_id → `None` — never a default/wrong endpoint. This is the
/// safety guarantee behind "absent = inert": a host with no runtime-control
/// entry must NOT silently fall back to some other host's endpoint.
#[test]
fn select_wasl_session_runtime_unconfigured_host_is_none() {
    let cfg = cfg_with(&[("local", "http://127.0.0.1:8788")]);
    assert_eq!(
        select_wasl_session_runtime(&cfg, "hpc"),
        None,
        "an unconfigured host must yield None, never another host's endpoint"
    );
}

// ===========================================================================
// Part 3 — production factory swap-in behavior (proven WITHOUT the real-IO
// serve() loop). §7: "the real client swaps in behind the factory in serve()."
//
// The `default_wasl_session_provider_factory` (a `RealWaslSessionProvider` per
// entry) and the actual `TcpListener`/observe-thread glue live inside `serve()`,
// which is `#[cfg(not(coverage))]` (real sockets, loops forever) — exactly as the
// X1 default-connector factory + forwarder threads do in `serve.rs`. So, as
// `tests/wasl.rs`'s `set_wasl_connector_factory` smoke test does for X1, we prove
// the FACTORY SEAM here: a registered zero-arg factory is what makes a provider
// available to a `LaunchWaslSession` for a configured host, and NO factory (the
// absent-config path) leaves the launch failing gracefully with the existing
// typed error. The real provider's own HTTP behavior is proven by
// marsad-wasl-session's own `real_*.rs` suite; here we prove the WIRING seam.
// ===========================================================================

/// The wiring `serve()` performs, distilled to a pure/no-socket unit: for each
/// entry in the loaded config, a provider is registered such that a
/// `LaunchWaslSession` for that host succeeds. We stand in for
/// `default_wasl_session_provider_factory` with a test factory (the real one
/// builds a `RealWaslSessionProvider`, unreachable without a live node) — proving
/// the SEAM `serve()` composes (`select_wasl_session_runtime` +
/// `set_wasl_session_provider_factory`), not the real HTTP client.
#[test]
fn configured_runtime_makes_launch_succeed_via_the_factory_seam() {
    // The config a real daemon would load in serve().
    let cfg = cfg_with(&[("local", "http://127.0.0.1:8788")]);
    let entry = select_wasl_session_runtime(&cfg, "local")
        .expect("the serving host must resolve to its runtime-control entry");

    let mut agent = make_agent();

    // serve() would, for this entry, register the factory. In production the
    // factory body is `default_wasl_session_provider_factory(&entry)` (a real
    // RealWaslSessionProvider); here we prove the SEAM by registering a
    // fake-backed factory that captures the entry's base_url — the observable
    // proof that the RIGHT endpoint reached the factory.
    let seen_base_url = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = seen_base_url.clone();
    let base_url = entry.base_url.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        *sink.lock().unwrap() = base_url.clone();
        Box::new(marsad_wasl_session::FakeWaslSessionProvider::new())
            as Box<dyn marsad_wasl_session::WaslSessionProvider>
    }));

    // A LaunchWaslSession for the configured host must succeed (the factory is
    // engaged on first launch — the §6.1 lazy-build contract).
    agent
        .handle_ipc(&launch_wasl_line(
            "sess-configured",
            "local",
            serde_json::json!({"protocol": "claude", "ctx": "c"}),
        ))
        .expect("a launch for a configured runtime host must succeed");

    assert_eq!(
        *seen_base_url.lock().unwrap(),
        "http://127.0.0.1:8788",
        "serve()'s factory must be built over the runtime-control endpoint selected \
         for the serving host (proves the config base_url reaches the provider factory)"
    );
}

/// N config entries → N distinct runtime endpoints resolvable, each under its own
/// host_id (the pure form of "N providers get registered under the right
/// host_ids"). The single-slot provider seam is local-first (one serving host),
/// but the SELECTION that would drive a per-host wiring is proven exhaustive.
#[test]
fn every_configured_host_resolves_to_its_own_endpoint() {
    let cfg = cfg_with(&[
        ("local", "http://127.0.0.1:8788"),
        ("hpc", "http://10.0.0.5:8788"),
        ("cloud", "http://192.168.1.9:8788"),
    ]);
    for (host, url) in [
        ("local", "http://127.0.0.1:8788"),
        ("hpc", "http://10.0.0.5:8788"),
        ("cloud", "http://192.168.1.9:8788"),
    ] {
        let entry = select_wasl_session_runtime(&cfg, host)
            .unwrap_or_else(|| panic!("host {host} must resolve"));
        assert_eq!(entry.host_id, host);
        assert_eq!(
            entry.base_url, url,
            "each host must map to its OWN runtime-control endpoint"
        );
    }
    // A host absent from the N-entry config still resolves to None.
    assert_eq!(select_wasl_session_runtime(&cfg, "nope"), None);
}

// ===========================================================================
// Part 4 — absent `session_runtimes` → Phase-B adopt-or-spawn path: when no
// pre-configured runtime-control endpoint exists (no `wasl.json`, or only the X1
// `operators` key), a `LaunchWaslSession` for a LOCAL host triggers Phase B's
// `ensure_wasl_runtime_node` — Marsad ADOPTS-OR-SPAWNS a managed `wasl-mcpd`
// node via the Container seam instead of returning a typed "no provider"
// Protocol error. The spawn is visible in the FakeContainer call log.
//
// Because no real HTTP server answers on the spawned node's control_bind
// (`FakeContainer` does not start a real process), the launch STILL returns
// `Err` — but the failure mode is a CONNECTION/transport error at
// `get_or_start`, not the old "no wasl configured" Protocol error.
//
// What changed from Phase A:
//   BEFORE (old contract): absent config → inert → typed Protocol error, no spawn.
//   NOW   (Phase B):       absent config → adopt-or-spawn via Container → connection
//                          error (no live node). The load_wasl_session_config(…)==None
//                          sanity check is UNCHANGED (that helper's behavior is
//                          unchanged — absent/operators-only still loads as None;
//                          None now triggers spawn, not inertness).
//
// RED status: `ensure_wasl_runtime_node` is `todo!()`, so no spawn happens yet
// and the old Err path fires instead of the Container adopt-or-spawn. The
// Container call-log assertions FAIL because no spawn occurred. The coder makes
// them green by implementing the real adopt-or-spawn logic.
// ===========================================================================

/// No `wasl.json` at all → `load_wasl_session_config` is `None` → serve() never
/// pre-sets a session-provider factory. A `LaunchWaslSession` for a LOCAL host
/// in this state triggers Phase B: `ensure_wasl_runtime_node` is called and
/// attempts to adopt-or-spawn a managed `wasl-mcpd-runtime-local` node via the
/// Container seam. Since `FakeContainer` does not start a real process, the
/// launch fails gracefully (connection/transport error) — never a panic, never
/// a silent success. The Container call log must show the probe and spawn.
///
/// RED: `ensure_wasl_runtime_node` is `todo!()` → the Container calls never
/// happen and the spawn assertions fail. The coder wires the real logic.
#[test]
fn absent_config_local_launch_adopts_or_spawns_a_node_then_fails_without_a_live_node() {
    let tmp = tempdir();
    // Sanity: the config helper's behavior is unchanged — absent file → None.
    // (None now means "spawn a local node," not "inert.")
    assert_eq!(
        load_wasl_session_config(tmp.str()),
        None,
        "no wasl.json must load as None; what changed is that None now triggers spawn, not inertness"
    );

    // A fresh agent (no factory, no provider pre-set) with a FakeContainer that
    // does NOT start a real process, so the managed node has no live HTTP server.
    let mut agent = make_agent();
    let result = agent.handle_ipc(&launch_wasl_line(
        "sess-spawn-absent",
        "local",
        serde_json::json!({"ctx": "x"}),
    ));

    // The launch must fail gracefully (no panic), but the failure is now a
    // connection/transport error from get_or_start — NOT the old Protocol error.
    assert!(
        result.is_err(),
        "LaunchWaslSession must still fail (FakeContainer has no live HTTP server), \
         but must not panic"
    );

    // The Container call log must show the adopt-or-spawn attempt for the managed
    // runtime node — proving Phase B's path was entered.
    // RED: no spawn logic yet, so the log is empty and these assertions fail.
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-runtime-local")),
        "ensure_wasl_runtime_node must probe the Container for the managed runtime node; \
         calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-runtime-local")),
        "an absent managed runtime node must be SPAWNED via the Container seam; \
         calls: {calls:?}"
    );
}

/// The `session_runtimes`-absent (X1-operators-only) `wasl.json` variant of the
/// same Phase-B guarantee: the presence of the OTHER Wasl key must not
/// accidentally pre-wire the backend, and `load_wasl_session_config` stays None.
/// But a `LaunchWaslSession` for a LOCAL host STILL triggers
/// `ensure_wasl_runtime_node` (Phase B adopt-or-spawn), not the old inert path.
///
/// RED: `ensure_wasl_runtime_node` is `todo!()` → no Container calls → spawn
/// assertions fail.
#[test]
fn operators_only_config_local_launch_adopts_or_spawns_a_node_then_fails_without_a_live_node() {
    let tmp = tempdir();
    write_wasl_json(
        &tmp,
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787"}]}"#,
    );
    // Sanity: operators-only still loads as None — unchanged helper behavior.
    // (None now triggers spawn, not inertness.)
    assert_eq!(
        load_wasl_session_config(tmp.str()),
        None,
        "an operators-only wasl.json must load as None; None now triggers spawn, not inertness"
    );

    // Fresh agent — no pre-set factory, FakeContainer has no live HTTP server.
    let mut agent = make_agent();
    let result = agent.handle_ipc(&launch_wasl_line(
        "sess-x1only-spawn",
        "local",
        serde_json::json!({"ctx": "x"}),
    ));

    // Graceful failure — connection error, not panic.
    assert!(
        result.is_err(),
        "LaunchWaslSession must fail (no live node behind FakeContainer) but must not panic"
    );

    // Container log must show the adopt-or-spawn attempt.
    // RED: no spawn logic yet → log is empty → assertions fail.
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-runtime-local")),
        "ensure_wasl_runtime_node must probe the Container for the managed runtime node; \
         calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-runtime-local")),
        "an absent managed runtime node must be SPAWNED via the Container seam; \
         calls: {calls:?}"
    );
}
