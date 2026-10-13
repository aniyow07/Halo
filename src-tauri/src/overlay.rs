//! Native window lifecycle: activation, the selection overlay window and
//! restoring the desktop on dismissal.

use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::AppContext;
use crate::capture::{self, CaptureInfo, MonitorHint};
use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::platform;
use crate::state::{Session, new_id};

pub const MAIN_LABEL: &str = "main";
const OVERLAY_PREFIX: &str = "overlay-";

/// Time for the compositor to finish unmapping a hidden window (Hyprland
/// animates closing windows) so it does not appear in the capture.
const HIDE_SETTLE: Duration = Duration::from_millis(350);

pub fn overlay_label(session_id: &str) -> String {
    format!("{OVERLAY_PREFIX}{}", session_id.replace('-', ""))
}

pub fn session_id_from_label(label: &str) -> Option<String> {
    let hex = label.strip_prefix(OVERLAY_PREFIX)?;
    uuid::Uuid::parse_str(hex).ok().map(|u| u.to_string())
}

pub fn is_overlay(label: &str) -> bool {
    label.starts_with(OVERLAY_PREFIX)
}

fn overlay_windows<R: Runtime>(app: &AppHandle<R>) -> Vec<WebviewWindow<R>> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| is_overlay(label))
        .map(|(_, w)| w)
        .collect()
}

fn monitor_hint<R: Runtime>(app: &AppHandle<R>) -> MonitorHint {
    #[cfg(target_os = "linux")]
    if platform::is_hyprland() {
        match platform::hyprland::focused_monitor() {
            Ok(m) => {
                return MonitorHint {
                    name: Some(m.name),
                    physical_position: None,
                };
            }
            Err(e) => log::warn!("could not query focused Hyprland monitor: {e}"),
        }
    }
    let by_cursor = app.cursor_position().ok().and_then(|p| {
        app.available_monitors().ok()?.into_iter().find(|m| {
            let pos = m.position();
            let size = m.size();
            p.x >= f64::from(pos.x)
                && p.y >= f64::from(pos.y)
                && p.x < f64::from(pos.x) + f64::from(size.width)
                && p.y < f64::from(pos.y) + f64::from(size.height)
        })
    });
    let monitor = by_cursor.or_else(|| app.primary_monitor().ok().flatten());
    match monitor {
        Some(m) => MonitorHint {
            name: m.name().cloned(),
            physical_position: Some((m.position().x, m.position().y)),
        },
        None => MonitorHint::default(),
    }
}

/// Full activation: hide Halo's own windows, capture the target monitor,
/// then show the selection overlay on top of the frozen frame.
pub async fn activate<R: Runtime>(app: AppHandle<R>) -> HaloResult<()> {
    let ctx = app.state::<AppContext>();
    ctx.halo.begin_activation()?;

    let result = activate_inner(&app).await;
    if let Err(e) = &result {
        log::error!("activation failed: {e}");
        ctx.halo.fail_activation();
        for w in overlay_windows(&app) {
            let _ = w.destroy();
        }
        restore_main(&app, true);
        let _ = app.emit_to(MAIN_LABEL, "halo://activation-error", e.clone());
    }
    result
}

async fn activate_inner<R: Runtime>(app: &AppHandle<R>) -> HaloResult<()> {
    let ctx = app.state::<AppContext>();
    let mut waited = false;

    // A previous overlay (e.g. an answer panel still open) must go before
    // capturing, or it would be in the picture.
    for w in overlay_windows(app) {
        w.destroy()?;
        waited = true;
    }
    if let Some(main) = app.get_webview_window(MAIN_LABEL)
        && main.is_visible().unwrap_or(false)
        && !main.is_minimized().unwrap_or(false)
    {
        main.hide()?;
        ctx.restore_main.store(true, Ordering::SeqCst);
        waited = true;
    }
    if waited {
        tokio::time::sleep(HIDE_SETTLE).await;
    }

    let hint = monitor_hint(app);
    let captured = tauri::async_runtime::spawn_blocking(move || capture::capture_monitor(&hint))
        .await
        .map_err(|e| HaloError::new(ErrorKind::Capture, format!("Capture task failed: {e}")))??;

    log::info!(
        "captured {}x{} from {} via {} in {}ms",
        captured.info.width,
        captured.info.height,
        captured.info.monitor_name,
        captured.info.backend,
        captured.info.duration_ms
    );
    *ctx.last_capture.lock().unwrap_or_else(|p| p.into_inner()) = Some(captured.info.clone());

    let session_id = new_id();
    let info = captured.info.clone();
    ctx.halo.install_session(Session {
        id: session_id.clone(),
        frame: std::sync::Arc::new(captured.image),
        capture: captured.info,
        context_id: 0,
        visual: None,
        stats: None,
        history: Vec::new(),
    });

    create_overlay(app, &session_id, &info)?;
    Ok(())
}

