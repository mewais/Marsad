//! Per-host environment settings (setup commands, variables, agent commands): stored by the hub
//! (secret values only in the store's `secrets.json`), and applied on each host by its daemon's
//! environment stage (`host.environment.configure`, Wasit ARCHITECTURE.md §3.5), which keeps
//! them in memory.
//!
//! Secret values are kept by the store in the system credential store, or in its owner-only
//! `secrets.json` where there is none ([`marsad_store::secrets`]; `hosts.environment.secrets`
//! says which).
//!
//! A host gets its settings when they are saved, whenever its link (re)connects (a restarted
//! daemon has none), and before every launch on it ([`HostEnvs::ensure`]: a no-op when the
//! host already reports the stored revision). Secret values are sent only in those requests;
//! they never go back to the UI, into a log line or into an error; every secret value the hub
//! loads or is given is registered with the diagnostics scrubber first, so that a text that
//! echoes one is masked in records.
//!
//! Every apply is recorded (`hostenv.apply`: the stored revision, applied / noop / failed / not
//! connected, how long it took, the code of the error). What the user can see of an apply that
//! failed after a connect is only that the host reports another revision than the stored one:
//! the failure itself is returned only to a save (`HostEnvSaved::not_applied`), a launch or a
//! test that asked for it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use marsad_proto::diag::{ApplyOutcome, MarsadEvent};
use marsad_proto::hosts::{HostEnvSaved, HostEnvSettings, HostVar};
use marsad_proto::{HostId, LinkState, MarsadError};
use marsad_store::{SecretValue, Store};
use wasit_control::{
    CallContext, CallError, Hub, LinkEvent, RequestOpts, RpcError, Service, Supervised,
};
use wasit_core::DriverRegistry;
use wasit_diag::Ident;
use wasit_runtime::wire::{
    methods as rt, ConfigureEnvironment, EnvVar, EnvironmentTest, HostEnvConfig, HostEnvironment,
};

