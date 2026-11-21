//! `desktop.window_surface`: the window's background — solid, or translucent with the OS's
//! blurred backdrop where there is one (Windows 11 Mica, else Acrylic, else Blur; macOS
//! vibrancy). On Linux the window is see-through (alpha) without blur, and only when the
//! desktop has a compositor; otherwise the answer says it stays solid.
//!
//! The window is created transparent (`tauri.conf.json`); the UI paints its own surfaces, opaque
//! or with alpha (`[data-surface]` tokens), so "solid" needs no native change besides clearing
//! the effect.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

use crate::bridge::BridgeError;

#[derive(Debug, Deserialize)]
pub struct SurfaceArgs {
    pub translucent: bool,
}

/// What the window can show now.
#[derive(Debug, Serialize)]
pub struct SurfaceResult {
    /// Whatever is behind the window shows through (the UI may use alpha surfaces).
    pub transparent: bool,
    /// The native backdrop applied (`mica`, `acrylic`, `blur`, `vibrancy`), if any.
    pub effect: Option<&'static str>,
}

pub async fn window_surface(app: &AppHandle, a: SurfaceArgs) -> Result<Value, BridgeError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| BridgeError::new("io", "the main window is not open"))?;
    let (tx, rx) = oneshot::channel();
    // Native window calls run on the UI thread; this future only waits for the answer.
    app.run_on_main_thread(move || {
        // The caller may have gone away.
        let _ = tx.send(apply(&window, a.translucent));
    })
    .map_err(|e| BridgeError::new("io", e.to_string()))?;
    let r = rx
        .await
        .map_err(|_| BridgeError::new("io", "the window went away"))?;
    serde_json::to_value(r).map_err(|e| BridgeError::new("io", e.to_string()))
}

#[cfg(target_os = "windows")]
fn apply(w: &tauri::WebviewWindow, translucent: bool) -> SurfaceResult {
    use window_vibrancy::{
        apply_acrylic, apply_blur, apply_mica, clear_acrylic, clear_blur, clear_mica,
    };
    // Clearing an effect that is not applied is harmless; nothing to report either way.
    let _ = clear_mica(w);
    let _ = clear_acrylic(w);
    let _ = clear_blur(w);
    if !translucent {
        return SurfaceResult {
            transparent: false,
            effect: None,
        };
    }
    let effect = if apply_mica(w, None).is_ok() {
        Some("mica")
    } else if apply_acrylic(w, None).is_ok() {
        Some("acrylic")
    } else if apply_blur(w, None).is_ok() {
        Some("blur")
    } else {
        None
    };
    // A WebView2 window created transparent shows the desktop through alpha pixels even
    // without a backdrop effect.
    SurfaceResult {
        transparent: true,
        effect,
    }
}

#[cfg(target_os = "macos")]
fn apply(w: &tauri::WebviewWindow, translucent: bool) -> SurfaceResult {
    use window_vibrancy::{
        apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
    };
    // Clearing when nothing is applied is harmless.
    let _ = clear_vibrancy(w);
    if !translucent {
        return SurfaceResult {
            transparent: false,
            effect: None,
        };
    }
    let ok = apply_vibrancy(
        w,
        NSVisualEffectMaterial::UnderWindowBackground,
        Some(NSVisualEffectState::FollowsWindowActiveState),
        None,
    )
    .is_ok();
    SurfaceResult {
        transparent: true,
        effect: ok.then_some("vibrancy"),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply(w: &tauri::WebviewWindow, translucent: bool) -> SurfaceResult {
    use gtk::prelude::GtkWindowExt;
    // Alpha only shows through with a compositing window manager; without one the
    // transparent pixels are black, so the UI stays solid.
    let composited = w
        .gtk_window()
        .ok()
        .and_then(|g| g.screen())
        .is_some_and(|s| s.is_composited());
    SurfaceResult {
        transparent: translucent && composited,
        effect: None,
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn apply(_w: &tauri::WebviewWindow, _translucent: bool) -> SurfaceResult {
    SurfaceResult {
        transparent: false,
        effect: None,
    }
}
