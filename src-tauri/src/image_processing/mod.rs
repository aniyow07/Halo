//! Crop, resize and encode the selected region for Gemini.
//!
//! The baseline ("original encoded size") is the selected region encoded as
//! a lossless PNG at its captured resolution: the payload a naive
//! implementation would send. The processed payload is what Halo actually
//! sends. Both are measured, never estimated.

use std::time::Instant;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::errors::{ErrorKind, HaloError, HaloResult};
use crate::selection::PixelRect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EncodingPreference {
    /// Encode as lossless PNG, lossless WebP and JPEG; send the smallest.
    Auto,
    Png,
    Jpeg,
    /// Lossless WebP (supported by Gemini as `image/webp`).
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessingOptions {
    /// Longest edge after resizing. Images at or below this are not resized.
    pub max_dimension: u32,
    /// JPEG quality, 1-100.
    pub jpeg_quality: u8,
    pub encoding: EncodingPreference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn mime_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingStats {
    pub original_width: u32,
    pub original_height: u32,
    pub processed_width: u32,
    pub processed_height: u32,
    pub original_bytes: usize,
    pub processed_bytes: usize,
    pub reduction_percent: f64,
    pub duration_ms: f64,
    pub format: ImageFormat,
}

#[derive(Debug, Clone)]
pub struct ProcessedImage {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    pub stats: ProcessingStats,
}

fn processing_error(message: impl Into<String>) -> HaloError {
    HaloError::new(ErrorKind::ImageProcessing, message)
}

/// Percentage by which `processed` is smaller than `original`.
/// Negative when the processed payload is larger.
pub fn reduction_percent(original: usize, processed: usize) -> f64 {
    if original == 0 {
        return 0.0;
    }
    let pct = (1.0 - processed as f64 / original as f64) * 100.0;
    (pct * 10.0).round() / 10.0
}

/// Dimensions after fitting the longest edge into `max_dimension`,
/// preserving aspect ratio. Never upscales, never returns a zero edge.
pub fn fit_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let longest = width.max(height);
    if max_dimension == 0 || longest <= max_dimension {
        return (width, height);
    }
    let scale = f64::from(max_dimension) / f64::from(longest);
    let w = ((f64::from(width) * scale).round() as u32).clamp(1, max_dimension);
    let h = ((f64::from(height) * scale).round() as u32).clamp(1, max_dimension);
    (w, h)
}

pub fn crop(frame: &RgbaImage, rect: PixelRect) -> HaloResult<RgbaImage> {
    let (fw, fh) = frame.dimensions();
    if rect.width == 0 || rect.height == 0 {
        return Err(processing_error("Cannot crop an empty region."));
    }
    let fits = rect.x.checked_add(rect.width).is_some_and(|r| r <= fw)
        && rect.y.checked_add(rect.height).is_some_and(|b| b <= fh);
    if !fits {
        return Err(processing_error(format!(
            "Crop region {}x{}+{}+{} exceeds frame {fw}x{fh}.",
            rect.width, rect.height, rect.x, rect.y
        )));
    }
    Ok(image::imageops::crop_imm(frame, rect.x, rect.y, rect.width, rect.height).to_image())
}

pub fn encode_png(image: &DynamicImage) -> HaloResult<Vec<u8>> {
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, PngFilter::Adaptive);
    let rgb;
    let (bytes, color): (&[u8], _) = if image.color().has_alpha() {
        (image.as_bytes(), image.color())
    } else {
        rgb = image.to_rgb8();
        (rgb.as_raw(), image::ColorType::Rgb8)
    };
    encoder
        .write_image(bytes, image.width(), image.height(), color.into())
        .map_err(|e| processing_error(format!("PNG encoding failed: {e}")))?;
    Ok(out)
}

pub fn encode_jpeg(image: &RgbImage, quality: u8) -> HaloResult<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| processing_error(format!("JPEG encoding failed: {e}")))?;
    Ok(out)
}

/// Lossless WebP: typically noticeably smaller than PNG for screenshots,
/// with identical pixels.
pub fn encode_webp_lossless(image: &RgbImage) -> HaloResult<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| processing_error(format!("WebP encoding failed: {e}")))?;
    Ok(out)
}

/// Screen pixels are opaque; drop alpha so encoders don't waste bytes on it.
fn flatten_to_rgb(image: &RgbaImage) -> RgbImage {
    DynamicImage::ImageRgba8(image.clone()).to_rgb8()
}

/// Full pipeline: crop the selected region out of the frozen frame, resize
/// if needed, and encode.
pub fn process_selection(
    frame: &RgbaImage,
    rect: PixelRect,
    options: ProcessingOptions,
) -> HaloResult<ProcessedImage> {
    let started = Instant::now();
    let cropped = crop(frame, rect)?;
    process_image(&cropped, options, started)
}

