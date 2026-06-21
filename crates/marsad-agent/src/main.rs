//! marsad-agent daemon entry point.
//!
//! The real `main` body performs blocking real-IO (socket/pipe binding and
//! accept loop) that is impossible to exercise in the unit-test suite.
//! Under `cfg(coverage)` (set automatically by cargo-llvm-cov) an empty stub
//! is compiled so the binary links but no uncoverable lines appear in the
//! measured surface.

#[cfg(not(coverage))]
fn main() {
    use marsad_agent::{Agent, AgentConfig, ListenSpec};

    let data_root = std::env::var("MARSAD_DATA_ROOT").unwrap_or_else(|_| {
        dirs_home()
            .map(|h| format!("{h}/.local/share/marsad"))
            .unwrap_or_else(|| "/var/lib/marsad".to_string())
    });

    // The socket path can be overridden via MARSAD_AGENT_ENDPOINT; the default
    // is <data_root>/agent.sock — the path the desktop GUI hard-codes.
    let socket_path = std::env::var("MARSAD_AGENT_ENDPOINT")
        .unwrap_or_else(|_| format!("{data_root}/agent.sock"));

    let cfg = AgentConfig {
        listen: ListenSpec::UnixSocket(socket_path),
        data_root,
    };

    let agent = match Agent::with_real_io(cfg) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("marsad-agent: failed to initialize: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = agent.serve() {
        eprintln!("marsad-agent: serve error: {e}");
        std::process::exit(1);
    }
}

/// Return the user's home directory as a `String`, if determinable.
#[cfg(not(coverage))]
fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok()
}

#[cfg(coverage)]
fn main() {}
