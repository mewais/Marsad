//! Where secret variable values are kept: the operating system's credential store (Windows
//! Credential Manager, macOS Keychain, the freedesktop Secret Service on Linux) when it works,
//! else `secrets.json` in the store directory (owner-only; headless Linux, CI). The choice is
//! made once at [`Store::open`](crate::Store::open) by a write/read/delete round trip with a
//! deadline, and reported ([`SecretsLocation`]) so the UI can say where secrets are; nothing
//! falls back silently.
//!
//! In the system store every value is one entry: service `Marsad`, account
//! `<store id>:<host>:<variable>` (the store id tells apart the stores of several hubs of one
//! user). Which entries exist is known from the host settings (their secret variables), so
//! stale entries are deleted when a variable or host goes away. Values found in `secrets.json`
//! while the system store works are moved there once, and the file is removed.
//!
//! Every call into the system store is blocking; it runs on the blocking pool with a deadline.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use marsad_proto::hosts::SecretsLocation;
use marsad_proto::HostId;

use crate::diag;
use crate::{SecretValue, StoreError};

/// Secret values by host and variable name.
pub type Secrets = BTreeMap<HostId, BTreeMap<String, SecretValue>>;

/// The keyring service name every entry is filed under.
const SERVICE: &str = "Marsad";
/// Bound on one batch of system store calls (a locked keyring may wait for a prompt).
const DEADLINE: Duration = Duration::from_secs(10);
/// Bound on the probe at open (a system store that cannot answer quickly is not used).
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// The system store's name on this platform (`None`: there is none).
fn system_name() -> Option<&'static str> {
    if cfg!(windows) {
        Some("Windows Credential Manager")
    } else if cfg!(target_os = "macos") {
        Some("macOS Keychain")
    } else if cfg!(target_os = "linux") {
        Some("Secret Service")
    } else {
        None
    }
}

/// See the module docs.
pub(crate) enum Backend {
    File(PathBuf),
    System {
        /// `<store id>`, the account prefix.
        id: String,
        name: &'static str,
        /// Entries written so far (deleted when no longer wanted).
        known: BTreeSet<(HostId, String)>,
    },
}

