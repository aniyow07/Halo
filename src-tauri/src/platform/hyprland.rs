//! Hyprland IPC (the `hyprctl` socket protocol), used for:
//! - finding the focused monitor (Wayland gives clients no cursor position);
//! - registering the global shortcut as a compositor keybinding, since
//!   Wayland does not let applications grab keys globally.
//!
//! Bindings added over IPC live only in the running compositor; they are
//! removed again on shutdown and never written to the user's config.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::errors::{ErrorKind, HaloError, HaloResult};

fn socket_path() -> HaloResult<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| HaloError::new(ErrorKind::Shortcut, "Not running under Hyprland."))?;
    if sig.contains('/') || sig.contains("..") {
        return Err(HaloError::new(
            ErrorKind::Shortcut,
            "Invalid HYPRLAND_INSTANCE_SIGNATURE.",
        ));
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let modern = PathBuf::from(runtime)
        .join("hypr")
        .join(&sig)
        .join(".socket.sock");
    if modern.exists() {
        return Ok(modern);
    }
    Ok(PathBuf::from("/tmp/hypr").join(sig).join(".socket.sock"))
}

pub fn request(command: &str) -> HaloResult<String> {
    let err = |e: std::io::Error| {
        HaloError::new(ErrorKind::Shortcut, format!("Hyprland IPC failed: {e}"))
    };
    let mut stream = UnixStream::connect(socket_path()?).map_err(err)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(err)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(err)?;
    stream.write_all(command.as_bytes()).map_err(err)?;
    let mut out = String::new();
    stream.read_to_string(&mut out).map_err(err)?;
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub focused: bool,
}

pub fn monitors() -> HaloResult<Vec<Monitor>> {
    let json = request("j/monitors")?;
    serde_json::from_str(&json).map_err(|e| {
        HaloError::new(
            ErrorKind::Capture,
            format!("Unexpected Hyprland monitor data: {e}"),
        )
    })
}

pub fn focused_monitor() -> HaloResult<Monitor> {
    let all = monitors()?;
    all.iter()
        .find(|m| m.focused)
        .or(all.first())
        .cloned()
        .ok_or_else(|| HaloError::new(ErrorKind::Capture, "Hyprland reports no monitors."))
}

#[derive(Debug, Clone, Deserialize)]
struct Bind {
    modmask: u32,
    key: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    arg: String,
    #[serde(default)]
    submap: String,
}

/// Hyprland modmask bits.
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 64;

const BIND_DESCRIPTION: &str = "Halo: select screen region";

fn mods_string(modmask: u32) -> String {
    let mut parts = Vec::new();
    for (bit, name) in [
        (MOD_SUPER, "SUPER"),
        (MOD_CTRL, "CTRL"),
        (MOD_ALT, "ALT"),
        (MOD_SHIFT, "SHIFT"),
    ] {
        if modmask & bit != 0 {
            parts.push(name);
        }
    }
    parts.join(" ")
}

/// Find an existing binding (not ours) for the same chord.
fn binds_for(modmask: u32, key: &str) -> HaloResult<Vec<Bind>> {
    let json = request("j/binds")?;
    let binds: Vec<Bind> = serde_json::from_str(&json).map_err(|e| {
        HaloError::new(
            ErrorKind::Shortcut,
            format!("Unexpected Hyprland binds data: {e}"),
        )
    })?;
    Ok(binds
        .into_iter()
        .filter(|b| b.submap.is_empty() && b.modmask == modmask && b.key.eq_ignore_ascii_case(key))
        .collect())
}

fn is_ours(b: &Bind) -> bool {
    b.description == BIND_DESCRIPTION || b.arg.contains("--activate")
}

/// Find an existing binding that is not Halo's for the same chord.
pub fn find_conflict(modmask: u32, key: &str) -> HaloResult<Option<String>> {
    Ok(binds_for(modmask, key)?
        .into_iter()
        .find(|b| !is_ours(b))
        .map(|b| {
            if b.description.is_empty() {
                b.arg
            } else {
                b.description
            }
        }))
}

/// Remove bindings a previous Halo process left behind (e.g. if it was
/// killed before it could clean up), so a key press activates only once.
fn remove_stale(modmask: u32, key: &str) -> HaloResult<()> {
    for _ in 0..32 {
        if !binds_for(modmask, key)?.iter().any(is_ours) {
            return Ok(());
        }
        unbind(modmask, key)?;
    }
    Err(HaloError::new(
        ErrorKind::Shortcut,
        "Could not remove stale Halo keybindings.",
    ))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn ok(reply: &str) -> bool {
    reply.trim().eq_ignore_ascii_case("ok")
}

/// Register `<exe> --activate` as a compositor keybinding. Supports both the
/// legacy `keyword bind` config and Lua configs (Hyprland >= 0.56, where
/// `keyword` is rejected and `eval` is used instead).
pub fn bind(modmask: u32, key: &str, exe: &str) -> HaloResult<()> {
    if let Some(existing) = find_conflict(modmask, key)? {
        return Err(HaloError::new(
            ErrorKind::Shortcut,
            format!(
                "{} + {key} is already bound in Hyprland ({existing}). Choose another shortcut in Settings.",
                mods_string(modmask)
            ),
        ));
    }
    // Only Halo's own bindings exist for this chord at this point, so
    // unbinding the chord cannot remove a user binding.
    remove_stale(modmask, key)?;
    let command = format!("{} --activate", shell_quote(exe));
    let mods = mods_string(modmask);

    let legacy = request(&format!("keyword bind {mods},{key},exec,{command}"))?;
    if ok(&legacy) {
        return Ok(());
    }
    let chord = if mods.is_empty() {
        key.to_string()
    } else {
        format!("{} + {key}", mods.replace(' ', " + "))
    };
    let lua = format!(
        "eval hl.bind({}, hl.dsp.exec_cmd({}), {{ description = {} }})",
        lua_string(&chord),
        lua_string(&command),
        lua_string(BIND_DESCRIPTION)
    );
    let reply = request(&lua)?;
    if ok(&reply) {
        Ok(())
    } else {
        Err(HaloError::new(
            ErrorKind::Shortcut,
            format!(
                "Hyprland refused the keybinding: {} / {}",
                legacy.trim(),
                reply.trim()
            ),
        ))
    }
}

pub fn unbind(modmask: u32, key: &str) -> HaloResult<()> {
    let mods = mods_string(modmask);
    let legacy = request(&format!("keyword unbind {mods},{key}"))?;
    if ok(&legacy) {
        return Ok(());
    }
    let chord = if mods.is_empty() {
        key.to_string()
    } else {
        format!("{} + {key}", mods.replace(' ', " + "))
    };
    let reply = request(&format!("eval hl.unbind({})", lua_string(&chord)))?;
    if ok(&reply) {
        Ok(())
    } else {
        Err(HaloError::new(
            ErrorKind::Shortcut,
            format!("Could not remove the keybinding: {}", reply.trim()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_is_safe() {
        assert_eq!(shell_quote("/opt/it's/halo"), r"'/opt/it'\''s/halo'");
        assert_eq!(lua_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(mods_string(MOD_CTRL | MOD_SHIFT), "CTRL SHIFT");
        assert_eq!(mods_string(MOD_SUPER | MOD_ALT), "SUPER ALT");
    }
}
