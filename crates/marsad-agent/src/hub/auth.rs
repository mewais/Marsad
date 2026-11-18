//! Interactive sign-in for hub links.
//!
//! ssh runs with `SSH_ASKPASS=<this binary>` and `SSH_ASKPASS_REQUIRE=force`; every prompt it
//! has (password, passphrase, one-time code, keyboard-interactive, host-key confirmation)
//! becomes a `marsad-agent askpass` process that calls `hosts.askpass` here and waits. The
//! prompt is published (fleet `auth_prompt`) until a client answers, cancels, or it times out.
//! Answers are handed to the waiting helper only: never logged, never stored.
//!
//! [`Prompts`] also holds the per-host "armed" bit: a host that signs in interactively only
//! connects when the user asked for it (`hosts.reconnect`), never on its own.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use marsad_proto::hosts::{AuthPrompt, PromptKind};
use marsad_proto::{HostId, MarsadError};
use tokio::sync::oneshot;

use super::fleet::Fleet;
use crate::util::now_ms;

struct Pending {
    prompt: AuthPrompt,
    answer: oneshot::Sender<Option<String>>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    pending: HashMap<HostId, Pending>,
    /// Prompts asked in the current connect attempt of each host.
    attempts: HashMap<HostId, u32>,
    /// Hosts whose current attempt was cancelled (further prompts fail at once).
    cancelled: HashSet<HostId>,
    armed: HashSet<HostId>,
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct Prompts {
    state: Arc<Mutex<State>>,
    fleet: Fleet,
    pub timeout: Duration,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Prompts {
    pub fn new(fleet: Fleet, timeout: Duration) -> Self {
        Prompts {
            state: Arc::default(),
            fleet,
            timeout,
        }
    }

    /// A connect attempt of `host` starts: reset its prompt count.
    pub fn begin_attempt(&self, host: &HostId) {
        let mut s = lock(&self.state);
        s.attempts.remove(host);
        s.cancelled.remove(host);
    }

    /// Prompts asked in the current attempt.
    pub fn asked(&self, host: &HostId) -> u32 {
        lock(&self.state).attempts.get(host).copied().unwrap_or(0)
    }

    /// Allow one connect attempt of an interactive host.
    pub fn arm(&self, host: &HostId) {
        lock(&self.state).armed.insert(host.clone());
    }

    /// Consume the allowance.
    pub fn take_arm(&self, host: &HostId) -> bool {
        lock(&self.state).armed.remove(host)
    }

    pub fn current(&self, host: &HostId) -> Option<AuthPrompt> {
        lock(&self.state)
            .pending
            .get(host)
            .map(|p| p.prompt.clone())
    }

    /// Wait for the user's answer to `text` (called for `hosts.askpass`).
    pub async fn ask(&self, host: HostId, text: String) -> Result<String, MarsadError> {
        let (tx, rx) = oneshot::channel();
        let prompt = {
            let mut s = lock(&self.state);
            if s.cancelled.contains(&host) {
                return Err(MarsadError::invalid("sign-in was cancelled"));
            }
            s.next_id += 1;
            let attempt = {
                let a = s.attempts.entry(host.clone()).or_insert(0);
                *a += 1;
                *a
            };
            let prompt = AuthPrompt {
                id: s.next_id,
                host: host.clone(),
                kind: PromptKind::guess(&text),
                prompt: text,
                attempt,
                expires_at_ms: now_ms() + self.timeout.as_millis() as u64,
            };
            // A newer prompt replaces an older one (ssh asks one at a time).
            if let Some(old) = s.pending.insert(
                host.clone(),
                Pending {
                    prompt: prompt.clone(),
                    answer: tx,
                },
            ) {
                // The older helper may be gone.
                let _ = old.answer.send(None);
            }
            prompt
        };
        tracing::info!(host = %host, id = prompt.id, kind = ?prompt.kind, "sign-in prompt");
        let id = prompt.id;
        self.fleet.auth_prompt(host.clone(), Some(prompt)).await;
        let r = tokio::time::timeout(self.timeout, rx).await;
        // Clear the published prompt unless a newer one has replaced it.
        let superseded = {
            let mut s = lock(&self.state);
            match s.pending.get(&host).map(|p| p.prompt.id) {
                Some(cur) if cur == id => {
                    s.pending.remove(&host);
                    false
                }
                Some(_) => true,
                None => false,
            }
        };
        if !superseded {
            self.fleet.auth_prompt(host.clone(), None).await;
        }
        match r {
            Ok(Ok(Some(answer))) => Ok(answer),
            Ok(Ok(None)) | Ok(Err(_)) => Err(MarsadError::invalid("sign-in was cancelled")),
            Err(_) => {
                lock(&self.state).cancelled.insert(host.clone());
                Err(MarsadError::Timeout {
                    what: format!("sign-in prompt for {host}"),
                })
            }
        }
    }

    fn resolve(&self, host: &HostId, id: u64, answer: Option<String>) -> Result<(), MarsadError> {
        let mut s = lock(&self.state);
        match s.pending.get(host) {
            Some(p) if p.prompt.id == id => {}
            _ => {
                return Err(MarsadError::NotFound {
                    what: format!("prompt {id} of {host}"),
                })
            }
        }
        if answer.is_none() {
            s.cancelled.insert(host.clone());
        }
        if let Some(p) = s.pending.remove(host) {
            // The helper may have timed out a moment ago.
            let _ = p.answer.send(answer);
        }
        Ok(())
    }

    pub fn answer(&self, host: &HostId, id: u64, answer: String) -> Result<(), MarsadError> {
        self.resolve(host, id, Some(answer))
    }

    pub fn cancel(&self, host: &HostId, id: u64) -> Result<(), MarsadError> {
        self.resolve(host, id, None)
    }
}
