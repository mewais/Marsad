//! Interactive sign-in for hub links.
//!
//! ssh runs with `SSH_ASKPASS=<this binary>` and `SSH_ASKPASS_REQUIRE=force`; every prompt it
//! has (password, passphrase, one-time code, keyboard-interactive, host-key confirmation)
//! becomes a `marsad-agent askpass` process that calls `hosts.askpass` here and waits. The
//! prompt is published (fleet `auth_prompt`) until a client answers, cancels, or it times out.
//! Answers are handed to the waiting helper only: never logged, never stored; every answer is
//! registered with the diagnostics scrubber before anything else can see it, so that a text
//! that echoes it (an ssh message, a banner) is masked in records. A prompt's life is recorded
//! (`auth.prompt`, then `auth.answered` or `auth.cancelled` with why: the user, the timeout, a
//! newer prompt, or the helper going away), never its text and never the answer.
//!
//! [`Prompts`] also holds the per-host "armed" bit: a host that signs in interactively only
//! connects when the user asked for it (`hosts.reconnect`), never on its own.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use marsad_proto::diag::{CancelWhy, MarsadEvent};
use marsad_proto::hosts::{AuthPrompt, PromptKind};
use marsad_proto::{HostId, MarsadError};
use tokio::sync::oneshot;
use wasit_diag::Ident;

use super::fleet::Fleet;
use crate::diag::{self, emit};
use crate::util::now_ms;

/// How a pending prompt was resolved by someone other than the waiting helper.
enum Resolution {
    Answer(String),
    Cancelled(CancelWhy),
}

struct Pending {
    prompt: AuthPrompt,
    answer: oneshot::Sender<Resolution>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    pending: HashMap<HostId, Pending>,
    /// Prompts asked in the current connect attempt of each host, in order (a repeated
    /// prompt means the previous answer was wrong; a different one is the next step, e.g. a
    /// one-time code after the password).
    attempts: HashMap<HostId, Vec<String>>,
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
        lock(&self.state)
            .attempts
            .get(host)
            .map_or(0, |v| v.len() as u32)
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
                let asked = s.attempts.entry(host.clone()).or_default();
                asked.push(text.clone());
                asked.iter().filter(|t| **t == text).count() as u32
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
                let _ = old
                    .answer
                    .send(Resolution::Cancelled(CancelWhy::Superseded));
            }
            prompt
        };
        let id = prompt.id;
        emit(MarsadEvent::AuthPrompt {
            host: Ident::new(host.as_str()),
            prompt: id,
            kind: prompt.kind.into(),
            attempt: prompt.attempt,
        });
        // From here on this prompt has an end, and `Waiting` records it: when this future is
        // dropped before then, the helper that asked is gone (its connection closed).
        let mut waiting = Waiting::new(self.clone(), host.clone(), id);
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
            Ok(Ok(Resolution::Answer(answer))) => {
                waiting.end(Ending::Answered);
                Ok(answer)
            }
            Ok(Ok(Resolution::Cancelled(why))) => {
                waiting.end(Ending::Cancelled(why));
                Err(MarsadError::invalid("sign-in was cancelled"))
            }
            // The sender was dropped unanswered: nobody can answer this prompt any more.
            Ok(Err(_)) => {
                waiting.end(Ending::Cancelled(CancelWhy::User));
                Err(MarsadError::invalid("sign-in was cancelled"))
            }
            Err(_) => {
                lock(&self.state).cancelled.insert(host.clone());
                waiting.end(Ending::Cancelled(CancelWhy::Timeout));
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
            let _ = p.answer.send(match answer {
                Some(a) => Resolution::Answer(a),
                None => Resolution::Cancelled(CancelWhy::User),
            });
        }
        Ok(())
    }

    pub fn answer(&self, host: &HostId, id: u64, answer: String) -> Result<(), MarsadError> {
        // First, before anything else can see it: a text that echoes the answer is masked from
        // now on, in every record of this process.
        wasit_diag::scrub::register_secret(&answer);
        self.resolve(host, id, Some(answer))
    }

    pub fn cancel(&self, host: &HostId, id: u64) -> Result<(), MarsadError> {
        self.resolve(host, id, None)
    }
}

/// How an `ask` ended.
enum Ending {
    Answered,
    Cancelled(CancelWhy),
}

/// The record of one prompt's end, written once. If the `ask` future is dropped before it ended
/// (the askpass helper's connection closed: ssh gave up on it or was killed), the helper is gone:
/// that is recorded, and the prompt it left is withdrawn so that nobody is asked to answer
/// something no one waits for.
struct Waiting {
    prompts: Prompts,
    host: HostId,
    id: u64,
    began: Instant,
    ended: bool,
}

impl Waiting {
    fn new(prompts: Prompts, host: HostId, id: u64) -> Waiting {
        Waiting {
            prompts,
            host,
            id,
            began: Instant::now(),
            ended: false,
        }
    }

    fn end(&mut self, how: Ending) {
        self.ended = true;
        let host = Ident::new(self.host.as_str());
        emit(match how {
            Ending::Answered => MarsadEvent::AuthAnswered {
                host,
                prompt: self.id,
                ms: diag::ms(self.began),
            },
            Ending::Cancelled(why) => MarsadEvent::AuthCancelled {
                host,
                prompt: self.id,
                why,
            },
        });
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        self.end(Ending::Cancelled(CancelWhy::HelperGone));
        // Withdraw the prompt if it is still the host's current one (a newer prompt of the
        // host is not ours to clear).
        let still_ours = {
            let mut s = lock(&self.prompts.state);
            match s.pending.get(&self.host).map(|p| p.prompt.id) {
                Some(cur) if cur == self.id => {
                    s.pending.remove(&self.host);
                    true
                }
                _ => false,
            }
        };
        if still_ours {
            let (fleet, host) = (self.prompts.fleet.clone(), self.host.clone());
            // Dropped inside the runtime (an abandoned request); at its shutdown there is
            // nobody left to tell.
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move { fleet.auth_prompt(host, None).await });
            }
        }
    }
}
