//! Global activation shortcut.
//!
//! Backends:
//! - Windows / X11: `tauri-plugin-global-shortcut` (OS-level hotkey).
//! - Hyprland (Wayland): a runtime compositor keybinding that runs
//!   `halo --activate`; the single-instance plugin forwards that to the
//!   running app.
//! - Other Wayland compositors: applications cannot grab global keys, so the
//!   user binds `halo --activate` in their compositor. Halo reports this
//!   instead of pretending the shortcut works.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Runtime};

use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::platform::{self, DisplayServer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Letter(char),
    Digit(char),
    Space,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
    pub key: Key,
}

impl ShortcutSpec {
    pub fn parse(input: &str) -> HaloResult<Self> {
        let bad = |m: &str| {
            HaloError::new(
                ErrorKind::Shortcut,
                format!("Invalid shortcut \"{input}\": {m}"),
            )
        };
        let mut spec = ShortcutSpec {
            ctrl: false,
            shift: false,
            alt: false,
            super_key: false,
            key: Key::Space,
        };
        let mut key = None;
        let parts: Vec<&str> = input.split('+').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(bad("empty component"));
        }
        for part in parts {
            let lower = part.to_ascii_lowercase();
            let modifier = match lower.as_str() {
                "ctrl" | "control" => Some(&mut spec.ctrl),
                "shift" => Some(&mut spec.shift),
                "alt" => Some(&mut spec.alt),
                "super" | "meta" | "win" | "cmd" => Some(&mut spec.super_key),
                _ => None,
            };
            if let Some(m) = modifier {
                if *m {
                    return Err(bad("duplicate modifier"));
                }
                *m = true;
                continue;
            }
            if key.is_some() {
                return Err(bad("only one non-modifier key is allowed"));
            }
            key = Some(match lower.as_str() {
                "space" => Key::Space,
                s if s.len() == 1 && s.as_bytes()[0].is_ascii_lowercase() => {
                    Key::Letter(s.as_bytes()[0].to_ascii_uppercase() as char)
                }
                s if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() => {
                    Key::Digit(s.as_bytes()[0] as char)
                }
                s if s.starts_with('f') => match s[1..].parse::<u8>() {
                    Ok(n @ 1..=12) => Key::F(n),
                    _ => return Err(bad("unsupported key")),
                },
                _ => return Err(bad("unsupported key (use A-Z, 0-9, Space or F1-F12)")),
            });
        }
        spec.key = key.ok_or_else(|| bad("missing key"))?;
        if !(spec.ctrl || spec.alt || spec.super_key) && !matches!(spec.key, Key::F(_)) {
            return Err(bad(
                "include Ctrl, Alt or Super so normal typing is not intercepted",
            ));
        }
        Ok(spec)
    }

    pub fn key_name(&self) -> String {
        match self.key {
            Key::Letter(c) | Key::Digit(c) => c.to_string(),
            Key::Space => "Space".into(),
            Key::F(n) => format!("F{n}"),
        }
    }

    /// Canonical display form, e.g. "Ctrl+Shift+Space".
    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.super_key {
            parts.push("Super".to_string());
        }
        parts.push(self.key_name());
        parts.join("+")
    }

    #[cfg(target_os = "linux")]
    fn hyprland_modmask(&self) -> u32 {
        use platform::hyprland::*;
        (if self.ctrl { MOD_CTRL } else { 0 })
            | (if self.shift { MOD_SHIFT } else { 0 })
            | (if self.alt { MOD_ALT } else { 0 })
            | (if self.super_key { MOD_SUPER } else { 0 })
    }

    #[cfg(target_os = "linux")]
    fn hyprland_key(&self) -> String {
        match self.key {
            Key::Space => "space".into(),
            _ => self.key_name(),
        }
    }

    fn plugin_shortcut(&self) -> tauri_plugin_global_shortcut::Shortcut {
        use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
        let mut mods = Modifiers::empty();
        if self.ctrl {
            mods |= Modifiers::CONTROL;
        }
        if self.shift {
            mods |= Modifiers::SHIFT;
        }
        if self.alt {
            mods |= Modifiers::ALT;
        }
        if self.super_key {
            mods |= Modifiers::SUPER;
        }
        let code = match self.key {
            Key::Space => Code::Space,
            Key::F(n) => [
                Code::F1,
                Code::F2,
                Code::F3,
                Code::F4,
                Code::F5,
                Code::F6,
                Code::F7,
                Code::F8,
                Code::F9,
                Code::F10,
                Code::F11,
                Code::F12,
            ][usize::from(n - 1)],
            Key::Digit(c) => [
                Code::Digit0,
                Code::Digit1,
                Code::Digit2,
                Code::Digit3,
                Code::Digit4,
                Code::Digit5,
                Code::Digit6,
                Code::Digit7,
                Code::Digit8,
                Code::Digit9,
            ][usize::from(c as u8 - b'0')],
            Key::Letter(c) => [
                Code::KeyA,
                Code::KeyB,
                Code::KeyC,
                Code::KeyD,
                Code::KeyE,
                Code::KeyF,
                Code::KeyG,
                Code::KeyH,
                Code::KeyI,
                Code::KeyJ,
                Code::KeyK,
                Code::KeyL,
                Code::KeyM,
                Code::KeyN,
                Code::KeyO,
                Code::KeyP,
                Code::KeyQ,
                Code::KeyR,
                Code::KeyS,
                Code::KeyT,
                Code::KeyU,
                Code::KeyV,
                Code::KeyW,
                Code::KeyX,
                Code::KeyY,
                Code::KeyZ,
            ][usize::from(c as u8 - b'A')],
        };
        Shortcut::new(Some(mods), code)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShortcutBackend {
    GlobalHotkey,
    HyprlandBinding,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    pub shortcut: String,
    pub backend: ShortcutBackend,
    pub registered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Default)]
