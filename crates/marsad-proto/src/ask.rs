//! `ask.user`: an agent asks the user a question through Marsad (the `ask_user` MCP tool that
//! Marsad injects for CLIs whose own question tool cannot reach it). The daemon of the
//! session's host raises a question gate on the session — it looks and is answered exactly
//! like a question the CLI asked itself — and replies with the answer.

use serde::{Deserialize, Serialize};
use wasit_core::QuestionPrompt;

/// `ask.user` (served by the daemon of the host the session runs on).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AskUserRequest {
    pub session: String,
    /// Chosen by the caller and reused when it retries after a lost connection, so a retry
    /// waits for the same gate instead of asking twice.
    pub request_id: String,
    pub question: QuestionPrompt,
}

/// The user's answer (`answers`: the chosen option labels, or the typed text), or why there
/// is none (`cancelled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AskUserReply {
    #[serde(default)]
    pub answers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<String>,
}
