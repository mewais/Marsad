//! `WaslSessionError` — the caller-side error type for the session-control
//! client (`wasl-backend-plan.md` §4.4), via `thiserror` in the house style
//! (mirrors `marsad_wasl::WaslError`).

use thiserror::Error;

/// Errors surfaced by a [`crate::WaslSessionClient`] / [`crate::WaslSessionProvider`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WaslSessionError {
    /// Transport / IO failure talking to the Wasl control surface.
    #[error("io error: {0}")]
    Io(String),
    /// A malformed or unexpected control-surface response / bad frame.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The target is not in a state that can service the request (offline,
    /// faulted, stopped).
    #[error("unavailable: {0}")]
    Unavailable(String),
}
