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
//! newer prompt, the helper going away, or its attempt ending), never its text and never the
//! answer.
//!
//! Prompts belong to a connect attempt. The connector begins one ([`Prompts::begin_attempt`])
//! and holds the returned [`SignInAttempt`] for as long as the attempt lives; its number goes to
//! the ssh it starts (`MARSAD_ASKPASS_ATTEMPT`) and from there to every helper, which
//! `hosts.askpass` hands to [`Prompts::ask`]. Only the attempt in progress can raise a prompt;
//! when it ends, for any reason (the guard is dropped: it finished, failed, was cut short by the
//! supervisor, or its link was stopped), the prompt it left is withdrawn at that moment, and a
//! helper that outlived it is refused. So a prompt can never be answered into a newer attempt,
//! and a helper's timeout can never cancel a newer attempt's prompt.
//!
//! [`Prompts`] also owns the per-host "armed" bit: a host that signs in interactively only
//! connects when the user asked for it (`hosts.reconnect`), never on its own. The bit is the
//! user's click, spent when an attempt asks them something or connects; a failure before
//! that (no network) keeps it, because nothing was asked of anybody.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use marsad_proto::diag::{CancelWhy, MarsadEvent};
use marsad_proto::hosts::{AuthPrompt, PromptKind, SignInRepeat};
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

/// One prompt asked in an attempt.
struct Asked {
    kind: PromptKind,
    text: String,
}

/// Everything about one host's prompts: its current connect attempt, the prompt waiting for the
/// user, and the click that allows the next attempt. One owner for it all.
#[derive(Default)]
struct HostState {
    /// The connect attempt in progress (or the last one): the number [`Prompts::begin_attempt`]
    /// gave it. `0`: none has begun.
    attempt: u64,
    /// `attempt` has ended: no prompt of it is taken any more.
    ended: bool,
    /// Prompts asked in `attempt`, in order (a repeated prompt means the previous answer was
    /// wrong; a different one is the next step, e.g. a one-time code after the password).
    asked: Vec<Asked>,
    /// Where the current connection's prompts begin in `asked` (an attempt may make a second
    /// connection, see [`SignInAttempt::again`]).
    round: usize,
    /// Why the current connection is a repeat within one sign-in.
    repeat: Option<SignInRepeat>,
    /// `attempt` was cancelled (by the user, or nobody answered): its further prompts fail at
    /// once.
    cancelled: bool,
    /// The prompt waiting for the user (always one of `attempt`).
    pending: Option<Pending>,
    /// The user asked for the next attempt.
    armed: bool,
}

impl HostState {
    /// Resolve the waiting prompt (if any) as cancelled for `why`. The `ask` that raised it
    /// wakes, records its end and takes it off the fleet.
    fn withdraw(&mut self, why: CancelWhy) {
        if let Some(p) = self.pending.take() {
            // The helper may have timed out a moment ago.
            let _ = p.answer.send(Resolution::Cancelled(why));
        }
    }
}

#[derive(Default)]
struct State {
    next_prompt: u64,
    next_attempt: u64,
    hosts: HashMap<HostId, HostState>,
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

    /// A connect attempt of `host` begins. A prompt left by the previous attempt (one that did
    /// not end it) is withdrawn. `repeat` says why the attempt's prompts are a second sign-in
    /// of one Connect (the hub restarts the host's helper and connects again); the guard ends
    /// the attempt when it is dropped.
    pub fn begin_attempt(&self, host: &HostId, repeat: Option<SignInRepeat>) -> SignInAttempt {
        let tag = {
            let mut s = lock(&self.state);
            s.next_attempt += 1;
            let tag = s.next_attempt;
            let h = s.hosts.entry(host.clone()).or_default();
            h.withdraw(CancelWhy::AttemptEnded);
            h.attempt = tag;
            h.ended = false;
            h.asked.clear();
            h.round = 0;
            h.repeat = repeat;
            h.cancelled = false;
            tag
        };
        SignInAttempt {
            prompts: self.clone(),
            host: host.clone(),
            tag,
        }
    }

