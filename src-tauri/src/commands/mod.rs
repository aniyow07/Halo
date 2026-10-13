//! Tauri commands exposed to the frontend. Every input is validated here;
//! nothing gives the webview filesystem or shell access.

use serde::Serialize;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::AppContext;
use crate::capture::CaptureInfo;
use crate::configuration::{self, KeyStatus, Settings};
use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::gemini::{self, ModelCheck, StreamFailure, Timeouts, VisualContext};
use crate::image_processing::{self, ProcessingStats};
use crate::overlay;
use crate::platform::{self, PlatformInfo};
use crate::selection::{self, CssRect, Viewport};
use crate::shortcut::ShortcutStatus;
use crate::state::Phase;

// ------------------------------------------------------------ activation

#[tauri::command]
pub async fn activate(app: AppHandle) -> HaloResult<()> {
    overlay::activate(app).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySession {
    session_id: String,
    frame_width: u32,
    frame_height: u32,
    capture: CaptureInfo,
    shortcut: String,
}

fn caller_session(window: &WebviewWindow) -> HaloResult<String> {
    overlay::session_id_from_label(window.label()).ok_or_else(|| {
        HaloError::invalid_input("This command is only available in the selection overlay.")
    })
}

/// The overlay asks which session it belongs to (derived from its window
/// label, so a page cannot pick another session).
#[tauri::command]
pub fn get_overlay_session(
    window: WebviewWindow,
    ctx: State<'_, AppContext>,
) -> HaloResult<OverlaySession> {
    let id = caller_session(&window)?;
    let shortcut = ctx.settings().shortcut;
    ctx.halo.with_session(&id, |s| OverlaySession {
        session_id: s.id.clone(),
        frame_width: s.frame.width(),
        frame_height: s.frame.height(),
        capture: s.capture.clone(),
        shortcut,
    })
}

/// Raw RGBA bytes of the frozen frame, for drawing under the selection.
#[tauri::command]
pub fn get_frame(window: WebviewWindow, ctx: State<'_, AppContext>) -> HaloResult<Response> {
    let id = caller_session(&window)?;
    let frame = ctx.halo.frame(&id)?;
    Ok(Response::new(frame.as_raw().clone()))
}

#[tauri::command]
pub fn overlay_ready(window: WebviewWindow) -> HaloResult<()> {
    caller_session(&window)?;
    overlay::show_overlay(&window)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionResult {
    context_id: u64,
    stats: ProcessingStats,
    preview_data_url: String,
}

#[tauri::command]
pub async fn process_selection(
    window: WebviewWindow,
    ctx: State<'_, AppContext>,
    rect: CssRect,
    viewport: Viewport,
) -> HaloResult<SelectionResult> {
    let id = caller_session(&window)?;
    let frame = ctx.halo.frame(&id)?;
    let pixels = selection::to_frame_pixels(rect, viewport, frame.width(), frame.height())?;
    log::info!("selection {rect:?} in viewport {viewport:?} -> frame pixels {pixels:?}");
    let context_id = ctx.halo.begin_selection(&id)?;
    let options = ctx.settings().processing_options();

    let processed = tauri::async_runtime::spawn_blocking(move || {
        image_processing::process_selection(&frame, pixels, options)
    })
    .await
    .map_err(|e| HaloError::internal(format!("Image processing task failed: {e}")))?
    .inspect_err(|_| ctx.halo.set_phase(Phase::Error))?;

    log::info!("processed selection: {:?}", processed.stats);
    let visual = VisualContext::new(processed.format.mime_type(), &processed.bytes);
    let preview_data_url = format!(
        "data:{};base64,{}",
        processed.format.mime_type(),
        visual.data_b64
    );
    ctx.halo
        .set_visual(&id, context_id, visual, processed.stats.clone())?;
    Ok(SelectionResult {
        context_id,
        stats: processed.stats,
        preview_data_url,
    })
}

#[tauri::command]
pub fn dismiss(app: AppHandle, window: WebviewWindow) -> HaloResult<()> {
    let id = overlay::session_id_from_label(window.label());
    overlay::dismiss(&app, id.as_deref());
    Ok(())
}

// --------------------------------------------------------------- Gemini

#[derive(Clone, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum StreamEvent {
    Started {
        request_id: String,
    },
    Delta {
        request_id: String,
        seq: u64,
        text: String,
    },
    Done {
        request_id: String,
        finish_reason: Option<String>,
    },
    Failed {
        request_id: String,
        error: HaloError,
    },
    Cancelled {
        request_id: String,
    },
}

#[tauri::command]
pub async fn ask(
    app: AppHandle,
    window: WebviewWindow,
    question: String,
    on_event: Channel<StreamEvent>,
) -> HaloResult<String> {
    let session_id = caller_session(&window)?;
    let ctx = app.state::<AppContext>();
    let api_key = configuration::resolve_api_key()?;
    let model = ctx.settings().model;
    let (req, payload) = ctx.halo.begin_request(&session_id, &question)?;
    let request_id = req.id.clone();
    let client = ctx.http.clone();
    let app2 = app.clone();

    tauri::async_runtime::spawn(async move {
        let ctx = app2.state::<AppContext>();
        let id = req.id.clone();
        let _ = on_event.send(StreamEvent::Started {
            request_id: id.clone(),
        });
        let mut seq = 0u64;
        let result = gemini::stream_generate_with_retry(
            &client,
            &gemini::Endpoint {
                base_url: gemini::API_BASE,
                api_key: &api_key,
                model: &model,
            },
            &payload,
            Timeouts::default(),
            &req.cancel,
            |text| {
                // Drop deltas from a request that has been superseded.
                if !ctx.halo.is_active(&id) {
                    return;
                }
                if seq == 0 {
                    ctx.halo.set_phase(Phase::StreamingResponse);
                }
                seq += 1;
                let _ = on_event.send(StreamEvent::Delta {
                    request_id: id.clone(),
                    seq,
                    text: text.to_string(),
                });
            },
        )
        .await;

        match result {
            Ok(out) => {
                if ctx
                    .halo
                    .finish_request(&id, Some(out.text), Phase::Completed)
                {
                    let _ = on_event.send(StreamEvent::Done {
                        request_id: id,
                        finish_reason: out.finish_reason,
                    });
                }
            }
            Err(StreamFailure::Cancelled { .. }) => {
                ctx.halo.finish_request(&id, None, Phase::Cancelled);
                let _ = on_event.send(StreamEvent::Cancelled { request_id: id });
            }
            Err(StreamFailure::Error(error)) => {
                log::warn!("request {id} failed: {:?} {}", error.kind, error.message);
                if ctx.halo.finish_request(&id, None, Phase::Error) {
                    let _ = on_event.send(StreamEvent::Failed {
                        request_id: id,
                        error,
                    });
                }
            }
        }
    });

    Ok(request_id)
}

#[tauri::command]
pub fn cancel_request(ctx: State<'_, AppContext>, request_id: String) -> bool {
    ctx.halo.cancel_request(&request_id)
}

#[tauri::command]
pub async fn check_model(ctx: State<'_, AppContext>, model: String) -> HaloResult<ModelCheck> {
    if !configuration::is_valid_model_id(&model) {
        return Err(HaloError::new(
            ErrorKind::Configuration,
            "That model id is not valid.",
        ));
    }
    let key = configuration::resolve_api_key()?;
    gemini::check_model(&ctx.http, gemini::API_BASE, &key, &model).await
}

// ------------------------------------------------------------- settings

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    settings: Settings,
    shortcut_status: Option<ShortcutStatus>,
    load_warning: Option<String>,
}

