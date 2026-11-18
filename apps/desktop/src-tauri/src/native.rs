//! Native-only `desktop.*` methods of the `rpc` bridge: open a link or path with the OS,
//! reveal a path in the file manager, pick a folder.

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
        other => Err(BridgeError::new(
            "method_not_found",
            format!("unknown method `desktop.{other}`"),
        )),
    }
}