    fn end_attempt(&self, host: &HostId, tag: u64) {
        let mut s = lock(&self.state);
        if let Some(h) = s
            .hosts
            .get_mut(host)
            .filter(|h| h.attempt == tag && !h.ended)
        {
            h.ended = true;
            h.withdraw(CancelWhy::AttemptEnded);
        }
    }

    /// The host is gone (removed, or its route changed): its prompt is withdrawn, its attempt
    /// ended, its click forgotten, and no helper of it is taken again.
    pub fn forget(&self, host: &HostId) {
        if let Some(mut h) = lock(&self.state).hosts.remove(host) {
            h.withdraw(CancelWhy::AttemptEnded);
        }
    }

    /// The user asked for the next attempt of `host` (to sign in interactively).
    pub fn arm(&self, host: &HostId) {
        lock(&self.state)
            .hosts
            .entry(host.clone())
            .or_default()
            .armed = true;
    }

    /// Whether the user's click is waiting to be spent.
    pub fn armed(&self, host: &HostId) -> bool {
        lock(&self.state).hosts.get(host).is_some_and(|h| h.armed)
    }

    /// The click is spent (an attempt connected without needing it).
    pub fn disarm(&self, host: &HostId) {
        if let Some(h) = lock(&self.state).hosts.get_mut(host) {
            h.armed = false;
        }
    }

