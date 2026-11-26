//! The hub's host registry: persisted host entries (marsad-store), one supervised link per
//! remote host (wasit-control `Hub`), each host's session feed into the fleet, and platform
//! updates found by provisioning.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use marsad_proto::diag::{
    HostChange, HostShape, HostsOp, LearnedWord, MarsadEvent, MasterExitOutcome, MasterExitWhy,
};
use marsad_proto::fleet::FleetLink;
use marsad_proto::hosts::{HostDraft, HostEntry, HostKind, ImportSuggestions};
use marsad_proto::{HostId, LinkState, MarsadError};
use marsad_store::Store;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wasit_control::{HostSpec, Hub};
use wasit_diag::Ident;

use super::auth::Prompts;
use super::fleet::Fleet;
use super::master::Masters;
use super::provision::{Entries, Learned};
use super::transport::{safe_host_id, transport_prefix};
use crate::diag::{self, emit};
use crate::util::now_ms;

/// A host entry changed: recorded by its id and the shape of its kind, never its target.
fn changed(entry: &HostEntry, op: HostChange) {
    let shape = HostShape::of(&entry.kind);
    emit(MarsadEvent::HostsChanged {
        host: Ident::new(entry.id.as_str()),
        op,
        kind: shape.kind,
        target_kind: shape.target,
        port_set: shape.port_set,
        identity_set: shape.identity_set,
        options: shape.options,
    });
}

fn failed(host: Option<&HostId>, op: HostsOp, e: &MarsadError) {
    emit(MarsadEvent::HostsFailed {
        host: host.map(|h| Ident::new(h.as_str())),
        op,
        // A store's or a link's error may quote what the user typed (a target, a path).
        error: diag::foreign("hosts", &e.to_string()),
    });
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    hub: Hub,
    fleet: Fleet,
    entries: Entries,
    local: HostEntry,
    prompts: Prompts,
    /// The shared ssh connections: ended with their host (removed, or its route edited) or at
    /// the user's sign-out.
    masters: Masters,
    feeds: Mutex<HashMap<HostId, JoinHandle<()>>>,
    /// Held across every read-modify-write of a stored entry (`update`, `learned`): an edit and
    /// something a connect learned at the same time cannot overwrite each other.
    edits: tokio::sync::Mutex<()>,
}

fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "host".into()
    } else {
        s
    }
}

/// What a [`Registry`] is made of: the pieces the hub builds before it.
pub struct RegistryParts {
    pub store: Store,
    pub hub: Hub,
    pub fleet: Fleet,
    pub entries: Entries,
    pub local: HostEntry,
    pub prompts: Prompts,
    pub masters: Masters,
}

impl Registry {
    /// Load the stored hosts, start their links and feeds, and consume platform updates.
    pub async fn start(
        parts: RegistryParts,
        mut learned: mpsc::Receiver<Learned>,
    ) -> Result<Registry, MarsadError> {
        let RegistryParts {
            store,
            hub,
            fleet,
            entries,
            local,
            prompts,
            masters,
        } = parts;
        let r = Registry {
            inner: Arc::new(Inner {
                store,
                hub,
                fleet,
                entries,
                local,
                prompts,
                masters,
                feeds: Mutex::new(HashMap::new()),
                edits: tokio::sync::Mutex::new(()),
            }),
        };
        r.inner
            .fleet
            .host_upserted(r.inner.local.clone(), FleetLink::Local)
            .await;
        for h in r.inner.store.hosts().await? {
            if Masters::applies(&h) {
                if h.signed_out {
                    r.inner.masters.mark_signed_out(&h.id);
                } else {
                    r.inner.masters.set_signed_out(&h.id, false);
                }
            }
            if let Err(e) = r.link(h.clone()).await {
                failed(Some(&h.id), HostsOp::StartLink, &e);
            }
        }
        let r2 = r.clone();
        tokio::spawn(async move {
            while let Some(l) = learned.recv().await {
                let host = l.host().clone();
                if let Err(e) = r2.learned(l).await {
                    failed(Some(&host), HostsOp::Learn, &e);
                }
            }
        });
        Ok(r)
    }

    pub fn local(&self) -> &HostEntry {
        &self.inner.local
    }

    pub fn hub(&self) -> &Hub {
        &self.inner.hub
    }

    /// All hosts, the local one first.
    pub fn list(&self) -> Vec<HostEntry> {
        let mut v: Vec<HostEntry> = self
            .inner
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        v.insert(0, self.inner.local.clone());
        v
    }

