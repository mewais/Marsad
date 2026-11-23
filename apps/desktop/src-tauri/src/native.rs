//! Native-only `desktop.*` methods of the `rpc` bridge: open a link or path with the OS,
//! reveal a path in the file manager, pick a folder, set the window background (`surface`),
//! pick files to attach and read them (only files the user picked), save downloads into
//! the Downloads folder (only files this shell created there), and save an exported
//! diagnostics file where the user says (`export_save`).

use serde::de::DeserializeOwned;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

use crate::bridge::{picked, BridgeError, OpenArgs, PickArgs};

fn args<T: DeserializeOwned>(p: Value) -> Result<T, BridgeError> {
    serde_json::from_value(p).map_err(|e| BridgeError::new("invalid_params", e.to_string()))
}

fn is_url(s: &str) -> bool {
    ["http://", "https://", "mailto:", "file://"]
        .iter()
        .any(|p| s.starts_with(p))
}

/// The path to hand to the OS for a target: for a WSL host the Windows form of its Linux path,
/// checked for reachability (a stopped distro and an unreadable file get distinct messages).
fn resolve_target(distro: Option<&str>, target: &str) -> Result<String, BridgeError> {
    match distro {
        None => Ok(target.to_string()),
        Some(d) => marsad_proto::paths::resolve_wsl(d, target)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| BridgeError::new("unreachable", e.to_string())),
    }
}

pub async fn call(app: &AppHandle, method: &str, p: Value) -> Result<Value, BridgeError> {
    let failed = |e: tauri_plugin_opener::Error| BridgeError::new("io", e.to_string());
    match method {
        "open" => {
            let a: OpenArgs = args(p)?;
            if is_url(&a.target) {
                app.opener()
                    .open_url(a.target, None::<&str>)
                    .map_err(failed)?;
            } else {
                let t = resolve_target(a.distro.as_deref(), &a.target)?;
                app.opener().open_path(t, None::<&str>).map_err(failed)?;
            }
            Ok(Value::Null)
        }
        "reveal" => {
            let a: OpenArgs = args(p)?;
            let t = resolve_target(a.distro.as_deref(), &a.target)?;
            app.opener().reveal_item_in_dir(t).map_err(failed)?;
            Ok(Value::Null)
        }
        "pick_folder" => {
            let a: PickArgs = if p.is_null() {
                PickArgs::default()
            } else {
                args(p)?
            };
            let mut d = app.dialog().file();
            if let Some(t) = a.title {
                d = d.set_title(t);
            }
            let distro = a.distro.clone();
            if let Some(dist) = &distro {
                // Root the picker in the distro (or the folder asked for, when Windows can see it).
                let start = a
                    .default_path
                    .as_deref()
                    .and_then(|p| marsad_proto::paths::resolve_wsl(dist, p).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .or_else(|| marsad_proto::paths::wsl_to_windows(dist, "/").ok());
                if let Some(s) = start {
                    d = d.set_directory(s);
                }
            } else if let Some(dir) = a.default_path {
                d = d.set_directory(dir);
            }
            let (tx, rx) = oneshot::channel();
            // The dialog answers on its own thread; nothing waits here but this future.
            d.pick_folder(move |f| {
                // The caller may have gone away.
                let _ = tx.send(f.map(|f| f.to_string()));
            });
            let path = rx
                .await
                .map_err(|_| BridgeError::new("io", "the folder picker went away"))?;
            // A folder chosen in a distro (or on a drive) comes back as the Linux path.
            let path = match (distro, path) {
                (Some(d), Some(p)) => Some(
                    marsad_proto::paths::windows_to_wsl(&d, &p)
                        .map_err(|e| BridgeError::new("unreachable", e.to_string()))?,
                ),
                (_, p) => p,
            };
            Ok(picked(path))
        }
        "pick_files" => {
            let a: PickArgs = if p.is_null() {
                PickArgs::default()
            } else {
                args(p)?
            };
            let mut d = app.dialog().file();
            if let Some(t) = a.title {
                d = d.set_title(t);
            }
            let (tx, rx) = oneshot::channel();
            d.pick_files(move |f| {
                // The caller may have gone away.
                let _ = tx.send(f.unwrap_or_default());
            });
            let picked = rx
                .await
                .map_err(|_| BridgeError::new("io", "the file picker went away"))?;
            let mut files = Vec::new();
            for f in picked {
                let Ok(path) = f.into_path() else { continue };
                let meta = tokio::fs::metadata(&path)
                    .await
                    .map_err(|e| BridgeError::new("io", e.to_string()))?;
                if !meta.is_file() {
                    continue;
                }
                files::allow_read(&path);
                files.push(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "size": meta.len(),
                }));
            }
            Ok(serde_json::json!({ "files": files }))
        }
        "read_picked" => files::read_picked(args(p)?).await,
        "download_begin" => {
            let a: files::DownloadBegin = args(p)?;
            let dir = tauri::Manager::path(app)
                .download_dir()
                .or_else(|_| tauri::Manager::path(app).temp_dir())
                .map_err(|e| BridgeError::new("io", e.to_string()))?;
            files::download_begin(&dir, &a.name).await
        }
        "download_chunk" => files::download_chunk(args(p)?).await,
        "export_save" => exports::save(app, args(p)?).await,
        "window_surface" => crate::surface::window_surface(app, args(p)?).await,
        other => Err(BridgeError::new(
            "method_not_found",
            format!("unknown method `desktop.{other}`"),
        )),
    }
}