fn create_overlay<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    info: &CaptureInfo,
) -> HaloResult<()> {
    let label = overlay_label(session_id);
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("Halo selection")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .visible(false)
        .focused(true);

    // Create the window at the monitor's logical size. WebKitGTK does not
    // always re-layout after a fullscreen resize (it kept the default
    // 800x600 viewport and stretched it on Hyprland), so the page must start
    // at the final size.
    let scale = info.scale_factor.max(0.1);
    builder = builder.inner_size(
        f64::from(info.width) / scale,
        f64::from(info.height) / scale,
    );
    if platform::display_server() != platform::DisplayServer::Wayland {
        // Place the window on the captured monitor before going fullscreen.
        // (Wayland compositors ignore client positioning; Hyprland opens new
        // windows on the focused monitor, which is the one we captured.)
        builder = builder.position(
            f64::from(info.monitor_x) / scale,
            f64::from(info.monitor_y) / scale,
        );
    }
    let window = builder.fullscreen(true).build()?;

    // Safety net: if the page never reports ready, show it anyway so the
    // user is not left with an invisible, focus-stealing window.
    let w = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        if !w.is_visible().unwrap_or(true) {
            log::warn!("overlay did not report ready; showing it anyway");
            let _ = w.show();
            let _ = w.set_focus();
        }
    });
    Ok(())
}

pub fn show_overlay<R: Runtime>(window: &WebviewWindow<R>) -> HaloResult<()> {
    window.show()?;
    window.set_focus()?;
    Ok(())
}

/// Bring the home window back if Halo hid it for the capture (or `force`).
pub fn restore_main<R: Runtime>(app: &AppHandle<R>, force: bool) {
    let ctx = app.state::<AppContext>();
    let was_hidden = ctx.restore_main.swap(false, Ordering::SeqCst);
    if (was_hidden || force)
        && let Some(main) = app.get_webview_window(MAIN_LABEL)
    {
        let _ = main.show();
        if was_hidden {
            let _ = main.set_focus();
        }
    }
}

/// End the session and close its overlay, returning the desktop to normal.
pub fn dismiss<R: Runtime>(app: &AppHandle<R>, session_id: Option<&str>) {
    let ctx = app.state::<AppContext>();
    ctx.halo.end_session(session_id);
    for w in overlay_windows(app) {
        let matches = session_id.is_none_or(|id| w.label() == overlay_label(id));
        if matches {
            let _ = w.destroy();
        }
    }
    restore_main(app, false);
}

/// Window-destroyed hook: an overlay closed by any means (Escape, the
/// compositor's close keybinding, a crash of the page) ends its session.
pub fn on_window_destroyed<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(session_id) = session_id_from_label(label) {
        let ctx = app.state::<AppContext>();
        if ctx.halo.end_session(Some(&session_id)) {
            restore_main(app, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_labels_round_trip_session_ids() {
        let id = new_id();
        let label = overlay_label(&id);
        assert!(is_overlay(&label));
        assert!(label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(session_id_from_label(&label).as_deref(), Some(id.as_str()));
        assert_eq!(session_id_from_label("main"), None);
        assert_eq!(session_id_from_label("overlay-zzz"), None);
    }
}