pub fn process_image(
    cropped: &RgbaImage,
    options: ProcessingOptions,
    started: Instant,
) -> HaloResult<ProcessedImage> {
    let (ow, oh) = cropped.dimensions();
    if ow == 0 || oh == 0 {
        return Err(processing_error("Image has no pixels."));
    }
    let rgb = flatten_to_rgb(cropped);
    let original_png = encode_png(&DynamicImage::ImageRgb8(rgb.clone()))?;

    let (pw, ph) = fit_dimensions(ow, oh, options.max_dimension);
    let resized = if (pw, ph) == (ow, oh) {
        rgb
    } else {
        // Lanczos keeps thin strokes (text, code, chart lines) crisp when
        // downscaling.
        image::imageops::resize(&rgb, pw, ph, FilterType::Lanczos3)
    };

    let same_size = (pw, ph) == (ow, oh);
    let png = |img: &RgbImage| -> HaloResult<Vec<u8>> {
        if same_size {
            Ok(original_png.clone())
        } else {
            encode_png(&DynamicImage::ImageRgb8(img.clone()))
        }
    };
    let (bytes, format) = match options.encoding {
        EncodingPreference::Png => (png(&resized)?, ImageFormat::Png),
        EncodingPreference::Jpeg => (
            encode_jpeg(&resized, options.jpeg_quality)?,
            ImageFormat::Jpeg,
        ),
        EncodingPreference::Webp => (encode_webp_lossless(&resized)?, ImageFormat::Webp),
        EncodingPreference::Auto => [
            (png(&resized)?, ImageFormat::Png),
            (encode_webp_lossless(&resized)?, ImageFormat::Webp),
            (
                encode_jpeg(&resized, options.jpeg_quality)?,
                ImageFormat::Jpeg,
            ),
        ]
        .into_iter()
        .min_by_key(|(bytes, _)| bytes.len())
        .expect("three candidates"),
    };

    let stats = ProcessingStats {
        original_width: ow,
        original_height: oh,
        processed_width: pw,
        processed_height: ph,
        original_bytes: original_png.len(),
        processed_bytes: bytes.len(),
        reduction_percent: reduction_percent(original_png.len(), bytes.len()),
        duration_ms: (started.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0,
        format,
    };
    Ok(ProcessedImage {
        bytes,
        format,
        stats,
    })
}

/// Decode and validate an encoded image (verifies our own output in tests).
#[cfg(test)]
pub fn decode(bytes: &[u8]) -> HaloResult<DynamicImage> {
    if bytes.is_empty() {
        return Err(processing_error("Image data is empty."));
    }
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| processing_error(format!("Unreadable image: {e}")))?
        .decode()
        .map_err(|e| processing_error(format!("Invalid image data: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn opts(encoding: EncodingPreference) -> ProcessingOptions {
        ProcessingOptions {
            max_dimension: 2048,
            jpeg_quality: 90,
            encoding,
        }
    }

    /// A frame where each pixel encodes its own coordinates, so crops can be
    /// verified exactly.
    fn coordinate_frame(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                ((x / 256) * 16 + y / 256) as u8,
                255,
            ])
        })
    }

    /// Something resembling a screenshot of text: white background with
    /// thin dark strokes and a little colour noise (antialiasing).
    fn text_like_frame(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let stroke = (y % 18 < 12) && ((x * 7 + y * 3) % 11 < 4) && (x % 90 < 80);
            if stroke {
                let n = ((x * 31 + y * 17) % 40) as u8;
                Rgba([20 + n, 20 + n / 2, 30, 255])
            } else {
                Rgba([250, 250, 250, 255])
            }
        })
    }

    #[test]
    fn crop_returns_exact_pixels() {
        let frame = coordinate_frame(400, 300);
        let rect = PixelRect {
            x: 37,
            y: 120,
            width: 50,
            height: 20,
        };
        let out = crop(&frame, rect).unwrap();
        assert_eq!(out.dimensions(), (50, 20));
        assert_eq!(out.get_pixel(0, 0), frame.get_pixel(37, 120));
        assert_eq!(out.get_pixel(49, 19), frame.get_pixel(86, 139));
    }

    #[test]
    fn crop_rejects_out_of_bounds_and_overflow() {
        let frame = coordinate_frame(100, 100);
        assert!(
            crop(
                &frame,
                PixelRect {
                    x: 90,
                    y: 0,
                    width: 20,
                    height: 10
                }
            )
            .is_err()
        );
        assert!(
            crop(
                &frame,
                PixelRect {
                    x: u32::MAX,
                    y: 0,
                    width: 2,
                    height: 2
                }
            )
            .is_err()
        );
        assert!(
            crop(
                &frame,
                PixelRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 10
                }
            )
            .is_err()
        );
    }

    #[test]
    fn fit_dimensions_preserves_aspect_ratio_and_never_upscales() {
        assert_eq!(fit_dimensions(4000, 1000, 2000), (2000, 500));
        assert_eq!(fit_dimensions(1000, 4000, 2000), (500, 2000));
        assert_eq!(fit_dimensions(800, 600, 2000), (800, 600));
        assert_eq!(fit_dimensions(10000, 1, 100), (100, 1));
    }

    #[test]
    fn reduction_percent_math() {
        assert_eq!(reduction_percent(1000, 400), 60.0);
        assert_eq!(reduction_percent(1000, 1000), 0.0);
        assert_eq!(reduction_percent(1000, 1500), -50.0);
        assert_eq!(reduction_percent(0, 10), 0.0);
        assert_eq!(reduction_percent(3, 1), 66.7);
    }

    #[test]
    fn png_output_round_trips_losslessly() {
        let frame = coordinate_frame(300, 200);
        let out = process_selection(
            &frame,
            PixelRect {
                x: 10,
                y: 10,
                width: 120,
                height: 80,
            },
            opts(EncodingPreference::Png),
        )
        .unwrap();
        assert_eq!(out.format, ImageFormat::Png);
        let decoded = decode(&out.bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (120, 80));
        assert_eq!(decoded.get_pixel(5, 7), frame.get_pixel(15, 17));
    }

    #[test]
    fn jpeg_output_is_valid_and_resized() {
        let frame = text_like_frame(3000, 1200);
        let options = ProcessingOptions {
            max_dimension: 1500,
            jpeg_quality: 90,
            encoding: EncodingPreference::Jpeg,
        };
        let out = process_selection(
            &frame,
            PixelRect {
                x: 0,
                y: 0,
                width: 3000,
                height: 1200,
            },
            options,
        )
        .unwrap();
        assert_eq!(out.format, ImageFormat::Jpeg);
        assert_eq!(
            (out.stats.processed_width, out.stats.processed_height),
            (1500, 600)
        );
        assert_eq!(
            (out.stats.original_width, out.stats.original_height),
            (3000, 1200)
        );
        let decoded = decode(&out.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1500, 600));
        assert_eq!(out.stats.processed_bytes, out.bytes.len());
    }

    #[test]
    fn auto_picks_the_smaller_encoding() {
        let frame = text_like_frame(800, 400);
        let rect = PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 400,
        };
        let auto = process_selection(&frame, rect, opts(EncodingPreference::Auto)).unwrap();
        let webp = process_selection(&frame, rect, opts(EncodingPreference::Webp)).unwrap();
        let png = process_selection(&frame, rect, opts(EncodingPreference::Png)).unwrap();
        let jpeg = process_selection(&frame, rect, opts(EncodingPreference::Jpeg)).unwrap();
        assert_eq!(
            auto.bytes.len(),
            png.bytes.len().min(jpeg.bytes.len()).min(webp.bytes.len())
        );
        assert!(auto.stats.reduction_percent >= 0.0);
    }

    #[test]
    fn stats_are_consistent() {
        let frame = coordinate_frame(500, 500);
        let out = process_selection(
            &frame,
            PixelRect {
                x: 0,
                y: 0,
                width: 500,
                height: 500,
            },
            opts(EncodingPreference::Auto),
        )
        .unwrap();
        let s = &out.stats;
        assert_eq!(s.processed_bytes, out.bytes.len());
        assert_eq!(
            s.reduction_percent,
            reduction_percent(s.original_bytes, s.processed_bytes)
        );
        assert!(s.duration_ms >= 0.0);
        let json = serde_json::to_value(s).unwrap();
        assert!(json.get("reductionPercent").is_some());
        assert!(["png", "jpeg", "webp"].contains(&json["format"].as_str().unwrap()));
    }

    #[test]
    fn webp_is_lossless() {
        let frame = coordinate_frame(200, 120);
        let out = process_selection(
            &frame,
            PixelRect {
                x: 5,
                y: 5,
                width: 150,
                height: 100,
            },
            opts(EncodingPreference::Webp),
        )
        .unwrap();
        assert_eq!(out.format, ImageFormat::Webp);
        assert_eq!(out.format.mime_type(), "image/webp");
        let decoded = decode(&out.bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (150, 100));
        assert_eq!(decoded.get_pixel(10, 20), frame.get_pixel(15, 25));
    }

    #[test]
    fn decode_rejects_invalid_input() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"definitely not an image").is_err());
        let mut png = encode_png(&DynamicImage::ImageRgba8(coordinate_frame(10, 10))).unwrap();
        png.truncate(png.len() / 2);
        assert!(decode(&png).is_err());
    }
}
