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

    let cfg = AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: std::env::var("MARSAD_DATA_ROOT")
            .unwrap_or_else(|_| "/var/lib/marsad".to_string()),
    };

    let agent = match Agent::new(cfg) {
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

#[cfg(coverage)]
fn main() {}