#[tauri::command]
pub fn get_settings(ctx: State<'_, AppContext>) -> SettingsView {
    SettingsView {
        settings: ctx.settings(),
        shortcut_status: ctx.shortcuts.status(),
        load_warning: ctx.load_warning.clone(),
    }
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    ctx: State<'_, AppContext>,
    settings: Settings,
) -> HaloResult<SettingsView> {
    settings.validate()?;
    let previous = ctx.settings();
    let mut settings = settings;
    if settings.shortcut != previous.shortcut {
        let status = ctx.shortcuts.register(&app, &settings.shortcut)?;
        settings.shortcut = status.shortcut;
    }
    configuration::save_settings(&ctx.config_dir, &settings)?;
    *ctx.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings;
    Ok(get_settings(ctx))
}

#[tauri::command]
pub fn get_key_status() -> KeyStatus {
    configuration::key_status()
}

#[tauri::command]
pub fn set_api_key(key: String) -> HaloResult<KeyStatus> {
    configuration::save_api_key(&key)?;
    Ok(configuration::key_status())
}

#[tauri::command]
pub fn clear_api_key() -> HaloResult<KeyStatus> {
    configuration::clear_api_key()?;
    Ok(configuration::key_status())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    platform: PlatformInfo,
    capture_backend: &'static str,
    phase: Phase,
    shortcut: Option<ShortcutStatus>,
    last_capture: Option<CaptureInfo>,
    config_dir: String,
    version: &'static str,
}

#[tauri::command]
pub fn get_diagnostics(ctx: State<'_, AppContext>) -> Diagnostics {
    let platform = platform::info();
    let capture_backend = match platform.display_server {
        platform::DisplayServer::Wayland => "Wayland wlr-screencopy (libwayshot)",
        platform::DisplayServer::X11 => "X11 GetImage + RandR (x11rb)",
        platform::DisplayServer::Windows => "Windows GDI (xcap)",
        platform::DisplayServer::Unknown => "unavailable (no display server detected)",
    };
    Diagnostics {
        platform,
        capture_backend,
        phase: ctx.halo.phase(),
        shortcut: ctx.shortcuts.status(),
        last_capture: ctx
            .last_capture
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
        config_dir: ctx.config_dir.display().to_string(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Open a link from a model answer in the default browser. Only http(s)
/// URLs are allowed; the click is always user-initiated in the UI.
#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> HaloResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let parsed =
        reqwest::Url::parse(&url).map_err(|_| HaloError::invalid_input("Invalid link."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(HaloError::invalid_input(
            "Only http and https links can be opened.",
        ));
    }
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|e| HaloError::internal(format!("Could not open link: {e}")))
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    if let Some(main) = app.get_webview_window(overlay::MAIN_LABEL) {
        let _ = main.show();
        let _ = main.set_focus();
    }
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_events_serialize_as_the_frontend_expects() {
        let v = serde_json::to_value(StreamEvent::Delta {
            request_id: "r".into(),
            seq: 3,
            text: "hi".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "type": "delta", "requestId": "r", "seq": 3, "text": "hi" })
        );
        let v = serde_json::to_value(StreamEvent::Done {
            request_id: "r".into(),
            finish_reason: Some("STOP".into()),
        })
        .unwrap();
        assert_eq!(v["finishReason"], "STOP");
        let v = serde_json::to_value(StreamEvent::Failed {
            request_id: "r".into(),
            error: HaloError::new(ErrorKind::RateLimited, "x"),
        })
        .unwrap();
        assert_eq!(v["error"]["kind"], "rateLimited");
    }
}
