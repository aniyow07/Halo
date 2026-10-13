//! Platform detection and compositor integration.

#[cfg(target_os = "linux")]
pub mod hyprland;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayServer {
    Windows,
    X11,
    Wayland,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
    pub display_server: DisplayServer,
    pub desktop: Option<String>,
    pub hyprland: bool,
}

pub fn display_server() -> DisplayServer {
    if cfg!(windows) {
        return DisplayServer::Windows;
    }
    let session_type = std::env::var("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || session_type == "wayland" {
        DisplayServer::Wayland
    } else if std::env::var_os("DISPLAY").is_some() || session_type == "x11" {
        DisplayServer::X11
    } else {
        DisplayServer::Unknown
    }
}

pub fn is_hyprland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

pub fn info() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS,
        display_server: display_server(),
        desktop: std::env::var("XDG_CURRENT_DESKTOP").ok(),
        hyprland: is_hyprland(),
    }
}