pub struct ShortcutManager {
    current: Mutex<Option<(ShortcutSpec, ShortcutBackend)>>,
    status: Mutex<Option<ShortcutStatus>>,
}

fn backend_for_platform() -> ShortcutBackend {
    match platform::display_server() {
        DisplayServer::Wayland if platform::is_hyprland() => ShortcutBackend::HyprlandBinding,
        DisplayServer::Wayland | DisplayServer::Unknown => ShortcutBackend::Unsupported,
        DisplayServer::Windows | DisplayServer::X11 => ShortcutBackend::GlobalHotkey,
    }
}

impl ShortcutManager {
    pub fn status(&self) -> Option<ShortcutStatus> {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Replace the current registration with `input`. On failure the old
    /// shortcut is restored where possible and the error is returned.
    pub fn register<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        input: &str,
    ) -> HaloResult<ShortcutStatus> {
        let spec = ShortcutSpec::parse(input)?;
        let previous = self
            .current
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some((old, backend)) = previous
            && let Err(e) = unregister_spec(app, &old, backend)
        {
            log::warn!("could not unregister previous shortcut: {e}");
        }
        let backend = backend_for_platform();
        let result = register_spec(app, &spec, backend);
        let status = match &result {
            Ok(()) => {
                *self.current.lock().unwrap_or_else(|p| p.into_inner()) = Some((spec, backend));
                ShortcutStatus {
                    shortcut: spec.display(),
                    backend,
                    registered: backend != ShortcutBackend::Unsupported,
                    message: (backend == ShortcutBackend::Unsupported).then(|| {
                        "This Wayland compositor does not let apps register global shortcuts. \
                         Bind your shortcut to run `halo --activate` in the compositor settings."
                            .to_string()
                    }),
                }
            }
            Err(e) => {
                // Put the previous shortcut back so the user is not left
                // without one.
                if let Some((old, old_backend)) = previous
                    && register_spec(app, &old, old_backend).is_ok()
                {
                    *self.current.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some((old, old_backend));
                }
                ShortcutStatus {
                    shortcut: spec.display(),
                    backend,
                    registered: false,
                    message: Some(e.message.clone()),
                }
            }
        };
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(status.clone());
        result.map(|_| status)
    }

    pub fn unregister_all<R: Runtime>(&self, app: &AppHandle<R>) {
        if let Some((spec, backend)) = self
            .current
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            && let Err(e) = unregister_spec(app, &spec, backend)
        {
            log::warn!("shortcut cleanup failed: {e}");
        }
    }
}

