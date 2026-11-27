//! Desktop integration: tray menu, hide-on-close, native notifications and
//! focusing the existing window when the app is launched again.

use {
    tauri::{
        AppHandle, Manager, WindowEvent,
        menu::{MenuBuilder, MenuItemBuilder},
        tray::TrayIconBuilder,
    },
    tauri_plugin_notification::NotificationExt,
};

use crate::sidecar::SidecarEvent;

pub const MAIN_WINDOW: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

/// Bring the main window back (tray click, second launch, dock reopen).
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A tray icon with "Show" and "Quit". Closing the window only hides it: the
/// agent keeps running in the daemon until the user quits from the tray.
pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id(MENU_SHOW, "Show Antre").build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "Quit Antre").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;
    let mut tray = TrayIconBuilder::with_id("antre")
        .tooltip("Antre")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {},
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Window events for the main window: close hides it instead of quitting.
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

/// The notification text for a supervisor event.
#[must_use]
pub fn sidecar_notice(event: SidecarEvent) -> (&'static str, &'static str) {
    match event {
        SidecarEvent::Restarted => ("Antre restarted", "The agent daemon crashed and is back."),
        SidecarEvent::GaveUp => (
            "Antre stopped",
            "The agent daemon keeps crashing. Quit and reopen Antre, or check its logs.",
        ),
    }
}

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!(%error, "notification failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sidecar_event_has_a_notice() {
        for event in [SidecarEvent::Restarted, SidecarEvent::GaveUp] {
            let (title, body) = sidecar_notice(event);
            assert!(!title.is_empty() && !body.is_empty());
        }
    }
}
