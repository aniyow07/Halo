mod capture;
mod commands;
mod configuration;
mod errors;
mod gemini;
mod image_processing;
mod overlay;
mod platform;
mod selection;
mod shortcut;
mod state;

#[cfg(test)]
mod live_tests;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use capture::CaptureInfo;
use configuration::Settings;
use shortcut::ShortcutManager;
use state::HaloState;

pub struct AppContext {
    pub halo: HaloState,
    pub settings: Mutex<Settings>,
    pub config_dir: PathBuf,
    pub load_warning: Option<String>,
    pub shortcuts: ShortcutManager,
    pub http: reqwest::Client,
    pub restore_main: AtomicBool,
    pub last_capture: Mutex<Option<CaptureInfo>>,
}

impl AppContext {
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

fn wants_activation(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == "--activate")
}

fn spawn_activation(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Errors are reported to the home window by `activate`; a "busy"
        // rejection simply means a selection is already on screen.
        if let Err(e) = overlay::activate(app).await {
            log::info!("activation not started: {}", e.message);
        }
    });
}

/// Exit cleanly on SIGTERM/SIGINT/SIGHUP so the global shortcut (a
/// compositor binding on Hyprland) is always removed.
fn spawn_signal_handler(app: &AppHandle) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
                signal(SignalKind::hangup()),
            ) else {
                log::warn!("could not install signal handlers");
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
                _ = hup.recv() => {}
            }
            log::info!("signal received, shutting down");
            app.exit(0);
        });
    }
    #[cfg(not(unix))]
    let _ = app;
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let select = MenuItem::with_id(app, "select", "Select screen region", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Halo", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Halo", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&select, &open, &quit])?;
    let mut builder = TrayIconBuilder::with_id("halo")
        .tooltip("Halo")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "select" => spawn_activation(app),
            "open" => {
                if let Some(w) = app.get_webview_window(overlay::MAIN_LABEL) {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Developer convenience: GEMINI_API_KEY / GEMINI_MODEL from a .env file.
    // Read by the native backend only; never bundled into the frontend.
    let _ = dotenvy::dotenv();
    env_logger_init();

    let start_activated = wants_activation(&std::env::args().collect::<Vec<_>>());

    let mut builder =
        tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if wants_activation(&args) {
                spawn_activation(app);
            } else if let Some(w) = app.get_webview_window(overlay::MAIN_LABEL) {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    if shortcut::needs_hotkey_plugin() {
        builder = builder.plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        spawn_activation(app);
                    }
                })
                .build(),
        );
    }

    let app = builder
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let config_dir = app.path().app_config_dir()?;
            let (settings, warning) = configuration::load_settings(&config_dir);
            let shortcut = settings.shortcut.clone();
            app.manage(AppContext {
                halo: HaloState::default(),
                settings: Mutex::new(settings),
                config_dir,
                load_warning: warning.map(|w| w.message),
                shortcuts: ShortcutManager::default(),
                http: gemini::build_client(gemini::Timeouts::default())
                    .map_err(|e| Box::<dyn std::error::Error>::from(e.message))?,
                restore_main: AtomicBool::new(false),
                last_capture: Mutex::new(None),
            });

            let handle = app.handle().clone();
            let ctx = handle.state::<AppContext>();
            match ctx.shortcuts.register(&handle, &shortcut) {
                Ok(s) => log::info!("shortcut {} registered via {:?}", s.shortcut, s.backend),
                Err(e) => log::warn!("shortcut registration failed: {}", e.message),
            }
            spawn_signal_handler(&handle);
            if let Err(e) = setup_tray(&handle) {
                log::warn!("system tray unavailable: {e}");
            }
            if start_activated {
                if let Some(w) = handle.get_webview_window(overlay::MAIN_LABEL) {
                    let _ = w.hide();
                }
                spawn_activation(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the home window hides it; Halo keeps running so the
            // shortcut works. Quit from the window or tray menu.
            WindowEvent::CloseRequested { api, .. } if window.label() == overlay::MAIN_LABEL => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Destroyed => {
                overlay::on_window_destroyed(window.app_handle(), window.label())
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::activate,
            commands::get_overlay_session,
            commands::get_frame,
            commands::overlay_ready,
            commands::process_selection,
            commands::dismiss,
            commands::ask,
            commands::cancel_request,
            commands::check_model,
            commands::get_settings,
            commands::save_settings,
            commands::get_key_status,
            commands::set_api_key,
            commands::clear_api_key,
            commands::get_diagnostics,
            commands::open_external,
            commands::show_main,
            commands::quit,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Halo application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let ctx = app.state::<AppContext>();
            ctx.halo.end_session(None);
            ctx.shortcuts.unregister_all(app);
        }
    });
}

fn env_logger_init() {
    struct StderrLogger;
    impl log::Log for StderrLogger {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info && m.target().starts_with("halo")
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("[halo {}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: StderrLogger = StderrLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