use crate::diag::{self, emit};

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
        // Every transition (a `watch` of the states would skip a quick reconnect, leaving a
        // restarted daemon without its settings). Subscribed before the states are read, so
        // a host connecting in between is seen at least once.
        let mut events = hub.events();
        let states = hub.states();
        let this = envs.clone();
        tokio::spawn(async move {
            let connected = |states: &tokio::sync::watch::Receiver<_>| -> Vec<HostId> {
                let m: &BTreeMap<HostId, LinkState> = &states.borrow();
                m.iter()
                    .filter(|(_, s)| s.is_connected())
                    .map(|(h, _)| h.clone())
                    .collect()
            };
            for h in connected(&states) {
                this.ensure_in_background(h);
            }
            loop {
                match events.recv().await {
                    Ok(LinkEvent {
                        host,
                        state: Some(LinkState::Connected { .. }),
                    }) => this.ensure_in_background(host),
                    Ok(_) => {}
                    // Missed transitions: any connected host may have reconnected (an ensure
                    // of a host that runs the stored revision is one report request).
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        for h in connected(&states) {
                            this.ensure_in_background(h);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        envs
    }

    fn ensure_in_background(&self, host: HostId) {
        let this = self.clone();
        tokio::spawn(async move {
            // A failure is recorded by the apply itself (`hostenv.apply`); the next connect,
            // launch or terminal tries again.
            let _recorded = this.ensure(&host).await;
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
        // Before anything else sees the values (a validation error, an ssh message).
        register_secrets(
            settings
                .vars
                .iter()
                .filter(|v| v.secret)
                .filter_map(|v| v.value.as_deref()),
        );
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
        // Held from before the settings are stored until they are applied, so a launch or
        // terminal asked for meanwhile waits for them ([`EnvGated`]).
        let lock = self.lock_for(&host);
        let _g = lock.lock().await;
        let stored = self.inner.store.set_host_env(settings).await?;
        let (environment, not_applied) = match self.ensure_locked(&host).await {
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
        self.ensure_locked(host).await
    }

    /// [`HostEnvs::ensure`] with the host's lock held by the caller.
    async fn ensure_locked(&self, host: &HostId) -> Result<HostEnvironment, MarsadError> {
        let began = Instant::now();
        let (revision, result) = self.apply_stored(host).await;
        record(host, revision.as_deref(), began, &result);
        result.map(|(env, _)| env)
    }

    /// The stored revision (when it could be read) and what applying it came to.
    async fn apply_stored(
        &self,
        host: &HostId,
    ) -> (
        Option<String>,
        Result<(HostEnvironment, ApplyOutcome), MarsadError>,
    ) {
        let r = match self.configuration(host).await {
            Ok(r) => r,
            Err(e) => return (None, Err(e)),
        };
        let revision = Some(r.revision.clone());
        let current: Option<HostEnvironment> = match self
            .call(host, rt::HOST_ENVIRONMENT, (), REPORT_DEADLINE)
            .await
        {
            Ok(c) => c,
            Err(e) => return (revision, Err(e)),
        };
        if let Some(env) = current.filter(|e| e.revision.as_deref() == Some(&r.revision)) {
            return (revision, Ok((env, ApplyOutcome::Noop)));
        }
        let applied = self
            .call(host, rt::HOST_ENVIRONMENT_CONFIGURE, &r, APPLY_DEADLINE)
            .await
            .map(|env| (env, ApplyOutcome::Applied));
        (revision, applied)
    }

    /// Apply the stored settings again (the setup re-runs) and report what the host then
    /// finds.
    pub async fn test(&self, host: &HostId) -> Result<EnvironmentTest, MarsadError> {
        let lock = self.lock_for(host);
        let _g = lock.lock().await;
        let began = Instant::now();
        let r = match self.configuration(host).await {
            Ok(r) => r,
            Err(e) => {
                record(host, None, began, &Err(e.clone()));
                return Err(e);
            }
        };
        let test: Result<EnvironmentTest, MarsadError> = self
            .call(host, rt::HOST_ENVIRONMENT_TEST, &r, TEST_DEADLINE)
            .await;
        let shown = test
            .as_ref()
            .map(|t| (t.environment.clone(), ApplyOutcome::Applied))
            .map_err(Clone::clone);
        record(host, Some(&r.revision), began, &shown);
        test
    }

    /// What the host's environment stage is given: the settings with their secret values.
    async fn configuration(&self, host: &HostId) -> Result<ConfigureEnvironment, MarsadError> {
        let (settings, secrets) = self.inner.store.host_env_with_secrets(host.clone()).await?;
        register_secrets(secrets.values().map(|s| s.0.as_str()));
        configuration(settings, &secrets)
    }
}

/// Methods that start processes on a host (terminals, sessions created or continued without
/// `sessions.launch`, which ensures by itself): they first wait until the host runs with the
/// stored settings, including a save still being applied. Everything else is passed through.
pub const ENV_GATED: &[&str] = &[
    marsad_proto::methods::PTY_START,
    rt::SESSIONS_CREATE,
    rt::SESSIONS_CONTINUE,
];

/// Serves [`ENV_GATED`]: [`HostEnvs::ensure`] on the request's host, then the routed call.
pub struct EnvGated {
    pub envs: HostEnvs,
    pub inner: Arc<dyn Service>,
    /// The host a request without `host` is for.
    pub local: HostId,
}

#[async_trait::async_trait]
impl Service for EnvGated {
    async fn call(
        &self,
        ctx: CallContext,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        let host = ctx.host.clone().unwrap_or_else(|| self.local.clone());
        self.envs
            .ensure(&host)
            .await
            .map_err(MarsadError::into_rpc)?;
        self.inner.call(ctx, method, params).await
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

/// Registers secret values with the diagnostics scrubber (values shorter than the scrubber's
/// minimum are refused by it: replacing every occurrence of a two-letter value would destroy
/// every text without protecting anything).
fn register_secrets<'a>(values: impl IntoIterator<Item = &'a str>) {
    wasit_diag::scrub::register_secrets(values);
}

/// Registers every stored secret value of the local host and of every stored host. The hub
/// calls it when it starts, before any link does: an ssh message that echoes a secret is then
/// masked from the very first record.
pub async fn register_stored_secrets(store: &Store, local: &HostId) {
    let mut hosts = vec![local.clone()];
    if let Ok(entries) = store.hosts().await {
        hosts.extend(entries.into_iter().map(|h| h.id));
    }
    for host in hosts {
        if let Ok((_, secrets)) = store.host_env_with_secrets(host).await {
            register_secrets(secrets.values().map(|s| s.0.as_str()));
        }
    }
}

/// `hostenv.apply`: how applying `host`'s stored settings (revision `revision`) came out.
fn record(
    host: &HostId,
    revision: Option<&str>,
    began: Instant,
    result: &Result<(HostEnvironment, ApplyOutcome), MarsadError>,
) {
    let (outcome, setup_ok, error) = match result {
        Ok((env, outcome)) => (*outcome, env.setup.as_ref().map(|s| s.ok), None),
        Err(e) => {
            let (code, down) = match e {
                MarsadError::Upstream { cause, .. } => (
                    cause.code.clone(),
                    cause.code == wasit_control::codes::HOST_UNREACHABLE,
                ),
                _ => ("hostenv".to_owned(), false),
            };
            let outcome = if down {
                ApplyOutcome::NotConnected
            } else {
                ApplyOutcome::Failed
            };
            // Its text may quote a peer's words or what the user stored: code and length only.
            (outcome, None, Some(diag::foreign(&code, &e.to_string())))
        }
    };
    emit(MarsadEvent::HostenvApply {
        host: Ident::new(host.as_str()),
        // (A host without stored settings has the empty revision: nothing to name.)
        revision: revision.filter(|r| !r.is_empty()).map(Ident::new),
        outcome,
        ms: diag::ms(began),
        setup_ok,
        error,
    });
}

fn upstream(host: &HostId, method: &str, e: CallError) -> MarsadError {
    let cause = match e {
        CallError::Rpc(r) => r,
        // The link is down: the same code a routed request to a lost host gets.
        CallError::NotConnected => CallError::NotConnected.into_rpc(),
        other => wasit_control::RpcError::internal(other.to_string()),
    };
    MarsadError::Upstream {
        message: format!("{method} on {host}: {cause}"),
        cause,
    }
}