/// A short stable id for the store directory (FNV-1a of its path, 8 hex digits).
fn store_id(dir: &Path) -> String {
    let p = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut h: u32 = 0x811c_9dc5;
    for b in p.to_string_lossy().bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

fn account(id: &str, host: &HostId, name: &str) -> String {
    format!("{id}:{host}:{name}")
}

fn sys_err(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Io {
        path: what.to_string(),
        message: e.to_string(),
    }
}

/// Run blocking system store calls with a deadline, on a thread of their own (not the blocking
/// pool: a call stuck on a password prompt must not keep the runtime from shutting down). The
/// thread enters this runtime, which the Secret Service client runs on.
async fn blocking<R: Send + 'static>(
    deadline: Duration,
    what: &str,
    f: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, StoreError> {
    let handle = tokio::runtime::Handle::current();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("marsad-secrets".into())
        .spawn(move || {
            let _rt = handle.enter();
            // The caller may have given up waiting (deadline).
            let _ = tx.send(f());
        })
        .map_err(|e| sys_err(what, e))?;
    match tokio::time::timeout(deadline, rx).await {
        Err(_) => Err(sys_err(
            what,
            format!(
                "did not answer within {} s (locked, waiting for a password?)",
                deadline.as_secs()
            ),
        )),
        Ok(Err(_)) => Err(sys_err(what, "the call ended without an answer")),
        Ok(Ok(r)) => r.map_err(|e| sys_err(what, e)),
    }
}

/// Linux: a Secret Service must already run on the session bus (never activated by us).
#[cfg(target_os = "linux")]
async fn service_running() -> Result<(), String> {
    let ask = async {
        let c = zbus::Connection::session()
            .await
            .map_err(|e| format!("no session bus ({e})"))?;
        let bus = zbus::fdo::DBusProxy::new(&c)
            .await
            .map_err(|e| e.to_string())?;
        let name =
            zbus::names::BusName::try_from("org.freedesktop.secrets").map_err(|e| e.to_string())?;
        match bus.name_has_owner(name).await {
            Ok(true) => Ok(()),
            Ok(false) => Err("none is running on the session bus (GNOME Keyring, KWallet)".into()),
            Err(e) => Err(e.to_string()),
        }
    };
    tokio::time::timeout(PROBE_DEADLINE, ask)
        .await
        .unwrap_or_else(|_| Err("the session bus did not answer".into()))
}

#[cfg(not(target_os = "linux"))]
async fn service_running() -> Result<(), String> {
    Ok(())
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod system {
    use keyring::{Entry, Error};

    fn entry(account: &str) -> Result<Entry, String> {
        Entry::new(super::SERVICE, account).map_err(|e| e.to_string())
    }

    pub fn get(account: &str) -> Result<Option<String>, String> {
        match entry(account)?.get_secret() {
            Ok(b) => String::from_utf8(b)
                .map(Some)
                .map_err(|_| "a stored value is not UTF-8".to_string()),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(account: &str, value: &str) -> Result<(), String> {
        entry(account)?
            .set_secret(value.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn delete(account: &str) -> Result<(), String> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod system {
    const NONE: &str = "no system credential store on this platform";
    pub fn get(_: &str) -> Result<Option<String>, String> {
        Err(NONE.into())
    }
    pub fn set(_: &str, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    pub fn delete(_: &str) -> Result<(), String> {
        Err(NONE.into())
    }
}

/// Write, read back and delete a probe entry.
async fn probe(id: &str, name: &str) -> Result<(), StoreError> {
    let acc = format!("{id}:probe:{}", fastrand::u64(..));
    blocking(PROBE_DEADLINE, name, move || {
        system::set(&acc, "probe")?;
        let back = system::get(&acc);
        let gone = system::delete(&acc);
        match back? {
            Some(v) if v == "probe" => gone,
            _ => Err("a value written could not be read back".into()),
        }
    })
    .await
}

impl Backend {
    /// Choose the backend for the store in `dir` (see the module docs), load the secrets of
    /// `wanted` (host → secret variable names) and move any values from `secrets.json` into
    /// the system store.
    pub(crate) async fn open(
        dir: &Path,
        wanted: BTreeMap<HostId, Vec<String>>,
    ) -> Result<(Backend, Secrets, SecretsLocation), StoreError> {
        let file = dir.join(crate::Doc::Secrets.file_name());
        let file_secrets: Secrets = crate::load(&file).await?;
        let Some(name) = system_name() else {
            return Ok(file_backend(
                file,
                file_secrets,
                "this platform has no system credential store".into(),
            ));
        };
        let id = store_id(dir);
        if let Err(e) = service_running().await {
            diag::secrets_service_unavailable(name, &e);
            return Ok(file_backend(
                file,
                file_secrets,
                format!("the {name} cannot be used: {e}"),
            ));
        }
        if let Err(e) = probe(&id, name).await {
            diag::secrets_probe_failed(name, &e);
            return Ok(file_backend(
                file,
                file_secrets,
                format!("the {name} cannot be used: {}", plain(&e)),
            ));
        }
        let mut backend = Backend::System {
            id: id.clone(),
            name,
            known: BTreeSet::new(),
        };
        // Load what the settings refer to.
        let pairs: Vec<(HostId, String)> = wanted
            .into_iter()
            .flat_map(|(h, names)| names.into_iter().map(move |n| (h.clone(), n)))
            .collect();
        let loaded = {
            let id = id.clone();
            let pairs = pairs.clone();
            blocking(DEADLINE, name, move || {
                let mut out = Vec::new();
                for (h, n) in pairs {
                    if let Some(v) = system::get(&account(&id, &h, &n))? {
                        out.push((h, n, v));
                    }
                }
                Ok(out)
            })
            .await?
        };
        let mut secrets = Secrets::new();
        for (h, n, v) in loaded {
            secrets.entry(h).or_default().insert(n, SecretValue(v));
        }
        if let Backend::System { known, .. } = &mut backend {
            *known = pairs.into_iter().collect();
        }
        // Values still in the file (an earlier build, or a time the system store was not
        // available) move to the system store once.
        if !file_secrets.is_empty() {
            for (h, vars) in file_secrets {
                for (n, v) in vars {
                    secrets.entry(h.clone()).or_default().insert(n, v);
                }
            }
            backend.persist(&secrets).await?;
            tokio::fs::remove_file(&file)
                .await
                .map_err(|e| crate::io_err(&file, e))?;
            diag::secrets_moved_from_file(name);
        }
        Ok((
            backend,
            secrets,
            SecretsLocation::System {
                store: name.to_string(),
            },
        ))
    }

    /// Make the backend hold exactly `secrets`.
    pub(crate) async fn persist(&mut self, secrets: &Secrets) -> Result<(), StoreError> {
        match self {
            Backend::File(path) => crate::save(path, secrets).await,
            Backend::System { id, name, known } => {
                let now: BTreeSet<(HostId, String)> = secrets
                    .iter()
                    .flat_map(|(h, v)| v.keys().map(move |n| (h.clone(), n.clone())))
                    .collect();
                let gone: Vec<String> = known
                    .difference(&now)
                    .map(|(h, n)| account(id, h, n))
                    .collect();
                let mut set: Vec<(String, String)> = Vec::new();
                for (h, vars) in secrets {
                    for (n, s) in vars {
                        set.push((account(id, h, n), s.0.clone()));
                    }
                }
                blocking(DEADLINE, name, move || {
                    for (a, v) in &set {
                        system::set(a, v)?;
                    }
                    for a in &gone {
                        system::delete(a)?;
                    }
                    Ok(())
                })
                .await?;
                *known = now;
                Ok(())
            }
        }
    }
}

fn file_backend(
    path: PathBuf,
    secrets: Secrets,
    reason: String,
) -> (Backend, Secrets, SecretsLocation) {
    let location = SecretsLocation::File {
        path: path.display().to_string(),
        reason,
    };
    (Backend::File(path), secrets, location)
}

/// The message of a system store error without the store name prefix.
fn plain(e: &StoreError) -> String {
    match e {
        StoreError::Io { message, .. } => message.clone(),
        other => other.to_string(),
    }
}
