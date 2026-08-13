//! Platform tweaks applied to the main webview once it exists.
//!
//! Right-clicking a WebView2 control opens Edge's own menu — Back, Reload, Save
//! as, Inspect — which is a browser affordance with no place in a desktop app.
//! Turning it off at the source (`AreDefaultContextMenusEnabled` on the
//! webview's settings) beats a `contextmenu` listener in the frontend: it covers
//! the whole surface, including areas React never paints, and cannot be undone
//! by a stray handler.

use tauri::{AppHandle, Manager, Runtime};

/// Turns off the WebView2 right-click menu on the main window.
///
/// Best effort. A failure here costs a menu that should not be there, which is
/// not worth refusing to start over, so each step logs and moves on. Devtools
/// stay reachable in debug builds through F12 / Ctrl+Shift+I.
pub fn disable_context_menu<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!("context menu left enabled: no `main` webview window");
        return;
    };

    // The closure runs on the main thread. Called from `setup`, which is already
    // on it, the dispatcher applies this inline rather than queueing it.
    let dispatched = window.with_webview(|_webview| {
        #[cfg(windows)]
        {
            // SAFETY: COM calls on the WebView2 interfaces Tauri hands us, made
            // on the thread that owns the controller — which is what they
            // require. Every handle is checked before use.
            let applied = unsafe {
                _webview
                    .controller()
                    .CoreWebView2()
                    .and_then(|core| core.Settings())
                    .and_then(|settings| settings.SetAreDefaultContextMenusEnabled(false))
            };
            if let Err(e) = applied {
                log::warn!("could not disable the WebView2 context menu: {e}");
            }
        }
    });

    if let Err(e) = dispatched {
        log::warn!("could not reach the platform webview: {e}");
    }
}
