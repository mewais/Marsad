//! Native-only `desktop.*` methods of the `rpc` bridge: open a link or path with the OS,
//! reveal a path in the file manager, pick a folder, set the window background (`surface`),
//! pick files to attach and read them (only files the user picked), and save downloads into
//! the Downloads folder (only files this shell created there).

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
                app.opener()
                    .open_path(a.target, None::<&str>)
                    .map_err(failed)?;
            }
            Ok(Value::Null)
        }
        "reveal" => {
            let a: OpenArgs = args(p)?;
            app.opener().reveal_item_in_dir(a.target).map_err(failed)?;
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
            if let Some(dir) = a.default_path {
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
        "window_surface" => crate::surface::window_surface(app, args(p)?).await,
        other => Err(BridgeError::new(
            "method_not_found",
            format!("unknown method `desktop.{other}`"),
        )),
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

    fn set(which: &'static OnceLock<Mutex<HashSet<PathBuf>>>) -> std::sync::MutexGuard<'static, HashSet<PathBuf>> {
        // Paths are inserted whole; a poisoned set is still consistent.
        which.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
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
            return Err(BridgeError::new("forbidden", "only files picked in the file dialog can be read"));
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
            .map(|c| if c.is_control() || ":*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        let base = base.trim().trim_start_matches('.').to_string();
        let base = if base.is_empty() { "download".to_string() } else { base };
        let (stem, ext) = match base.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (base.clone(), String::new()),
        };
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
        for n in 0..1000 {
            let file = if n == 0 { base.clone() } else { format!("{stem} ({n}){ext}") };
            let p = dir.join(file);
            match tokio::fs::OpenOptions::new().write(true).create_new(true).open(&p).await {
                Ok(_) => {
                    set(&DOWNLOADS).insert(p.clone());
                    return Ok(json!({ "path": p.to_string_lossy() }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(e)),
            }
        }
        Err(BridgeError::new("io", "no free file name in the Downloads folder"))
    }

    #[derive(Deserialize)]
    pub struct DownloadChunk {
        path: String,
        data: String,
    }

    pub async fn download_chunk(a: DownloadChunk) -> Result<Value, BridgeError> {
        let path = PathBuf::from(&a.path);
        if !set(&DOWNLOADS).contains(&path) {
            return Err(BridgeError::new("forbidden", "not a download started by this window"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(a.data.as_bytes())
            .map_err(|e| BridgeError::new("invalid_params", e.to_string()))?;
        let mut f = tokio::fs::OpenOptions::new().append(true).open(&path).await.map_err(io)?;
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
            let (a, b) = (a["path"].as_str().unwrap().to_string(), b["path"].as_str().unwrap().to_string());
            assert!(a.ends_with("evil.txt") && Path::new(&a).parent() == Some(dir.as_path()));
            assert!(b.ends_with("evil (1).txt"));
            download_chunk(DownloadChunk { path: a.clone(), data: "aGk=".into() }).await.unwrap();
            assert_eq!(std::fs::read(&a).unwrap(), b"hi");
            assert!(download_chunk(DownloadChunk { path: dir.join("x").to_string_lossy().into(), data: "aGk=".into() }).await.is_err());
            assert!(read_picked(ReadPicked { path: a.clone(), offset: 0, length: 2 }).await.is_err());
            allow_read(Path::new(&a));
            let r = read_picked(ReadPicked { path: a, offset: 1, length: 9 }).await.unwrap();
            assert_eq!(r["data"], "aQ==");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