/// `desktop.export_save`: copy an export the hub made to where the user chooses.
mod exports {
    use std::future::Future;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;
    use serde_json::{json, Value};
    use tauri::{AppHandle, Manager};
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    use crate::bridge::BridgeError;
    use crate::diag::ExportsDir;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SaveArgs {
        /// The export's file name in `<data>/diag/exports/` (the hub's `diagnostics.exports`
        /// topic names it).
        id: String,
    }

    fn io(e: impl std::fmt::Display) -> BridgeError {
        BridgeError::new("io", e.to_string())
    }

    /// `id` as a plain file name: letters, digits, `.`, `_` and `-`, starting with a letter or
    /// a digit, at most 200 bytes. No separator, no `..`, no drive or stream (`:`), no absolute
    /// path, no hidden file. The hub names an export with nothing else.
    fn file_name(id: &str) -> Result<&str, BridgeError> {
        let plain = !id.is_empty()
            && id.len() <= 200
            && id.starts_with(|c: char| c.is_ascii_alphanumeric())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if plain {
            Ok(id)
        } else {
            Err(BridgeError::new(
                "invalid_params",
                "an export is named by a plain file name",
            ))
        }
    }

    /// The export `id` in `exports` (`<data>/diag/exports`): a regular file there, nothing else
    /// (not a directory, not a link to somewhere else).
    pub async fn source(exports: &Path, id: &str) -> Result<PathBuf, BridgeError> {
        let path = exports.join(file_name(id)?);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(m) if m.file_type().is_file() => Ok(path),
            Ok(_) => Err(BridgeError::new("not_found", "no such export")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(BridgeError::new("not_found", "no such export"))
            }
            Err(e) => Err(io(e)),
        }
    }

    /// Saves export `id` of `exports` where `choose` says: `choose(default_dir, file_name)`
    /// shows the Save dialog (the one native call, kept out of here so this is testable) and
    /// answers the file the user picked, or `None` when they cancelled. Answers `{path}` of the
    /// copy, or `{saved: false}`.
    pub async fn save_with<F, Fut>(
        exports: &Path,
        id: &str,
        default_dir: Option<PathBuf>,
        choose: F,
    ) -> Result<Value, BridgeError>
    where
        F: FnOnce(Option<PathBuf>, String) -> Fut,
        Fut: Future<Output = Result<Option<PathBuf>, BridgeError>>,
    {
        let from = source(exports, id).await?;
        let Some(to) = choose(default_dir, id.to_owned()).await? else {
            return Ok(json!({ "saved": false }));
        };
        // The user may have picked the export itself: copying a file onto itself empties it.
        if tokio::fs::canonicalize(&to).await.ok() == tokio::fs::canonicalize(&from).await.ok()
            && tokio::fs::try_exists(&to).await.map_err(io)?
        {
            return Err(BridgeError::new(
                "invalid_params",
                "choose a place other than the export itself",
            ));
        }
        tokio::fs::copy(&from, &to).await.map_err(io)?;
        Ok(json!({ "path": to.to_string_lossy() }))
    }

    /// The Save dialog, defaulting to `dir` and `name`; `None` when the user cancels.
    async fn ask(
        app: &AppHandle,
        dir: Option<PathBuf>,
        name: String,
    ) -> Result<Option<PathBuf>, BridgeError> {
        let mut d = app.dialog().file().set_file_name(name);
        if let Some(dir) = dir {
            d = d.set_directory(dir);
        }
        let (tx, rx) = oneshot::channel();
        // The dialog answers on its own thread; nothing waits here but this future.
        d.save_file(move |f| {
            // The caller may have gone away.
            let _ = tx.send(f);
        });
        let picked = rx
            .await
            .map_err(|_| BridgeError::new("io", "the save dialog went away"))?;
        picked.map(|f| f.into_path().map_err(io)).transpose()
    }

    pub async fn save(app: &AppHandle, a: SaveArgs) -> Result<Value, BridgeError> {
        let exports = app
            .try_state::<ExportsDir>()
            .and_then(|d| d.0.clone())
            .ok_or_else(|| BridgeError::new("io", "this machine's data directory is unknown"))?;
        let downloads = app
            .path()
            .download_dir()
            .or_else(|_| app.path().temp_dir())
            .ok();
        save_with(&exports, &a.id, downloads, |dir, name| ask(app, dir, name)).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn scratch(name: &str) -> PathBuf {
            let dir =
                std::env::temp_dir().join(format!("marsad-export-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("exports")).unwrap();
            dir
        }

        #[tokio::test]
        async fn only_a_plain_file_name_names_an_export() {
            let dir = scratch("confine");
            let exports = dir.join("exports");
            std::fs::write(exports.join("ok-1.ndjson.gz"), b"x").unwrap();
            // A secret next to the exports directory, and one far from it.
            std::fs::write(dir.join("secret.txt"), b"s").unwrap();
            let outside = dir.join("secret.txt").to_string_lossy().into_owned();
            for id in [
                "",
                ".",
                "..",
                "../secret.txt",
                "..\\secret.txt",
                "sub/ok-1.ndjson.gz",
                "sub\\ok-1.ndjson.gz",
                "/etc/passwd",
                "C:\\Windows\\win.ini",
                "C:win.ini",
                "ok-1.ndjson.gz:stream",
                ".hidden",
                "ok 1",
                "ok-1.ndjson.gz\0",
                "ok\n",
                "é",
                &"a".repeat(201),
                outside.as_str(),
            ] {
                let r = source(&exports, id).await;
                assert_eq!(
                    r.as_ref().map_err(|e| e.code.as_str()),
                    Err("invalid_params"),
                    "{id:?} -> {r:?}"
                );
            }
            assert_eq!(
                source(&exports, "ok-1.ndjson.gz").await.unwrap(),
                exports.join("ok-1.ndjson.gz")
            );
            // Plain, but not there (or not a file): not found, never another path.
            for id in ["missing.ndjson.gz", "adir"] {
                std::fs::create_dir_all(exports.join("adir")).unwrap();
                assert_eq!(
                    source(&exports, id).await.unwrap_err().code,
                    "not_found",
                    "{id}"
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn a_link_out_of_the_exports_directory_is_not_an_export() {
            let dir = scratch("link");
            std::fs::write(dir.join("secret.txt"), b"s").unwrap();
            std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("exports").join("l.gz"))
                .unwrap();
            assert_eq!(
                source(&dir.join("exports"), "l.gz").await.unwrap_err().code,
                "not_found"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[tokio::test]
        async fn the_chosen_place_gets_a_copy_and_a_cancel_copies_nothing() {
            let dir = scratch("copy");
            let exports = dir.join("exports");
            std::fs::write(exports.join("e.ndjson.gz"), b"the export").unwrap();
            let dest = dir.join("saved.ndjson.gz");
            let target = dest.clone();
            let downloads = Some(dir.join("Downloads"));
            let seen = std::sync::Mutex::new(None);
            let r = save_with(&exports, "e.ndjson.gz", downloads.clone(), |d, n| {
                *seen.lock().unwrap() = Some((d, n));
                async move { Ok(Some(target)) }
            })
            .await
            .unwrap();
            // The dialog was offered the Downloads folder and the export's own name.
            assert_eq!(
                seen.into_inner().unwrap(),
                Some((downloads.clone(), "e.ndjson.gz".to_string()))
            );
            assert_eq!(r, json!({ "path": dest.to_string_lossy() }));
            assert_eq!(std::fs::read(&dest).unwrap(), b"the export");
            assert_eq!(
                std::fs::read(exports.join("e.ndjson.gz")).unwrap(),
                b"the export"
            );

            let none = dir.join("none.gz");
            let r = save_with(&exports, "e.ndjson.gz", downloads.clone(), |_, _| async {
                Ok(None)
            })
            .await
            .unwrap();
            assert_eq!(r, json!({ "saved": false }));
            assert!(!none.exists());

            // Picking the export itself would empty it: refused, the file is untouched.
            let own = exports.join("e.ndjson.gz");
            let e = save_with(&exports, "e.ndjson.gz", downloads, |_, _| async move {
                Ok(Some(own))
            })
            .await
            .unwrap_err();
            assert_eq!(e.code, "invalid_params");
            assert_eq!(
                std::fs::read(exports.join("e.ndjson.gz")).unwrap(),
                b"the export"
            );

            // The dialog is never reached for an id that is not an export.
            let reached = std::sync::atomic::AtomicBool::new(false);
            let e = save_with(&exports, "../x", None, |_, _| {
                reached.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Ok(None) }
            })
            .await
            .unwrap_err();
            assert_eq!(e.code, "invalid_params");
            assert!(!reached.load(std::sync::atomic::Ordering::SeqCst));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn arguments_are_exactly_an_id() {
            assert!(serde_json::from_value::<SaveArgs>(json!({"id": "a.gz"})).is_ok());
            assert!(
                serde_json::from_value::<SaveArgs>(json!({"id": "a.gz", "path": "/x"})).is_err()
            );
            assert!(serde_json::from_value::<SaveArgs>(json!({})).is_err());
        }
    }
}

/// The files the user picked (readable) and the downloads this shell started (writable).
mod files {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use base64::Engine;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

    use crate::bridge::BridgeError;

    const MAX_CHUNK: u64 = 4 * 1024 * 1024;

    fn set(
        which: &'static OnceLock<Mutex<HashSet<PathBuf>>>,
    ) -> std::sync::MutexGuard<'static, HashSet<PathBuf>> {
        // Paths are inserted whole; a poisoned set is still consistent.
        which
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
    static PICKED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    static DOWNLOADS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

    fn io(e: impl std::fmt::Display) -> BridgeError {
        BridgeError::new("io", e.to_string())
    }

    pub fn allow_read(p: &Path) {
        set(&PICKED).insert(p.to_path_buf());
    }

    #[derive(Deserialize)]
    pub struct ReadPicked {
        path: String,
        offset: u64,
        length: u64,
    }

    pub async fn read_picked(a: ReadPicked) -> Result<Value, BridgeError> {
        let path = PathBuf::from(&a.path);
        if !set(&PICKED).contains(&path) {
            return Err(BridgeError::new(
                "forbidden",
                "only files picked in the file dialog can be read",
            ));
        }
        let mut f = tokio::fs::File::open(&path).await.map_err(io)?;
        let size = f.metadata().await.map_err(io)?.len();
        let offset = a.offset.min(size);
        let len = a.length.min(MAX_CHUNK).min(size - offset);
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(io)?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).await.map_err(io)?;
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(&buf),
            "eof": offset + len >= size,
        }))
    }

    #[derive(Deserialize)]
    pub struct DownloadBegin {
        pub name: String,
    }

    /// A new, unique file in `dir` named after `name` (no directories, no hidden names).
    pub async fn download_begin(dir: &Path, name: &str) -> Result<Value, BridgeError> {
        let base: String = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("download")
            .chars()
            .map(|c| {
                if c.is_control() || ":*?\"<>|".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let base = base.trim().trim_start_matches('.').to_string();
        let base = if base.is_empty() {
            "download".to_string()
        } else {
            base
        };
        let (stem, ext) = match base.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (base.clone(), String::new()),
        };
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
        for n in 0..1000 {
            let file = if n == 0 {
                base.clone()
            } else {
                format!("{stem} ({n}){ext}")
            };
            let p = dir.join(file);
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
                .await
            {
                Ok(_) => {
                    set(&DOWNLOADS).insert(p.clone());
                    return Ok(json!({ "path": p.to_string_lossy() }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(e)),
            }
        }
        Err(BridgeError::new(
            "io",
            "no free file name in the Downloads folder",
        ))
    }

    #[derive(Deserialize)]
    pub struct DownloadChunk {
        path: String,
        data: String,
    }

    pub async fn download_chunk(a: DownloadChunk) -> Result<Value, BridgeError> {
        let path = PathBuf::from(&a.path);
        if !set(&DOWNLOADS).contains(&path) {
            return Err(BridgeError::new(
                "forbidden",
                "not a download started by this window",
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(a.data.as_bytes())
            .map_err(|e| BridgeError::new("invalid_params", e.to_string()))?;
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(io)?;
        f.write_all(&bytes).await.map_err(io)?;
        f.flush().await.map_err(io)?;
        Ok(Value::Null)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn downloads_get_unique_safe_names_and_only_picked_files_are_readable() {
            let dir = std::env::temp_dir().join(format!("marsad-dl-{}", std::process::id()));
            let a = download_begin(&dir, "../../evil.txt").await.unwrap();
            let b = download_begin(&dir, "../../evil.txt").await.unwrap();
            let (a, b) = (
                a["path"].as_str().unwrap().to_string(),
                b["path"].as_str().unwrap().to_string(),
            );
            assert!(a.ends_with("evil.txt") && Path::new(&a).parent() == Some(dir.as_path()));
            assert!(b.ends_with("evil (1).txt"));
            download_chunk(DownloadChunk {
                path: a.clone(),
                data: "aGk=".into(),
            })
            .await
            .unwrap();
            assert_eq!(std::fs::read(&a).unwrap(), b"hi");
            assert!(download_chunk(DownloadChunk {
                path: dir.join("x").to_string_lossy().into(),
                data: "aGk=".into()
            })
            .await
            .is_err());
            assert!(read_picked(ReadPicked {
                path: a.clone(),
                offset: 0,
                length: 2
            })
            .await
            .is_err());
            allow_read(Path::new(&a));
            let r = read_picked(ReadPicked {
                path: a,
                offset: 1,
                length: 9,
            })
            .await
            .unwrap();
            assert_eq!(r["data"], "aQ==");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
