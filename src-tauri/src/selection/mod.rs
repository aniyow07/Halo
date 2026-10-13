//! Selection geometry.
//!
//! The overlay reports a rectangle in CSS pixels relative to its own
//! viewport. The overlay covers exactly one monitor and shows the frozen
//! frame of that monitor stretched to the viewport, so CSS pixels map to
//! frame pixels by a per-axis ratio. That ratio absorbs fractional display
//! scaling (e.g. 1.6x) without needing to know the scale factor itself.

use serde::Deserialize;

use crate::errors::{ErrorKind, HaloError, HaloResult};

/// Smallest selection (in frame pixels) we accept. Anything smaller is
/// almost certainly an accidental click rather than a deliberate region.
pub const MIN_SELECTION_PX: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// A validated region in frame (physical) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Convert an overlay selection into frame pixels, clamping to the frame
/// edges. Rejects non-finite, empty, inverted, fully off-screen or too-small
/// selections.
pub fn to_frame_pixels(
    rect: CssRect,
    viewport: Viewport,
    frame_width: u32,
    frame_height: u32,
) -> HaloResult<PixelRect> {
    let invalid = |msg: &str| HaloError::new(ErrorKind::InvalidSelection, msg.to_string());

    if !finite(&[
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        viewport.width,
        viewport.height,
    ]) {
        return Err(invalid("Selection contains non-numeric coordinates."));
    }
    if viewport.width <= 0.0 || viewport.height <= 0.0 {
        return Err(invalid("Overlay viewport has no size."));
    }
    if frame_width == 0 || frame_height == 0 {
        return Err(invalid("Captured frame is empty."));
    }
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(invalid("Selection is empty. Drag to select a region."));
    }

    let sx = f64::from(frame_width) / viewport.width;
    let sy = f64::from(frame_height) / viewport.height;

    let left = (rect.x * sx).floor().clamp(0.0, f64::from(frame_width));
    let top = (rect.y * sy).floor().clamp(0.0, f64::from(frame_height));
    let right = ((rect.x + rect.width) * sx)
        .ceil()
        .clamp(0.0, f64::from(frame_width));
    let bottom = ((rect.y + rect.height) * sy)
        .ceil()
        .clamp(0.0, f64::from(frame_height));

    if right <= left || bottom <= top {
        return Err(invalid("Selection lies outside the screen."));
    }

    // Values are clamped to [0, frame dimension], so the casts are lossless.
    let out = PixelRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    };

    if out.width < MIN_SELECTION_PX || out.height < MIN_SELECTION_PX {
        return Err(invalid(&format!(
            "Selection is too small ({}x{} px). Select at least {MIN_SELECTION_PX}x{MIN_SELECTION_PX} px.",
            out.width, out.height
        )));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(w: f64, h: f64) -> Viewport {
        Viewport {
            width: w,
            height: h,
        }
    }

    fn r(x: f64, y: f64, w: f64, h: f64) -> CssRect {
        CssRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn identity_mapping_at_scale_one() {
        let p =
            to_frame_pixels(r(10.0, 20.0, 100.0, 50.0), vp(1920.0, 1080.0), 1920, 1080).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 10,
                y: 20,
                width: 100,
                height: 50
            }
        );
    }

    #[test]
    fn fractional_scaling_maps_to_physical_pixels() {
        // 2560x1600 panel at 1.6x scale => 1600x1000 CSS viewport.
        let p =
            to_frame_pixels(r(100.0, 50.0, 200.0, 100.0), vp(1600.0, 1000.0), 2560, 1600).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 160,
                y: 80,
                width: 320,
                height: 160
            }
        );
    }

    #[test]
    fn fractional_css_coordinates_expand_outward() {
        let p = to_frame_pixels(r(10.4, 10.6, 20.2, 20.2), vp(100.0, 100.0), 100, 100).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 10,
                y: 10,
                width: 21,
                height: 21
            }
        );
    }

    #[test]
    fn clamps_selection_crossing_screen_edges() {
        let p = to_frame_pixels(r(-50.0, -10.0, 100.0, 60.0), vp(200.0, 100.0), 200, 100).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 0,
                y: 0,
                width: 50,
                height: 50
            }
        );
        let p = to_frame_pixels(r(180.0, 80.0, 100.0, 100.0), vp(200.0, 100.0), 200, 100).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 180,
                y: 80,
                width: 20,
                height: 20
            }
        );
    }

    #[test]
    fn full_screen_selection() {
        let p =
            to_frame_pixels(r(0.0, 0.0, 1600.0, 1000.0), vp(1600.0, 1000.0), 2560, 1600).unwrap();
        assert_eq!(
            p,
            PixelRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1600
            }
        );
    }

    #[test]
    fn rejects_off_screen_selection() {
        let err =
            to_frame_pixels(r(300.0, 300.0, 50.0, 50.0), vp(200.0, 100.0), 200, 100).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidSelection);
    }

    #[test]
    fn rejects_empty_and_negative_sizes() {
        assert!(to_frame_pixels(r(0.0, 0.0, 0.0, 10.0), vp(100.0, 100.0), 100, 100).is_err());
        assert!(to_frame_pixels(r(50.0, 50.0, -20.0, 20.0), vp(100.0, 100.0), 100, 100).is_err());
    }

    #[test]
    fn rejects_tiny_selection() {
        let err = to_frame_pixels(r(0.0, 0.0, 3.0, 3.0), vp(100.0, 100.0), 100, 100).unwrap_err();
        assert!(err.message.contains("too small"));
    }

    #[test]
    fn rejects_non_finite_values_and_bad_viewport() {
        assert!(to_frame_pixels(r(f64::NAN, 0.0, 10.0, 10.0), vp(100.0, 100.0), 100, 100).is_err());
        assert!(
            to_frame_pixels(r(0.0, 0.0, f64::INFINITY, 10.0), vp(100.0, 100.0), 100, 100).is_err()
        );
        assert!(to_frame_pixels(r(0.0, 0.0, 10.0, 10.0), vp(0.0, 100.0), 100, 100).is_err());
        assert!(to_frame_pixels(r(0.0, 0.0, 10.0, 10.0), vp(100.0, 100.0), 0, 100).is_err());
    }
}
