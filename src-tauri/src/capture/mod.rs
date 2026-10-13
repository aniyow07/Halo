//! Real screen capture.
//!
//! Halo captures the whole target monitor *before* any Halo window is shown
//! on it (freeze-frame), then lets the user select on top of that frozen
//! image. The overlay and selection outline therefore can never end up in
//! the captured pixels, and the crop is taken from exactly the pixels the
//! user saw.
//!
//! Backends:
//! - Wayland (wlroots-protocol compositors such as Hyprland, Sway):
//!   `zwlr_screencopy_v1` via `libwayshot`, at native output resolution.
//! - X11: core-protocol `GetImage` on the root window via `x11rb`, with
//!   monitor geometry from RandR.
//! - Windows: `xcap` (GDI), physical pixels with per-monitor DPI awareness.

use std::time::Instant;

use image::RgbaImage;
use serde::Serialize;

use crate::errors::{ErrorKind, HaloError, HaloResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureInfo {
    pub backend: &'static str,
    pub monitor_name: String,
    /// Monitor position in the desktop layout (physical pixels where the
    /// platform exposes them, otherwise compositor layout units).
    pub monitor_x: i32,
    pub monitor_y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub duration_ms: f64,
}

#[cfg(test)]
impl CaptureInfo {
    pub fn test_info(width: u32, height: u32) -> Self {
        Self {
            backend: "test",
            monitor_name: "test".into(),
            monitor_x: 0,
            monitor_y: 0,
            width,
            height,
            scale_factor: 1.0,
            duration_ms: 0.0,
        }
    }
}

pub struct Captured {
    pub image: RgbaImage,
    pub info: CaptureInfo,
}

/// Which monitor to capture, described in a platform-neutral way.
#[derive(Debug, Clone, Default)]
pub struct MonitorHint {
    /// Output / monitor name (e.g. "eDP-1").
    pub name: Option<String>,
    /// Physical top-left of the monitor in desktop coordinates.
    pub physical_position: Option<(i32, i32)>,
}

fn capture_error(message: impl Into<String>) -> HaloError {
    HaloError::new(ErrorKind::Capture, message)
}

fn validate(image: &RgbaImage) -> HaloResult<()> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Err(capture_error("The screen capture returned an empty image."));
    }
    Ok(())
}

