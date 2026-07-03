//! `WaslError` — `docs/specs/wasl-connector.md` §2.7.

/// Errors surfaced by the HTTP/SSE client and decision POST.
#[derive(Debug, thiserror::Error)]
pub enum WaslError {
    /// Connect failures and mid-stream EOF/IO errors.
    #[error("io error: {0}")]
    Io(String),
    /// A non-ack HTTP status (Marsad only ever sends schema-correct decision
    /// bodies, so a 4xx/5xx here is a bug, not a race).
    #[error("http error: status={status} body={body}")]
    Http { status: u16, body: String },
    /// Bad hello (not the first frame, or unparsable) / `api_version`
    /// mismatch / unparsable known frame.
    #[error("protocol error: {0}")]
    Protocol(String),
}
