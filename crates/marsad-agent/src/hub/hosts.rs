//! The hub's host registry: persisted host entries (marsad-store), one supervised link per
//! remote host (wasit-control `Hub`), each host's session feed into the fleet, and platform
//! updates found by provisioning.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use marsad_proto::fleet::FleetLink;
use marsad_proto::hosts::{HostDraft, HostEntry, HostKind, ImportSuggestions};
use marsad_proto::{HostId, LinkState, MarsadError};
use marsad_store::Store;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wasit_control::{HostSpec, Hub};

use super::auth::Prompts;
use super::fleet::Fleet;
use super::provision::{Entries, Learned};
use super::transport::bootstrap_argv;
use crate::util::now_ms;

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
    feeds: Mutex<HashMap<HostId, JoinHandle<()>>>,
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

impl Registry {
    /// Load the stored hosts, start their links and feeds, and consume platform updates.
    pub async fn start(
        store: Store,
        hub: Hub,
        fleet: Fleet,
        entries: Entries,
        local: HostEntry,
        prompts: Prompts,
        mut learned: mpsc::Receiver<Learned>,
    ) -> Result<Registry, MarsadError> {
        let r = Registry {
            inner: Arc::new(Inner {
                store,
                hub,
                fleet,
                entries,
                local,
                prompts,
                feeds: Mutex::new(HashMap::new()),
            }),
        };
        r.inner
            .fleet
            .host_upserted(r.inner.local.clone(), FleetLink::Local)
            .await;
        for h in r.inner.store.hosts().await? {
            if let Err(e) = r.link(h.clone()).await {
                tracing::error!(host = %h.id, error = %e, "cannot start the link");
            }
        }
        let r2 = r.clone();
        tokio::spawn(async move {
            while let Some(l) = learned.recv().await {
                if let Err(e) = r2.learned(l).await {
                    tracing::error!(error = %e, "cannot record what a connect attempt learned");
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
        let argv = bootstrap_argv(&entry).ok_or_else(|| {
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
                    state: LinkState::Connecting,
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
            check_wsl_distro(distro).await?;
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
        };
        let mut entry = entry;
        entry.derive_local_files();
        self.inner.store.put_host(entry.clone()).await?;
        self.link(entry.clone()).await?;
        Ok(entry)
    }

    pub async fn update(&self, entry: HostEntry) -> Result<HostEntry, MarsadError> {
        if let HostKind::Wsl { distro } = &entry.kind {
            check_wsl_distro(distro).await?;
        }
        let Some(old) = self.get(&entry.id) else {
            return Err(MarsadError::NotFound {
                what: format!("host {}", entry.id),
            });
        };
        if entry.id == self.inner.local.id || matches!(entry.kind, HostKind::Local) {
            return Err(MarsadError::invalid("the local host cannot be changed"));
        }
        let mut entry = entry;
        entry.derive_local_files();
        entry.created_at_ms = old.created_at_ms;
        if entry.kind != old.kind {
            // A different machine (or route to it): what we knew about it no longer holds.
            entry.platform = None;
            entry.interactive_auth = false;
        } else {
            entry.platform = old.platform.clone();
            entry.interactive_auth = old.interactive_auth;
        }
        self.inner.store.put_host(entry.clone()).await?;
        if entry.kind != old.kind {
            self.unlink(&entry.id).await;
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
                .unwrap_or(LinkState::Connecting);
            self.inner
                .fleet
                .host_upserted(entry.clone(), FleetLink::Remote { state })
                .await;
        }
        Ok(entry)
    }

    pub async fn remove(&self, id: &HostId) -> Result<(), MarsadError> {
        if *id == self.inner.local.id {
            return Err(MarsadError::invalid("the local host cannot be removed"));
        }
        self.inner.store.remove_host(id).await?;
        self.unlink(id).await;
        self.inner
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.inner.fleet.host_removed(id.clone()).await;
        Ok(())
    }

    /// Connect now; for a host that signs in interactively this is the user's explicit
    /// "Connect" (one attempt is allowed).
    pub fn reconnect(&self, id: &HostId) -> Result<(), MarsadError> {
        self.inner.prompts.arm(id);
        if self.inner.hub.reconnect_now(id) {
            Ok(())
        } else {
            Err(MarsadError::NotFound {
                what: format!("link to {id}"),
            })
        }
    }

    async fn learned(&self, l: Learned) -> Result<(), MarsadError> {
        let id = match &l {
            Learned::Platform { host, .. } | Learned::Interactive { host } => host.clone(),
        };
        let Some(mut entry) = self.get(&id) else {
            return Ok(());
        };
        match l {
            Learned::Platform { platform, .. } => entry.platform = Some(platform),
            Learned::Interactive { .. } => entry.interactive_auth = true,
        }
        self.inner.store.put_host(entry.clone()).await?;
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
            .unwrap_or(LinkState::Connecting);
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

/// A WSL host needs a Windows hub with WSL and a distribution of that name (when the list can
/// be read; a list that cannot be read leaves the check to the connection).
async fn check_wsl_distro(distro: &str) -> Result<(), MarsadError> {
    if distro.trim().is_empty() {
        return Err(MarsadError::invalid("pick a WSL distribution"));
    }
    let list = super::wsl::distros().await;
    if !list.available {
        return Err(MarsadError::invalid(
            list.problem
                .unwrap_or_else(|| "WSL is not available here".into()),
        ));
    }
    if !list.distros.is_empty() && !list.distros.iter().any(|d| d.name == distro) {
        let names: Vec<&str> = list.distros.iter().map(|d| d.name.as_str()).collect();
        return Err(MarsadError::invalid(format!(
            "WSL has no distribution named {distro:?}; installed: {}",
            names.join(", ")
        )));
    }
    Ok(())
}
