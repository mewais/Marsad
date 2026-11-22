//! Secret variable values: kept where [`Store::secrets_location`] says — the system credential
//! store (Windows Credential Manager and macOS Keychain on those CI runners, a real Secret
//! Service under `dbus-run-session` + gnome-keyring on Linux CI) or the owner-only
//! `secrets.json` (no Secret Service: the reason is reported). Values survive reopening, a
//! removed variable or host is deleted from the store, and values found in `secrets.json`
//! move to the system store once.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use marsad_proto::hosts::{HostEnvSettings, HostVar, SecretsLocation};
use marsad_proto::HostId;
use marsad_store::Store;

fn tmp() -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "marsad-secrets-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn var(name: &str, value: Option<&str>, secret: bool) -> HostVar {
    HostVar {
        name: name.into(),
        value: value.map(str::to_string),
        secret,
    }
}

fn settings(host: &str, vars: Vec<HostVar>) -> HostEnvSettings {
    HostEnvSettings {
        host: HostId::new(host),
        setup: String::new(),
        vars,
        commands: BTreeMap::new(),
        revision: String::new(),
    }
}

async fn secret_of(store: &Store, host: &str, name: &str) -> Option<String> {
    let (_, s) = store
        .host_env_with_secrets(HostId::new(host))
        .await
        .unwrap();
    s.get(name).map(|v| v.0.clone())
}

/// Every file under `dir` whose bytes contain `needle`.
fn files_with(dir: &std::path::Path, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if let Ok(b) = std::fs::read(e.path()) {
            if b.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                out.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// What the other tests check, wherever the secrets are.
async fn round_trip(dir: &std::path::Path) -> SecretsLocation {
    let store = Store::open(dir).await.unwrap();
    let location = store.secrets_location().clone();
    eprintln!("secrets location: {location:?}");
    store
        .set_host_env(settings(
            "box",
            vec![
                var("PLAIN", Some("plain"), false),
                var("TOKEN", Some("tok-3f9a-secret"), true),
                var("OTHER", Some("other-81c2-secret"), true),
            ],
        ))
        .await
        .unwrap();
    store
        .set_host_env(settings(
            "pc",
            vec![var("PC_KEY", Some("pc-77d0-secret"), true)],
        ))
        .await
        .unwrap();
    match &location {
        SecretsLocation::File { path, reason } => {
            assert!(path.ends_with("secrets.json"), "{path}");
            assert!(!reason.is_empty());
            assert_eq!(files_with(dir, "tok-3f9a-secret"), ["secrets.json"]);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600);
            }
        }
        SecretsLocation::System { store: name } => {
            assert!(!name.is_empty());
            assert!(
                files_with(dir, "tok-3f9a-secret").is_empty(),
                "a secret value is in a store file"
            );
            assert!(!dir.join("secrets.json").exists());
        }
    }
    store.close().await.unwrap();

    // Reopened: the values are back; a removed secret variable and a removed host are gone.
    let store = Store::open(dir).await.unwrap();
    assert_eq!(store.secrets_location(), &location);
    assert_eq!(
        secret_of(&store, "box", "TOKEN").await.as_deref(),
        Some("tok-3f9a-secret")
    );
    assert_eq!(
        secret_of(&store, "pc", "PC_KEY").await.as_deref(),
        Some("pc-77d0-secret")
    );
    // Saving without a value keeps a secret; a variable left out is deleted.
    store
        .set_host_env(settings("box", vec![var("TOKEN", None, true)]))
        .await
        .unwrap();
    store
        .set_host_env(settings("pc", Vec::new()))
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(dir).await.unwrap();
    assert_eq!(
        secret_of(&store, "box", "TOKEN").await.as_deref(),
        Some("tok-3f9a-secret")
    );
    assert_eq!(secret_of(&store, "box", "OTHER").await, None);
    assert_eq!(secret_of(&store, "pc", "PC_KEY").await, None);
    // Clean up the system store's entries.
    store
        .set_host_env(settings("box", Vec::new()))
        .await
        .unwrap();
    store.close().await.unwrap();
    location
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_are_where_the_store_says() {
    let dir = tmp();
    let location = round_trip(&dir).await;
    // Without a Secret Service (headless Linux), the file is used and the reason given.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        match &location {
            SecretsLocation::File { reason, .. } => {
                assert!(reason.contains("Secret Service"), "{reason}")
            }
            other => panic!("no session bus, yet {other:?}"),
        }
    }
    // Windows and macOS always have their store.
    if cfg!(any(windows, target_os = "macos")) {
        assert!(
            matches!(location, SecretsLocation::System { .. }),
            "{location:?}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Values an earlier build left in `secrets.json` move to the system store once (where there
/// is one); without one the file stays in use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_json_moves_to_the_system_store_once() {
    let dir = tmp();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("host-environments.json"),
        serde_json::json!({"version": 1, "data": [
            {"host": "old", "setup": "", "vars": [{"name": "LEGACY", "secret": true}],
             "commands": {}, "revision": "r1"}
        ]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("secrets.json"),
        serde_json::json!({"version": 1, "data": {"old": {"LEGACY": "legacy-5e1b-secret"}}})
            .to_string(),
    )
    .unwrap();
    let store = Store::open(&dir).await.unwrap();
    let location = store.secrets_location().clone();
    assert_eq!(
        secret_of(&store, "old", "LEGACY").await.as_deref(),
        Some("legacy-5e1b-secret")
    );
    let moved = matches!(location, SecretsLocation::System { .. });
    assert_eq!(dir.join("secrets.json").exists(), !moved, "{location:?}");
    store.close().await.unwrap();
    let store = Store::open(&dir).await.unwrap();
    assert_eq!(
        secret_of(&store, "old", "LEGACY").await.as_deref(),
        Some("legacy-5e1b-secret")
    );
    store
        .set_host_env(settings("old", Vec::new()))
        .await
        .unwrap();
    store.close().await.unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Linux with a real Secret Service (CI: `dbus-run-session` + an unlocked gnome-keyring).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a Secret Service on the session bus (CI runs it under dbus-run-session with gnome-keyring)"]
async fn secret_service_is_used_on_linux_when_running() {
    let dir = tmp();
    let location = round_trip(&dir).await;
    assert_eq!(
        location,
        SecretsLocation::System {
            store: "Secret Service".into()
        }
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
