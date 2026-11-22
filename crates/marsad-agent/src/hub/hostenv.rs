//! Per-host environment settings (setup commands, variables, agent commands): stored by the hub
//! (secret values only in the store's `secrets.json`), and applied on each host by its daemon's
//! environment stage (`host.environment.configure`, Wasit ARCHITECTURE.md §3.5), which keeps
//! them in memory.
//!
//! A host gets its settings when they are saved, whenever its link (re)connects (a restarted
//! daemon has none), and before every launch on it ([`HostEnvs::ensure`]: a no-op when the
//! host already reports the stored revision). Secret values are sent only in those requests;
//! they never go back to the UI, into a log line or into an error.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marsad_proto::hosts::{HostEnvSaved, HostEnvSettings, HostVar};
use marsad_proto::{HostId, LinkState, MarsadError};
use marsad_store::{SecretValue, Store};
use wasit_control::{CallError, Hub, RequestOpts, Supervised};
use wasit_core::DriverRegistry;
use wasit_runtime::wire::{
    methods as rt, ConfigureEnvironment, EnvVar, EnvironmentTest, HostEnvConfig, HostEnvironment,
};

/// Applying runs the setup commands (their own deadline is 30 s on the host).
const APPLY_DEADLINE: Duration = Duration::from_secs(60);
/// A test also waits for discovery (Wasit bounds it at 50 s after the setup).
const TEST_DEADLINE: Duration = Duration::from_secs(110);
/// Reading the host's report.
const REPORT_DEADLINE: Duration = Duration::from_secs(15);
/// How long a request waits for the hub's loopback link (in-process; connects at once).
const WAIT_CONNECTED: Duration = Duration::from_secs(10);

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct HostEnvs {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    /// The hub's loopback link: requests carry the target host.
    link: Supervised,
    drivers: DriverRegistry,
    /// One apply at a time per host (a save, a reconnect and a launch may meet).
    locks: Mutex<HashMap<HostId, Arc<tokio::sync::Mutex<()>>>>,
}

impl HostEnvs {
    /// Start: apply to the local host now and to every remote host whenever its link connects.
    pub fn start(
        store: Store,
        link: Supervised,
        drivers: DriverRegistry,
        hub: &Hub,
        local: HostId,
    ) -> HostEnvs {
        let envs = HostEnvs {
            inner: Arc::new(Inner {
                store,
                link,
                drivers,
                locks: Mutex::new(HashMap::new()),
            }),
        };
        envs.ensure_in_background(local);
        let mut states = hub.states();
        let this = envs.clone();
        tokio::spawn(async move {
            let mut connected: BTreeMap<HostId, bool> = BTreeMap::new();
            loop {
                let now: Vec<(HostId, bool)> = states
                    .borrow_and_update()
                    .iter()
                    .map(|(h, s)| (h.clone(), matches!(s, LinkState::Connected { .. })))
                    .collect();
                for (h, up) in now {
                    if up && !connected.get(&h).copied().unwrap_or(false) {
                        this.ensure_in_background(h.clone());
                    }
                    connected.insert(h, up);
                }
                if states.changed().await.is_err() {
                    return;
                }
            }
        });
        envs
    }

