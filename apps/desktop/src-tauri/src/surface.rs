//! `desktop.window_surface`: the window's background — solid, or translucent with the OS's
//! blurred backdrop where there is one (Windows Acrylic, else Mica, else Blur; macOS
//! vibrancy). On Linux the window is see-through (alpha) without blur, and only when the
//! desktop has a compositor; otherwise the answer says it stays solid.
//!
//! The window is created transparent (`tauri.conf.json`); the UI paints its own surfaces, opaque
//! or with alpha (`[data-surface]` tokens), so "solid" needs no native change besides clearing
//! the effect.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Webview};
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
    // Acrylic first: it blurs the windows behind (what "translucent" should show); Mica only
    // tints from the wallpaper and reads as a flat shade. On Windows 11 the system backdrop
    // is drawn solid while the window is inactive (by design); the UI's tint layer is the same
    // either way.
    // `MARSAD_WINDOW_EFFECT` (acrylic | mica | blur | none) pins one effect: CI uses `none` to
    // check the window/WebView transparency on its own.
    let only = std::env::var("MARSAD_WINDOW_EFFECT").ok();
    let wants = |e: &str| only.as_deref().is_none_or(|o| o == e);
    let effect = if wants("acrylic") && apply_acrylic(w, None).is_ok() {
        Some("acrylic")
    } else if wants("mica") && apply_mica(w, None).is_ok() {
        Some("mica")
    } else if wants("blur") && apply_blur(w, None).is_ok() {
        Some("blur")
    } else {
        None
    };
    tracing::info!(?effect, "window surface: translucent");
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

/// Test hook: `MARSAD_SURFACE=translucent[:<strength 0..100>]` or `solid` makes the page start
/// with that window background (the UI's own cached choice), as if picked in Settings. Used by
/// CI to screenshot the native backdrop; a hub setting chosen later still wins.
pub const ENV: &str = "MARSAD_SURFACE";

pub fn on_page_load(w: &Webview) {
    let Ok(v) = std::env::var(ENV) else {
        return;
    };
    let (surface, strength) = v.split_once(':').unwrap_or((v.as_str(), "50"));
    let ok_surface = surface == "translucent" || surface == "solid";
    let Ok(strength) = strength.parse::<u8>() else {
        tracing::warn!(value = %v, "{ENV}: bad strength");
        return;
    };
    if !ok_surface || strength > 100 {
        tracing::warn!(value = %v, "{ENV}: expected translucent[:0..100] or solid");
        return;
    }
    // Reload once when the cached choice differs, so the first paint already has it.
    let js = format!(
        "(function(){{var s=localStorage.getItem('marsad.surface'),n=localStorage.getItem('marsad.surface.strength');\
         if(s!=='{surface}'||n!=='{strength}'){{localStorage.setItem('marsad.surface','{surface}');\
         localStorage.setItem('marsad.surface.strength','{strength}');location.reload();}}}})()"
    );
    if let Err(e) = w.eval(&js) {
        tracing::warn!(error = %e, "{ENV}: cannot apply");
    }
}
