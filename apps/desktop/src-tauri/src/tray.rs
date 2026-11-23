//! Tray / menu-bar item: shows what needs the user (sessions that need an answer, hosts that
//! wait for a sign-in); clicking it (or "Show") brings the window forward. Where the platform has no tray (e.g. Linux without an
//! AppIndicator host) the app runs without it.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::notify::Attention;

/// The tray and its attention line (managed state when the tray exists).
pub struct Tray {
    icon: TrayIcon<Wry>,
    attention: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let attention = MenuItem::with_id(app, "attention", "Nothing needs you", false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Marsad", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&attention, &sep, &show, &quit])?;
    let mut b = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Marsad")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "show" | "attention" => crate::watch::bring_forward(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                crate::watch::bring_forward(tray.app_handle());
            }
        });
    if let Some(i) = app.default_window_icon() {
        b = b.icon(i.clone());
    }
    let icon = b.build(app)?;
    app.manage(Tray { icon, attention });
    Ok(())
}

/// Show what needs the user: sessions needing an answer and hosts waiting for a sign-in.
pub fn set_attention(app: &AppHandle, a: Attention) {
    let Some(t) = app.try_state::<Tray>() else {
        return;
    };
    let n = a.total();
    let text = a.words();
    let tooltip = if n == 0 {
        "Marsad".to_string()
    } else {
        format!("Marsad — {text}")
    };
    let title = (n > 0).then(|| n.to_string());
    let r = t
        .attention
        .set_text(&text)
        .and_then(|_| t.icon.set_tooltip(Some(&tooltip)))
        .and_then(|_| t.icon.set_title(title.as_deref()));
    if let Err(e) = r {
        crate::diag::failed("tray_update", &e);
    }
}