    fn ensure_in_background(&self, host: HostId) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.ensure(&host).await {
                // The host shows the settings as not applied; the next connect or launch
                // tries again.
                tracing::warn!(%host, error = %e, "cannot apply the host's environment settings");
            }
        });
    }

    fn lock_for(&self, host: &HostId) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.inner.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(host.clone()).or_default().clone()
    }

    async fn call<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        host: &HostId,
        method: &str,
        params: P,
        deadline: Duration,
    ) -> Result<R, MarsadError> {
        self.inner
            .link
            .request(
                method,
                params,
                RequestOpts::new(deadline).host(host.clone()),
                WAIT_CONNECTED,
            )
            .await
            .map_err(|e| upstream(host, method, e))
    }

    /// The stored settings, secret values left out.
    pub async fn get(&self, host: &HostId) -> Result<HostEnvSettings, MarsadError> {
        Ok(self.inner.store.host_env(host.clone()).await?)
    }

    /// Validate and store `settings`, then apply them on the host if it is reachable.
    pub async fn set(&self, settings: HostEnvSettings) -> Result<HostEnvSaved, MarsadError> {
        let mut settings = settings;
        settings.commands.retain(|_, c| !c.trim().is_empty());
        for d in settings.commands.keys() {
            self.inner
                .drivers
                .get(d)
                .map_err(|_| MarsadError::invalid(format!("unknown agent {d}")))?;
        }
        // Names and commands are checked the way the host checks them (values do not matter).
        let shape = HostEnvConfig {
            setup: settings.setup.clone(),
            vars: settings
                .vars
                .iter()
                .map(|v| EnvVar {
                    name: v.name.clone(),
                    value: String::new(),
                    secret: v.secret,
                })
                .collect(),
            commands: settings.commands.clone(),
        };
        shape.validate().map_err(MarsadError::invalid)?;
        let host = settings.host.clone();
        let stored = self.inner.store.set_host_env(settings).await?;
        let (environment, not_applied) = match self.ensure(&host).await {
            Ok(env) => (Some(env), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Ok(HostEnvSaved {
            settings: stored,
            environment,
            not_applied,
        })
    }

    /// Make sure `host` runs with the stored settings (applies them when its report names
    /// another revision). Returns the host's environment report.
    pub async fn ensure(&self, host: &HostId) -> Result<HostEnvironment, MarsadError> {
        let lock = self.lock_for(host);
        let _g = lock.lock().await;
        let r = self.configuration(host).await?;
        let current: Option<HostEnvironment> = self
            .call(host, rt::HOST_ENVIRONMENT, (), REPORT_DEADLINE)
            .await?;
        if let Some(env) = current.filter(|e| e.revision.as_deref() == Some(&r.revision)) {
            return Ok(env);
        }
        self.call(host, rt::HOST_ENVIRONMENT_CONFIGURE, &r, APPLY_DEADLINE)
            .await
    }

    /// Apply the stored settings again (the setup re-runs) and report what the host then
    /// finds.
    pub async fn test(&self, host: &HostId) -> Result<EnvironmentTest, MarsadError> {
        let lock = self.lock_for(host);
        let _g = lock.lock().await;
        let r = self.configuration(host).await?;
        self.call(host, rt::HOST_ENVIRONMENT_TEST, &r, TEST_DEADLINE)
            .await
    }

    /// What the host's environment stage is given: the settings with their secret values.
    async fn configuration(&self, host: &HostId) -> Result<ConfigureEnvironment, MarsadError> {
        let (settings, secrets) = self.inner.store.host_env_with_secrets(host.clone()).await?;
        configuration(settings, &secrets)
    }
}

fn configuration(
    settings: HostEnvSettings,
    secrets: &BTreeMap<String, SecretValue>,
) -> Result<ConfigureEnvironment, MarsadError> {
    let vars = settings
        .vars
        .into_iter()
        .map(
            |HostVar {
                 name,
                 value,
                 secret,
             }| {
                let value = if secret {
                    secrets.get(&name).map(|s| s.0.clone()).ok_or_else(|| {
                        MarsadError::invalid(format!(
                            "the secret {name} has no stored value; enter it again"
                        ))
                    })?
                } else {
                    value.unwrap_or_default()
                };
                Ok(EnvVar {
                    name,
                    value,
                    secret,
                })
            },
        )
        .collect::<Result<Vec<_>, MarsadError>>()?;
    Ok(ConfigureEnvironment {
        config: HostEnvConfig {
            setup: settings.setup,
            vars,
            commands: settings.commands,
        },
        revision: settings.revision,
    })
}

fn upstream(host: &HostId, method: &str, e: CallError) -> MarsadError {
    let cause = match e {
        CallError::Rpc(r) => r,
        other => wasit_control::RpcError::internal(other.to_string()),
    };
    MarsadError::Upstream {
        message: format!("{method} on {host}: {cause}"),
        cause,
    }
}