fn register_spec<R: Runtime>(
    app: &AppHandle<R>,
    spec: &ShortcutSpec,
    backend: ShortcutBackend,
) -> HaloResult<()> {
    match backend {
        ShortcutBackend::GlobalHotkey => {
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            app.global_shortcut().register(spec.plugin_shortcut()).map_err(|e| {
                HaloError::new(
                    ErrorKind::Shortcut,
                    format!(
                        "Could not register {} ({e}). Another application may already use it; choose a different shortcut in Settings.",
                        spec.display()
                    ),
                )
            })
        }
        ShortcutBackend::HyprlandBinding => {
            #[cfg(target_os = "linux")]
            {
                let exe = std::env::current_exe().map_err(|e| {
                    HaloError::new(
                        ErrorKind::Shortcut,
                        format!("Cannot locate the Halo executable: {e}"),
                    )
                })?;
                let _ = app;
                platform::hyprland::bind(
                    spec.hyprland_modmask(),
                    &spec.hyprland_key(),
                    &exe.to_string_lossy(),
                )
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = app;
                Ok(())
            }
        }
        ShortcutBackend::Unsupported => Ok(()),
    }
}

fn unregister_spec<R: Runtime>(
    app: &AppHandle<R>,
    spec: &ShortcutSpec,
    backend: ShortcutBackend,
) -> HaloResult<()> {
    match backend {
        ShortcutBackend::GlobalHotkey => {
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            app.global_shortcut()
                .unregister(spec.plugin_shortcut())
                .map_err(|e| HaloError::new(ErrorKind::Shortcut, e.to_string()))
        }
        ShortcutBackend::HyprlandBinding => {
            #[cfg(target_os = "linux")]
            {
                platform::hyprland::unbind(spec.hyprland_modmask(), &spec.hyprland_key())
            }
            #[cfg(not(target_os = "linux"))]
            Ok(())
        }
        ShortcutBackend::Unsupported => Ok(()),
    }
}

/// Whether the OS hotkey plugin should be initialised on this platform.
pub fn needs_hotkey_plugin() -> bool {
    backend_for_platform() == ShortcutBackend::GlobalHotkey
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_shortcut() {
        let s = ShortcutSpec::parse("Ctrl+Shift+Space").unwrap();
        assert!(s.ctrl && s.shift && !s.alt && !s.super_key);
        assert_eq!(s.key, Key::Space);
        assert_eq!(s.display(), "Ctrl+Shift+Space");
    }

    #[test]
    fn parsing_is_case_and_space_insensitive() {
        let s = ShortcutSpec::parse(" control + alt + h ").unwrap();
        assert_eq!(s.display(), "Ctrl+Alt+H");
        assert_eq!(ShortcutSpec::parse("super+F5").unwrap().key, Key::F(5));
        assert_eq!(ShortcutSpec::parse("F9").unwrap().display(), "F9");
        assert_eq!(ShortcutSpec::parse("Ctrl+7").unwrap().key, Key::Digit('7'));
    }

    #[test]
    fn rejects_bad_shortcuts() {
        for bad in [
            "",
            "Ctrl+",
            "Ctrl+Shift",
            "Ctrl+A+B",
            "Ctrl+Ctrl+A",
            "Shift+A",
            "A",
            "Ctrl+F13",
            "Ctrl+Enter",
            "Ctrl+é",
        ] {
            assert!(
                ShortcutSpec::parse(bad).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn converts_to_plugin_shortcut() {
        use tauri_plugin_global_shortcut::{Code, Modifiers};
        let s = ShortcutSpec::parse("Ctrl+Shift+Space")
            .unwrap()
            .plugin_shortcut();
        assert!(s.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::Space));
        let s = ShortcutSpec::parse("Alt+Z").unwrap().plugin_shortcut();
        assert!(s.matches(Modifiers::ALT, Code::KeyZ));
        let s = ShortcutSpec::parse("Ctrl+F12").unwrap().plugin_shortcut();
        assert!(s.matches(Modifiers::CONTROL, Code::F12));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn converts_to_hyprland_binding() {
        let s = ShortcutSpec::parse("Ctrl+Shift+Space").unwrap();
        assert_eq!(s.hyprland_modmask(), 5);
        assert_eq!(s.hyprland_key(), "space");
    }
}