/// Capture the monitor described by `hint`. Blocking; call from a worker
/// thread.
pub fn capture_monitor(hint: &MonitorHint) -> HaloResult<Captured> {
    let started = Instant::now();
    #[cfg(target_os = "linux")]
    let result = if crate::platform::display_server() == crate::platform::DisplayServer::Wayland {
        wayland::capture(hint)
    } else {
        x11::capture(hint)
    };
    #[cfg(windows)]
    let result = xcap_backend::capture(hint);
    #[cfg(not(any(target_os = "linux", windows)))]
    let result: HaloResult<Captured> = {
        let _ = hint;
        Err(capture_error(
            "Screen capture is not implemented on this platform.",
        ))
    };

    let mut captured = result?;
    validate(&captured.image)?;
    captured.info.duration_ms = (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    Ok(captured)
}

#[cfg(target_os = "linux")]
mod wayland {
    use super::*;
    use libwayshot::WayshotConnection;

    pub fn capture(hint: &MonitorHint) -> HaloResult<Captured> {
        let conn = WayshotConnection::new().map_err(|e| {
            capture_error(format!(
                "Could not connect to the Wayland compositor for screen capture ({e}). \
                 Halo needs a compositor that supports wlr-screencopy (e.g. Hyprland, Sway)."
            ))
        })?;
        let outputs = conn.get_all_outputs();
        if outputs.is_empty() {
            return Err(capture_error(
                "The compositor reported no outputs to capture.",
            ));
        }
        let output = match &hint.name {
            Some(name) => outputs.iter().find(|o| &o.name == name).ok_or_else(|| {
                capture_error(format!("Monitor \"{name}\" is not available for capture."))
            })?,
            None => &outputs[0],
        };
        let image = conn
            .screenshot_single_output(output, false)
            .map_err(|e| {
                capture_error(format!(
                    "Screen capture failed on {} ({e}). The compositor may not support wlr-screencopy; \
                     GNOME and KDE Wayland sessions are not supported yet.",
                    output.name
                ))
            })?
            .to_rgba8();
        let region = output.logical_region.inner;
        let scale = if region.size.width > 0 {
            f64::from(image.width()) / f64::from(region.size.width)
        } else {
            1.0
        };
        Ok(Captured {
            info: CaptureInfo {
                backend: "wayland-wlr-screencopy",
                monitor_name: output.name.clone(),
                monitor_x: region.position.x,
                monitor_y: region.position.y,
                width: image.width(),
                height: image.height(),
                scale_factor: (scale * 100.0).round() / 100.0,
                duration_ms: 0.0,
            },
            image,
        })
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

    struct Mon {
        name: String,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        primary: bool,
    }

    pub fn capture(hint: &MonitorHint) -> HaloResult<Captured> {
        let err = |what: &str, e: &dyn std::fmt::Display| {
            capture_error(format!("X11 {what} failed: {e}"))
        };
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| err("connection", &e))?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;

        let monitors: Vec<Mon> = conn
            .randr_get_monitors(root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .map(|m| Mon {
                        name: conn
                            .get_atom_name(m.name)
                            .ok()
                            .and_then(|c| c.reply().ok())
                            .map(|n| String::from_utf8_lossy(&n.name).into_owned())
                            .unwrap_or_default(),
                        x: m.x,
                        y: m.y,
                        width: m.width,
                        height: m.height,
                        primary: m.primary,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let whole = Mon {
            name: "screen".into(),
            x: 0,
            y: 0,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            primary: true,
        };
        let target = hint
            .physical_position
            .and_then(|(px, py)| {
                monitors
                    .iter()
                    .find(|m| i32::from(m.x) == px && i32::from(m.y) == py)
            })
            .or_else(|| {
                hint.name
                    .as_ref()
                    .and_then(|n| monitors.iter().find(|m| &m.name == n))
            })
            .or_else(|| monitors.iter().find(|m| m.primary))
            .or(monitors.first())
            .unwrap_or(&whole);

        let reply = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                root,
                target.x,
                target.y,
                target.width,
                target.height,
                !0,
            )
            .map_err(|e| err("GetImage", &e))?
            .reply()
            .map_err(|e| err("GetImage", &e))?;
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == reply.depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(0);
        if bpp != 32 || !(reply.depth == 24 || reply.depth == 32) {
            return Err(capture_error(format!(
                "Unsupported X11 pixel format (depth {}, {bpp} bpp).",
                reply.depth
            )));
        }
        let (w, h) = (u32::from(target.width), u32::from(target.height));
        let expected = (w * h * 4) as usize;
        if reply.data.len() < expected {
            return Err(capture_error("X11 returned a truncated image."));
        }
        let lsb = setup.image_byte_order == ImageOrder::LSB_FIRST;
        let mut rgba = Vec::with_capacity(expected);
        for px in reply.data[..expected].as_chunks::<4>().0 {
            let (r, g, b) = if lsb {
                (px[2], px[1], px[0])
            } else {
                (px[1], px[2], px[3])
            };
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        let image = RgbaImage::from_raw(w, h, rgba)
            .ok_or_else(|| capture_error("X11 image size mismatch."))?;
        Ok(Captured {
            info: CaptureInfo {
                backend: "x11-getimage",
                monitor_name: target.name.clone(),
                monitor_x: i32::from(target.x),
                monitor_y: i32::from(target.y),
                width: w,
                height: h,
                scale_factor: 1.0,
                duration_ms: 0.0,
            },
            image,
        })
    }
}

#[cfg(windows)]
mod xcap_backend {
    use super::*;
    use xcap::Monitor;

    fn physical_origin(m: &Monitor) -> Option<(i32, i32, f32)> {
        let scale = m.scale_factor().ok()?;
        let x = m.x().ok()?;
        let y = m.y().ok()?;
        Some((x, y, scale))
    }

    pub fn capture(hint: &MonitorHint) -> HaloResult<Captured> {
        let monitors =
            Monitor::all().map_err(|e| capture_error(format!("Could not list monitors: {e}")))?;
        if monitors.is_empty() {
            return Err(capture_error("No monitors found."));
        }
        // Match the monitor by its position (xcap reports logical positions
        // on some platforms and physical on others, so accept either).
        let by_position = hint.physical_position.and_then(|(px, py)| {
            monitors.iter().find(|m| {
                physical_origin(m).is_some_and(|(x, y, s)| {
                    (x == px && y == py)
                        || ((x as f32 * s).round() as i32 == px
                            && (y as f32 * s).round() as i32 == py)
                })
            })
        });
        let by_name = || {
            hint.name
                .as_ref()
                .and_then(|n| monitors.iter().find(|m| m.name().is_ok_and(|mn| &mn == n)))
        };
        let primary = || monitors.iter().find(|m| m.is_primary().unwrap_or(false));
        let monitor = by_position
            .or_else(by_name)
            .or_else(primary)
            .unwrap_or(&monitors[0]);

        let image = monitor
            .capture_image()
            .map_err(|e| capture_error(format!("Screen capture failed: {e}")))?;
        let (x, y, scale) = physical_origin(monitor).unwrap_or((0, 0, 1.0));
        Ok(Captured {
            info: CaptureInfo {
                backend: "windows-gdi (xcap)",
                monitor_name: monitor.name().unwrap_or_default(),
                monitor_x: x,
                monitor_y: y,
                width: image.width(),
                height: image.height(),
                scale_factor: f64::from(scale),
                duration_ms: 0.0,
            },
            image,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_capture() {
        assert!(validate(&RgbaImage::new(0, 10)).is_err());
        assert!(validate(&RgbaImage::new(10, 10)).is_ok());
    }
}