    pub fn get(&self, id: &HostId) -> Option<HostEntry> {
        if *id == self.inner.local.id {
            return Some(self.inner.local.clone());
        }
        self.inner
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    async fn link(&self, entry: HostEntry) -> Result<(), MarsadError> {
        let mut entry = entry;
        entry.derive_local_files();
        // The connector builds each attempt's command itself (it depends on the route of the
        // attempt); the spec carries the transport, for what only names the link.
        let argv = transport_prefix(&entry.kind)
            .filter(|_| safe_host_id(entry.id.as_str()))
            .ok_or_else(|| {
                MarsadError::invalid("no link for this host (the local host, or an unusable id)")
            })?;
        self.inner
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(entry.id.clone(), entry.clone());
        // The fleet must know the host before its first link state arrives.
        self.inner
            .fleet
            .host_upserted(
                entry.clone(),
                FleetLink::Remote {
                    state: LinkState::Connecting {
                        stage: None,
                        since_ms: 0,
                    },
                },
            )
            .await;
        self.inner
            .hub
            .add_link(HostSpec::new(entry.id.clone(), argv))
            .map_err(|e| MarsadError::invalid(e.to_string()))?;
        let Some(sup) = self.inner.hub.link(&entry.id) else {
            return Err(MarsadError::invalid("the link vanished while starting"));
        };
        let feed = self.inner.fleet.feed_remote(entry.id.clone(), sup);
        if let Some(old) = self
            .inner
            .feeds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(entry.id.clone(), feed)
        {
            old.abort();
        }
        Ok(())
    }

    async fn unlink(&self, id: &HostId) {
        let feed = self
            .inner
            .feeds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(f) = feed {
            f.abort();
        }
        self.inner.hub.remove_link(id).await;
    }

    pub async fn add(&self, draft: HostDraft) -> Result<HostEntry, MarsadError> {
        if matches!(draft.kind, HostKind::Local) {
            return Err(MarsadError::invalid("the local host always exists"));
        }
        if draft.name.trim().is_empty() {
            return Err(MarsadError::invalid("a host needs a name"));
        }
        if let HostKind::Wsl { distro } = &draft.kind {
            super::wsl::check_distro(None, distro).await?;
        }
        let taken = |id: &HostId| self.get(id).is_some();
        let id = match draft.id {
            Some(id) if taken(&id) => {
                return Err(MarsadError::invalid(format!("host id {id} is taken")))
            }
            Some(id) if !super::transport::safe_host_id(id.as_str()) => {
                return Err(MarsadError::invalid(
                    "a host id is letters, digits, '-', '_' and '.'",
                ))
            }
            Some(id) => id,
            None => {
                let base = slug(&draft.name);
                let mut id = HostId::new(base.clone());
                let mut n = 2;
                while taken(&id) {
                    id = HostId::new(format!("{base}-{n}"));
                    n += 1;
                }
                id
            }
        };
        let entry = HostEntry {
            id,
            name: draft.name.trim().to_string(),
            kind: draft.kind,
            platform: None,
            created_at_ms: now_ms(),
            interactive_auth: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        let mut entry = entry;
        entry.derive_local_files();
        self.inner.store.put_host(entry.clone()).await?;
        self.link(entry.clone()).await?;
        changed(&entry, HostChange::Added);
        Ok(entry)
    }

    pub async fn update(&self, entry: HostEntry) -> Result<HostEntry, MarsadError> {
        let _edit = self.inner.edits.lock().await;
        let Some(old) = self.get(&entry.id) else {
            return Err(MarsadError::NotFound {
                what: format!("host {}", entry.id),
            });
        };
        if entry.id == self.inner.local.id || matches!(entry.kind, HostKind::Local) {
            return Err(MarsadError::invalid("the local host cannot be changed"));
        }
        if let HostKind::Wsl { distro } = &entry.kind {
            // The distribution the host already has was checked when it was chosen: only a new
            // one is looked up.
            let previous = match &old.kind {
                HostKind::Wsl { distro } => Some(distro.as_str()),
                _ => None,
            };
            super::wsl::check_distro(previous, distro).await?;
        }
        let mut entry = entry;
        entry.derive_local_files();
        entry.created_at_ms = old.created_at_ms;
        if entry.kind != old.kind {
            // A different machine (or route to it): what we knew about it no longer holds.
            entry.platform = None;
            entry.interactive_auth = false;
            entry.machine = None;
        } else {
            entry.platform = old.platform.clone();
            entry.interactive_auth = old.interactive_auth;
            entry.machine = old.machine.clone();
        }
        entry.signed_out = old.signed_out;
        self.inner.store.put_host(entry.clone()).await?;
        if entry.kind != old.kind {
            self.unlink(&entry.id).await;
            // The old machine's sessions, git rows and feed error go with its route: the host is
            // removed from the fleet, then added again by the new route's link.
            self.inner.fleet.host_removed(entry.id.clone()).await;
            self.inner.prompts.forget(&entry.id);
            // The old route's shared connection ends (the new route has its own socket and
            // signs in on its own).
            self.end_shared(&old, MasterExitWhy::Retargeted).await;
            self.link(entry.clone()).await?;
        } else {
            self.inner
                .entries
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(entry.id.clone(), entry.clone());
            let state = self
                .inner
                .hub
                .link(&entry.id)
                .map(|s| s.current_state())
                .unwrap_or(LinkState::Connecting {
                    stage: None,
                    since_ms: 0,
                });
            self.inner
                .fleet
                .host_upserted(entry.clone(), FleetLink::Remote { state })
                .await;
        }
        changed(&entry, HostChange::Updated);
        Ok(entry)
    }

    pub async fn remove(&self, id: &HostId) -> Result<(), MarsadError> {
        if *id == self.inner.local.id {
            return Err(MarsadError::invalid("the local host cannot be removed"));
        }
        let _edit = self.inner.edits.lock().await;
        let removed = self.get(id);
        self.inner.store.remove_host(id).await?;
        self.unlink(id).await;
        // After the link (its attempt ended with it): no prompt, no click is left for the host.
        self.inner.prompts.forget(id);
        if let Some(entry) = &removed {
            self.end_shared(entry, MasterExitWhy::Removed).await;
        }
        self.inner
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.inner.fleet.host_removed(id.clone()).await;
        if let Some(entry) = removed {
            changed(&entry, HostChange::Removed);
        }
        Ok(())
    }

    /// Connect now. For a host that needs the user at login this is the user's explicit
    /// "Connect": it allows the next attempt (the click is spent when that attempt asks them
    /// something or connects, see [`Prompts`]), which then asks.
    ///
    /// Asking to connect after a sign-out clears the stored flag: written under the edits lock
    /// (so no concurrent edit is lost) and awaited, and the fleet is told of it.
    pub async fn reconnect(&self, id: &HostId) -> Result<(), MarsadError> {
        let _edit = self.inner.edits.lock().await;
        if self.inner.hub.link(id).is_none() {
            return Err(MarsadError::NotFound {
                what: format!("link to {id}"),
            });
        }
        if let Some(mut entry) = self.get(id) {
            if entry.signed_out {
                entry.signed_out = false;
                self.inner.store.put_host(entry.clone()).await?;
                self.inner
                    .entries
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(entry.id.clone(), entry.clone());
                let state = self.link_state(id);
                self.inner
                    .fleet
                    .host_upserted(entry, FleetLink::Remote { state })
                    .await;
            }
        }
        // Asking to connect is asking to sign in again, after a sign-out too.
        self.inner.masters.set_signed_out(id, false);
        self.inner.prompts.arm(id);
        if self.inner.hub.reconnect_now(id) {
            Ok(())
        } else {
            // The link went away between the two calls: nothing is left to allow.
            self.inner.prompts.forget(id);
            Err(MarsadError::NotFound {
                what: format!("link to {id}"),
            })
        }
    }

    /// Ends the shared ssh connection of `entry` (if it has one) and forgets what was known of
    /// it, unless the end failed: then the connection is still open and stays tracked. Every
    /// outcome is on record ([`Masters::exit`]).
    async fn end_shared(&self, entry: &HostEntry, why: MasterExitWhy) {
        if Masters::applies(entry)
            && self.inner.masters.exit(entry, why).await == MasterExitOutcome::Failed
        {
            return;
        }
        self.inner.masters.forget(&entry.id);
    }

    /// Sign out of a host's shared ssh connection: it ends, the link that rode it drops, and the
    /// host waits for the user to sign in again (`hosts.reconnect`); nothing reconnects it on
    /// its own, a host with a key included. A link that did not ride the connection is dropped
    /// too.
    ///
    /// The sign-out is stored (under the edits lock, so a concurrent edit is not overwritten)
    /// before the connection is ended, and the lock is held until it has ended: a reconnect
    /// cannot slip in between and have its new connection ended. If ssh cannot end the
    /// connection, the sign-out still stands (the link is dropped and waits for the user), but
    /// the call fails and the connection stays tracked: it is still open.
    pub async fn signout(&self, id: &HostId) -> Result<(), MarsadError> {
        // The sign-out is recorded under the lock; ending the connection is ssh's work and can
        // be slow (a master that does not answer), so it runs after the lock is released: no
        // edit waits for it.
        let entry = {
            let _edit = self.inner.edits.lock().await;
            let mut entry = self.get(id).ok_or_else(|| MarsadError::NotFound {
                what: format!("host {id}"),
            })?;
            if !Masters::applies(&entry) {
                return Err(MarsadError::invalid(
                    "this host has no shared ssh connection to sign out of",
                ));
            }
            if self.inner.hub.link(id).is_none() {
                return Err(MarsadError::NotFound {
                    what: format!("link to {id}"),
                });
            }
            // First the user's word, so that nothing started in between signs in again; a prompt
            // or click of the host is void.
            self.inner.prompts.forget(id);
            self.inner.masters.mark_signed_out(id);
            entry.signed_out = true;
            self.inner.store.put_host(entry.clone()).await?;
            self.inner
                .entries
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(entry.id.clone(), entry.clone());
            entry
        };
        let ended = self
            .inner
            .masters
            .exit(&entry, MasterExitWhy::SignedOut)
            .await;
        // The link that rode the connection drops by itself, and the supervisor then asks
        // whether the host must wait: it must. One that did not ride a connection (there was
        // none to end, or it could not be ended) is dropped here, to the same question.
        if ended != MasterExitOutcome::Ended {
            self.inner.hub.reconnect_now(id);
        }
        // Record the flag with the fleet state that follows from the supervisor. The entry is read
        // again: an edit may have landed while the connection was ending.
        let state = self.link_state(id);
        let entry = self.get(id).unwrap_or(entry);
        self.inner
            .fleet
            .host_upserted(entry, FleetLink::Remote { state })
            .await;
        match ended {
            MasterExitOutcome::Failed => Err(MarsadError::Io {
                message: format!(
                    "signed out of {id}, but its shared ssh connection could not be ended: it may still be open"
                ),
            }),
            _ => Ok(()),
        }
    }

    /// The state of `id`'s link as the fleet shows it.
    fn link_state(&self, id: &HostId) -> LinkState {
        self.inner
            .hub
            .link(id)
            .map(|s| s.current_state())
            .unwrap_or(LinkState::Connecting {
                stage: None,
                since_ms: 0,
            })
    }

    async fn learned(&self, l: Learned) -> Result<(), MarsadError> {
        let id = l.host().clone();
        let _edit = self.inner.edits.lock().await;
        let Some(mut entry) = self.get(&id) else {
            return Ok(());
        };
        let what = match l {
            Learned::Platform { platform, .. } => {
                entry.platform = Some(platform);
                LearnedWord::Platform
            }
            // Who owns "this host needs the user at login": this registry, which stores it
            // with the host. It changes here and nowhere else (and by a change of the host's
            // route, which forgets everything learned): set by the connect that was asked
            // something, cleared by one that was not.
            Learned::Interactive { .. } => {
                if entry.interactive_auth {
                    return Ok(());
                }
                entry.interactive_auth = true;
                LearnedWord::Interactive
            }
            Learned::NotInteractive { .. } => {
                if !entry.interactive_auth {
                    return Ok(());
                }
                entry.interactive_auth = false;
                LearnedWord::NotInteractive
            }
            Learned::Machine { machine, .. } => {
                if entry.machine.as_deref() == Some(machine.as_str()) {
                    return Ok(());
                }
                entry.machine = Some(machine);
                LearnedWord::Machine
            }
        };
        self.inner.store.put_host(entry.clone()).await?;
        // Recorded once stored, before the fleet is told: whoever sees the fleet's event can
        // rely on the record.
        emit(MarsadEvent::HostsLearned {
            host: Ident::new(id.as_str()),
            what,
        });
        self.inner
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(entry.id.clone(), entry.clone());
        let state = self
            .inner
            .hub
            .link(&entry.id)
            .map(|s| s.current_state())
            .unwrap_or(LinkState::Connecting {
                stage: None,
                since_ms: 0,
            });
        self.inner
            .fleet
            .host_upserted(entry, FleetLink::Remote { state })
            .await;
        Ok(())
    }

    /// Hosts from `~/.ssh/config` that are not configured yet.
    pub async fn import_suggestions(&self) -> ImportSuggestions {
        let Some(path) = crate::util::home_dir().map(|h| h.join(".ssh").join("config")) else {
            return ImportSuggestions {
                hosts: vec![],
                source: None,
            };
        };
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(t) => t,
            Err(_) => {
                return ImportSuggestions {
                    hosts: vec![],
                    source: None,
                }
            }
        };
        let targets: Vec<String> = self
            .list()
            .into_iter()
            .filter_map(|h| match h.kind {
                HostKind::Ssh { target, .. } => Some(target),
                _ => None,
            })
            .collect();
        let mut hosts = marsad_proto::hosts::parse_ssh_config(&text);
        for h in &mut hosts {
            h.already_added = targets.contains(&h.alias);
        }
        ImportSuggestions {
            hosts,
            source: Some(path.display().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use marsad_proto::fleet::FleetHost;
    use wasit_control::{Attempt, ClientOptions, ConnectError, ControlClient, Hello, HubConfig};

    use super::*;

    /// Links that never connect: the tests act on the registry, not on a host.
    struct Parked;

    #[async_trait]
    impl wasit_control::LinkConnector for Parked {
        async fn connect(
            &self,
            _spec: &HostSpec,
            _opts: ClientOptions,
            _attempt: Attempt,
        ) -> Result<ControlClient, ConnectError> {
            std::future::pending().await
        }

        async fn hold(&self, _spec: &HostSpec) -> Option<String> {
            Some("test: links are parked".into())
        }
    }

    struct Harness {
        registry: Registry,
        // Read by the Unix-only tests (a Unix socket stands in for a master).
        #[cfg_attr(not(unix), allow(dead_code))]
        masters: Masters,
        fleet: Fleet,
        store: Store,
        dir: std::path::PathBuf,
    }

    async fn harness(name: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!("mrs-hosts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.join("store")).await.unwrap();
        let fleet = Fleet::spawn();
        let prompts = Prompts::new(fleet.clone(), Duration::from_secs(5));
        let masters = Masters::new(dir.join("ssh"));
        masters.prepare().await.unwrap();
        let hub = Hub::new(
            HubConfig::new(Hello::new("test", "hub", "hub")),
            Arc::new(Parked),
            None,
        );
        let local = HostEntry {
            id: HostId::new("local"),
            name: "local".into(),
            kind: HostKind::Local,
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        let (_learned_tx, learned_rx) = mpsc::channel(8);
        let registry = Registry::start(
            RegistryParts {
                store: store.clone(),
                hub,
                fleet: fleet.clone(),
                entries: Arc::new(std::sync::RwLock::new(HashMap::new())),
                local,
                prompts,
                masters: masters.clone(),
            },
            learned_rx,
        )
        .await
        .unwrap();
        Harness {
            registry,
            masters,
            fleet,
            store,
            dir,
        }
    }

    async fn add_ssh(h: &Harness, id: &str) -> HostId {
        let entry = h
            .registry
            .add(HostDraft {
                name: id.into(),
                kind: HostKind::Ssh {
                    target: "u@127.0.0.1".into(),
                    port: None,
                    identity_file: None,
                    options: Vec::new(),
                },
                id: Some(HostId::new(id)),
            })
            .await
            .unwrap();
        entry.id
    }

    // A Unix socket stands in for a master (shared connections are Unix-only).
    #[cfg(unix)]
    /// A control socket that accepts and drops every connection: `ssh -O exit` on it fails
    /// (the master answers nothing), so the connection is still open as far as the hub knows.
    fn wedged_master(h: &Harness, id: &HostId) -> tokio::task::JoinHandle<()> {
        let entry = h.registry.get(id).unwrap();
        let control = h.masters.path(&entry).unwrap();
        let listener = tokio::net::UnixListener::bind(&control).expect("bind the test socket");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        })
    }

    /// A sign-in that ended with the connection open: the master is tracked, as a real one is.
    #[cfg(unix)]
    async fn signed_in_master(h: &Harness, id: &HostId) {
        let entry = h.registry.get(id).unwrap();
        let control = h.masters.path(&entry).unwrap();
        let started = h.masters.begin_signin(&entry, &control).await;
        assert!(h.masters.signed_in(id, started));
    }

    /// A WSL distribution is looked up when a host is added and when its distribution changes, and
    /// a failed lookup changes nothing. Off Windows WSL is unavailable, so every lookup fails
    /// (on Windows the test would depend on the machine's distributions, so it does not run).
    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_wsl_distribution_is_looked_up_when_it_is_chosen_or_changed() {
        let h = harness("wsl-lookup").await;
        let wsl = HostKind::Wsl {
            distro: "Ubuntu".into(),
        };
        let added = h
            .registry
            .add(HostDraft {
                name: "wsl".into(),
                kind: wsl.clone(),
                id: Some(HostId::new("wsl")),
            })
            .await;
        assert!(
            added.is_err(),
            "a host is not added on an unavailable distribution"
        );
        assert!(h.registry.get(&HostId::new("wsl")).is_none());

        let id = add_ssh(&h, "box").await;
        let mut entry = h.registry.get(&id).unwrap();
        entry.kind = wsl;
        assert!(h.registry.update(entry).await.is_err());
        let kept = h.registry.get(&id).unwrap();
        assert!(
            matches!(kept.kind, HostKind::Ssh { .. }),
            "a refused update keeps the host as it was"
        );
    }

    async fn fleet_host(fleet: &Fleet, id: &HostId) -> FleetHost {
        fleet
            .snapshot()
            .await
            .into_iter()
            .find(|h| h.host.id == *id)
            .expect("the fleet knows the host")
    }

    async fn stored_host(store: &Store, id: &HostId) -> HostEntry {
        store
            .hosts()
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.id == *id)
            .expect("the store has the host")
    }

    // A Unix socket stands in for a master (shared connections are Unix-only).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_signout_whose_exit_failed_says_so_and_keeps_the_master_tracked() {
        let h = harness("signout-failed").await;
        let id = add_ssh(&h, "box").await;
        signed_in_master(&h, &id).await;
        let listener = wedged_master(&h, &id);

        let err = h
            .registry
            .signout(&id)
            .await
            .expect_err("a connection that could not be ended is not a sign-out");
        listener.abort();
        assert!(
            err.to_string().contains("could not be ended"),
            "the failure is reported: {err}"
        );
        assert!(
            h.masters.tracked(&id),
            "the connection is still open, so it stays tracked"
        );
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    // A Unix socket stands in for a master (shared connections are Unix-only).
    #[cfg(unix)]
    #[tokio::test]
    async fn removing_a_host_whose_exit_failed_keeps_its_master_tracked() {
        let h = harness("remove-failed").await;
        let id = add_ssh(&h, "box").await;
        signed_in_master(&h, &id).await;
        let listener = wedged_master(&h, &id);

        h.registry.remove(&id).await.unwrap();
        listener.abort();
        assert!(
            h.masters.tracked(&id),
            "the connection did not end, so nothing forgets it"
        );
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn an_edit_and_a_signout_at_once_keep_both_changes() {
        let h = harness("edit-signout").await;
        let id = add_ssh(&h, "box").await;
        let mut edited = h.registry.get(&id).unwrap();
        edited.name = "renamed".into();

        // Both run concurrently: the edit must not be overwritten by the sign-out's copy of the
        // entry, nor the sign-out by the edit's.
        let (updated, signed_out) =
            tokio::join!(h.registry.update(edited), h.registry.signout(&id));
        updated.unwrap();
        signed_out.unwrap();

        let stored = stored_host(&h.store, &id).await;
        assert_eq!(stored.name, "renamed", "the edit is kept");
        assert!(stored.signed_out, "the sign-out is kept");
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn the_fleet_and_the_store_see_a_sign_out_and_its_reconnect() {
        let h = harness("fleet-signed-out").await;
        let id = add_ssh(&h, "box").await;

        h.registry.signout(&id).await.unwrap();
        assert!(fleet_host(&h.fleet, &id).await.host.signed_out);

        h.registry.reconnect(&id).await.unwrap();
        // The write is awaited: the store has the cleared flag when reconnect returns.
        assert!(!stored_host(&h.store, &id).await.signed_out);
        assert!(
            !fleet_host(&h.fleet, &id).await.host.signed_out,
            "the fleet is told the sign-out is cleared"
        );
        let _ = std::fs::remove_dir_all(&h.dir);
    }
}