    /// Wait for the user's answer to `text`, asked by a helper of connect attempt `attempt`
    /// of `host` (called for `hosts.askpass`). Refused at once when that attempt is not the
    /// one in progress.
    pub async fn ask(
        &self,
        host: HostId,
        attempt: u64,
        text: String,
    ) -> Result<String, MarsadError> {
        let (tx, rx) = oneshot::channel();
        let prompt = {
            let mut s = lock(&self.state);
            let id = s.next_prompt + 1;
            let h = s.hosts.entry(host.clone()).or_default();
            if h.attempt != attempt || h.ended {
                return Err(MarsadError::invalid(
                    "this sign-in attempt has ended; a prompt of it is not taken",
                ));
            }
            if h.cancelled {
                return Err(MarsadError::invalid("sign-in was cancelled"));
            }
            let kind = PromptKind::guess(&text);
            h.asked.push(Asked {
                kind,
                text: text.clone(),
            });
            let again = h.asked[h.round..].iter().filter(|a| a.text == text).count() as u32;
            // The user's click is spent the moment the attempt asks them something.
            h.armed = false;
            let prompt = AuthPrompt {
                id,
                host: host.clone(),
                kind,
                prompt: text,
                attempt: again,
                expires_at_ms: now_ms() + self.timeout.as_millis() as u64,
                repeat: h.repeat,
            };
            // A newer prompt replaces an older one (ssh asks one at a time).
            h.withdraw(CancelWhy::Superseded);
            h.pending = Some(Pending {
                prompt: prompt.clone(),
                answer: tx,
            });
            s.next_prompt = id;
            prompt
        };
        let id = prompt.id;
        emit(MarsadEvent::AuthPrompt {
            host: Ident::new(host.as_str()),
            prompt: id,
            kind: prompt.kind.into(),
            attempt: prompt.attempt,
            repeat: prompt.repeat.map(Into::into),
        });
        // From here on this prompt has an end, and `Waiting` records it: when this future is
        // dropped before then, the helper that asked is gone (its connection closed).
        let mut waiting = Waiting::new(self.clone(), host.clone(), id);
        self.fleet.show_prompt(host.clone(), prompt).await;
        let r = tokio::time::timeout(self.timeout, rx).await;
        {
            let mut s = lock(&self.state);
            if let Some(h) = s.hosts.get_mut(&host) {
                if h.pending.as_ref().is_some_and(|p| p.prompt.id == id) {
                    h.pending = None;
                }
                // Nobody answered: the attempt this prompt belongs to is cancelled, no other.
                if r.is_err() && h.attempt == attempt && !h.ended {
                    h.cancelled = true;
                }
            }
        }
        // This prompt's end, whoever caused it. A newer prompt of the host is not touched.
        self.fleet.end_prompt(host.clone(), id).await;
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
                waiting.end(Ending::Cancelled(CancelWhy::AttemptEnded));
                Err(MarsadError::invalid("sign-in was cancelled"))
            }
            Err(_) => {
                waiting.end(Ending::Cancelled(CancelWhy::Timeout));
                Err(MarsadError::Timeout {
                    what: format!("sign-in prompt for {host}"),
                })
            }
        }
    }

    fn resolve(&self, host: &HostId, id: u64, answer: Option<String>) -> Result<(), MarsadError> {
        let mut s = lock(&self.state);
        let Some(h) = s.hosts.get_mut(host) else {
            return Err(not_found(host, id));
        };
        match &h.pending {
            Some(p) if p.prompt.id == id => {}
            _ => return Err(not_found(host, id)),
        }
        if answer.is_none() {
            h.cancelled = true;
        }
        if let Some(p) = h.pending.take() {
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

fn not_found(host: &HostId, id: u64) -> MarsadError {
    MarsadError::NotFound {
        what: format!("prompt {id} of {host}"),
    }
}

/// A connect attempt's hold on its prompts: what it was asked and, when dropped, its end (the
/// prompt it left is withdrawn and its helpers are refused from then on).
pub struct SignInAttempt {
    prompts: Prompts,
    host: HostId,
    tag: u64,
}

impl SignInAttempt {
    /// The attempt's number, for the ssh it starts to hand to its askpass helpers.
    pub fn tag(&self) -> u64 {
        self.tag
    }

    fn with<T>(&self, f: impl FnOnce(&mut HostState) -> T) -> Option<T> {
        let mut s = lock(&self.prompts.state);
        s.hosts
            .get_mut(&self.host)
            .filter(|h| h.attempt == self.tag)
            .map(f)
    }

    /// How many prompts the attempt has been asked.
    pub fn asked(&self) -> u32 {
        self.with(|h| h.asked.len() as u32).unwrap_or(0)
    }

    /// Whether the attempt asked the user for something at login: any prompt for which
    /// [`PromptKind::needs_user`] holds. The one place the question "does this host need the
    /// user?" is answered from what happened.
    pub fn needed_user(&self) -> bool {
        self.with(|h| h.asked.iter().any(|a| a.kind.needs_user()))
            .unwrap_or(false)
    }

    /// The attempt makes another connection within the same sign-in (the host turned out to
    /// need another shell): from now on its prompts are a repeat for `why`, counted afresh.
    pub fn again(&self, why: SignInRepeat) {
        self.with(|h| {
            h.round = h.asked.len();
            h.repeat = Some(why);
        });
    }
}

impl Drop for SignInAttempt {
    fn drop(&mut self) {
        self.prompts.end_attempt(&self.host, self.tag);
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
        {
            let mut s = lock(&self.prompts.state);
            if let Some(h) = s.hosts.get_mut(&self.host) {
                if h.pending.as_ref().is_some_and(|p| p.prompt.id == self.id) {
                    h.pending = None;
                }
            }
        }
        let (fleet, host, id) = (self.prompts.fleet.clone(), self.host.clone(), self.id);
        // Dropped inside the runtime (an abandoned request); at its shutdown there is
        // nobody left to tell.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { fleet.end_prompt(host, id).await });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use marsad_proto::fleet::FleetLink;
    use marsad_proto::hosts::{HostEntry, HostKind};

    const T: Duration = Duration::from_secs(5);

    fn entry(id: &str) -> HostEntry {
        HostEntry {
            id: HostId::new(id),
            name: id.into(),
            kind: HostKind::Wsl { distro: "d".into() },
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            local_files: false,
            machine: None,
        }
    }

    async fn setup(host: &str) -> (Prompts, Fleet, HostId) {
        let fleet = Fleet::spawn();
        let id = HostId::new(host);
        fleet
            .host_upserted(
                entry(host),
                FleetLink::Remote {
                    state: marsad_proto::LinkState::Connecting,
                },
            )
            .await;
        (
            Prompts::new(fleet.clone(), Duration::from_secs(30)),
            fleet,
            id,
        )
    }

    async fn showing(fleet: &Fleet, host: &HostId) -> Option<AuthPrompt> {
        fleet
            .snapshot()
            .await
            .into_iter()
            .find(|h| h.host.id == *host)
            .and_then(|h| h.auth_prompt)
    }

    /// Wait (on the fleet's own events, not on time) until `pred` holds of what is showing.
    async fn until_showing(
        fleet: &Fleet,
        host: &HostId,
        pred: impl Fn(&Option<AuthPrompt>) -> bool,
    ) -> Option<AuthPrompt> {
        let mut sub = fleet.subscribe(None).await.unwrap();
        loop {
            let now = showing(fleet, host).await;
            if pred(&now) {
                return now;
            }
            tokio::time::timeout(T, sub.next())
                .await
                .expect("the fleet showed nothing new")
                .expect("the fleet's stream ended")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_prompt_goes_with_its_attempt_and_a_helper_of_an_ended_attempt_is_refused() {
        let (prompts, fleet, host) = setup("h").await;
        let first = prompts.begin_attempt(&host, None);
        let (p2, h2, t1) = (prompts.clone(), host.clone(), first.tag());
        let asking = tokio::spawn(async move { p2.ask(h2, t1, "password:".into()).await });
        let shown = until_showing(&fleet, &host, Option::is_some).await.unwrap();
        assert_eq!(
            (shown.kind, shown.attempt, shown.repeat),
            (PromptKind::Secret, 1, None)
        );
        assert!(!prompts.armed(&host), "asking spent the click");

        // The attempt ends (its future was dropped, say): the prompt is withdrawn at once,
        // the helper is refused, and nobody can answer the prompt any more.
        drop(first);
        let r = tokio::time::timeout(T, asking).await.unwrap().unwrap();
        assert!(r.is_err());
        until_showing(&fleet, &host, Option::is_none).await;
        assert!(prompts.answer(&host, shown.id, "x".into()).is_err());

        // A new attempt is not touched by the old one's helper, however it asks.
        let second = prompts.begin_attempt(&host, None);
        let stale = prompts.ask(host.clone(), 1, "password:".into()).await;
        assert!(stale.is_err());
        assert!(showing(&fleet, &host).await.is_none());
        let (p3, h3, t2) = (prompts.clone(), host.clone(), second.tag());
        let asking = tokio::spawn(async move { p3.ask(h3, t2, "password:".into()).await });
        let shown2 = until_showing(&fleet, &host, Option::is_some).await.unwrap();
        assert_ne!(shown2.id, shown.id);
        // The old prompt's id cannot answer the new attempt's prompt.
        assert!(prompts.answer(&host, shown.id, "x".into()).is_err());
        prompts.answer(&host, shown2.id, "pw".into()).unwrap();
        assert_eq!(asking.await.unwrap().unwrap(), "pw");
        assert!(second.needed_user());
    }

    #[tokio::test]
    async fn a_new_attempt_withdraws_the_prompt_the_last_one_left() {
        let (prompts, fleet, host) = setup("h").await;
        let first = prompts.begin_attempt(&host, None);
        let (p2, h2, t1) = (prompts.clone(), host.clone(), first.tag());
        let asking = tokio::spawn(async move { p2.ask(h2, t1, "Enter passphrase:".into()).await });
        until_showing(&fleet, &host, Option::is_some).await.unwrap();
        // Another attempt begins before the first was dropped.
        let _second = prompts.begin_attempt(&host, Some(SignInRepeat::HelperRestart));
        assert!(asking.await.unwrap().is_err());
        until_showing(&fleet, &host, Option::is_none).await;
        // The first attempt's guard no longer speaks for the host.
        assert_eq!(first.asked(), 0);
        drop(first);
        assert!(!prompts.state.lock().unwrap().hosts[&host].ended);
    }

    #[tokio::test]
    async fn a_timeout_cancels_its_own_attempt_only() {
        let (prompts, fleet, host) = setup("h").await;
        let short = Prompts::new(fleet.clone(), Duration::from_millis(20));
        // `short` has its own state: use it for the timeout, and check the flag it sets.
        let a = short.begin_attempt(&host, None);
        let r = short.ask(host.clone(), a.tag(), "password:".into()).await;
        assert!(matches!(r, Err(MarsadError::Timeout { .. })));
        // Further prompts of the same attempt fail at once; a new attempt asks again.
        let again = short.ask(host.clone(), a.tag(), "password:".into()).await;
        assert!(again.is_err());
        let b = short.begin_attempt(&host, None);
        let r = short.ask(host.clone(), b.tag(), "password:".into()).await;
        assert!(
            matches!(r, Err(MarsadError::Timeout { .. })),
            "the new attempt's prompt was asked, not refused as cancelled"
        );
        let _ = prompts;
    }

    #[tokio::test]
    async fn only_a_host_key_confirmation_does_not_need_the_user_and_the_click_is_spent_by_asking()
    {
        let (prompts, fleet, host) = setup("h").await;
        prompts.arm(&host);
        let a = prompts.begin_attempt(&host, None);
        assert!(
            prompts.armed(&host),
            "a click survives the start of an attempt"
        );
        let (p2, h2, t) = (prompts.clone(), host.clone(), a.tag());
        let asking = tokio::spawn(async move {
            p2.ask(
                h2,
                t,
                "The authenticity of host 'x' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ".into(),
            )
            .await
        });
        let shown = until_showing(&fleet, &host, Option::is_some).await.unwrap();
        assert_eq!(shown.kind, PromptKind::Confirm);
        assert!(!prompts.armed(&host));
        prompts.answer(&host, shown.id, "yes".into()).unwrap();
        asking.await.unwrap().unwrap();
        assert_eq!((a.asked(), a.needed_user()), (1, false));
        // A passphrase (or a code, a password, a site's own question) is the user's.
        let (p3, h3) = (prompts.clone(), host.clone());
        let asking =
            tokio::spawn(
                async move { p3.ask(h3, t, "Enter passphrase for key 'k': ".into()).await },
            );
        let shown = until_showing(&fleet, &host, Option::is_some).await.unwrap();
        prompts.answer(&host, shown.id, "pw".into()).unwrap();
        asking.await.unwrap().unwrap();
        assert_eq!((a.asked(), a.needed_user()), (2, true));
    }

    #[tokio::test]
    async fn forgetting_a_host_withdraws_its_prompt_and_drops_its_click() {
        let (prompts, fleet, host) = setup("h").await;
        prompts.arm(&host);
        let a = prompts.begin_attempt(&host, None);
        let (p2, h2, t) = (prompts.clone(), host.clone(), a.tag());
        let asking = tokio::spawn(async move { p2.ask(h2, t, "password:".into()).await });
        until_showing(&fleet, &host, Option::is_some).await.unwrap();
        prompts.arm(&host);
        prompts.forget(&host);
        assert!(asking.await.unwrap().is_err());
        until_showing(&fleet, &host, Option::is_none).await;
        assert!(!prompts.armed(&host));
        // Its helpers are refused from now on.
        assert!(prompts
            .ask(host.clone(), t, "password:".into())
            .await
            .is_err());
    }
}
