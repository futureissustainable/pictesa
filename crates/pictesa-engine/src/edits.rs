//! Helpers for the edit canvas: painted masks and expanded canvases. They are stored as library
//! files with source `edit`, hidden from library lists, and remember the image they were made from
//! (`made_from`), so a run that uses them still shows where it came from.

use std::io::Cursor;

use image::{DynamicImage, GrayImage, ImageFormat, Luma, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{store, Asset, Engine, EngineError, Result};

/// Source of edit helpers in the `assets` table.
pub const HELPER_SOURCE: &str = "edit";
/// Largest side and area of an expanded canvas, so one request cannot exhaust memory.
pub const MAX_EDGE: u32 = 8192;
pub const MAX_PIXELS: u64 = 24_000_000;
/// Largest painted mask file accepted.
pub const MAX_MASK_BYTES: usize = 32 * 1024 * 1024;

/// Pixels to add on each side of an image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Padding {
    #[serde(default)]
    pub left: u32,
    #[serde(default)]
    pub top: u32,
    #[serde(default)]
    pub right: u32,
    #[serde(default)]
    pub bottom: u32,
}

/// An expanded canvas and the mask of its new area (white = fill).
#[derive(Debug, Clone, Serialize)]
pub struct Expanded {
    pub image: Asset,
    pub mask: Asset,
}

impl Engine {
    /// Stores a painted mask for an image. Painted pixels become white, the rest black, at the
    /// image's own size. The mask may mark paint with alpha (a canvas export) or with light pixels.
    pub async fn save_mask(&self, user: &str, source_id: &str, bytes: Vec<u8>) -> Result<Asset> {
        if bytes.len() > MAX_MASK_BYTES {
            return Err(EngineError::invalid("The mask is larger than 32 MB.", Some("mask")));
        }
        let (size, _) = self.edit_source(user, source_id).await?;
        let png = tokio::task::spawn_blocking(move || normalize_mask(&bytes, size.0, size.1).and_then(|m| encode(&DynamicImage::ImageLuma8(m))))
            .await
            .map_err(anyhow::Error::from)?
            .map_err(|message| EngineError::invalid(message, Some("mask")))?;
        self.store_helper(user, png, source_id, "mask.png").await
    }

    /// Grows an image by `pad` pixels per side. The new area repeats the edge pixels (a neutral
    /// start for outpainting models); the mask marks it white, overlapping the old edge slightly so
    /// the seam blends.
    pub async fn expand_canvas(&self, user: &str, source_id: &str, pad: Padding) -> Result<Expanded> {
        let (_, path) = self.edit_source(user, source_id).await?;
        let made = tokio::task::spawn_blocking(move || -> std::result::Result<(Vec<u8>, Vec<u8>), String> {
            let img = image::ImageReader::open(&path).and_then(|r| r.with_guessed_format()).map_err(|_| unreadable_text())?.decode().map_err(|_| unreadable_text())?;
            let (canvas, mask) = expand(&img, pad)?;
            Ok((encode(&canvas)?, encode(&DynamicImage::ImageLuma8(mask))?))
        })
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|message| EngineError::invalid(message, Some("padding")))?;
        let image = self.store_helper(user, made.0, source_id, "expanded.png").await?;
        let mask = self.store_helper(user, made.1, source_id, "expanded-mask.png").await?;
        Ok(Expanded { image, mask })
    }

    /// The size and file of the caller's own image, when it is not in the trash and small enough
    /// to edit in memory.
    async fn edit_source(&self, user: &str, id: &str) -> Result<((u32, u32), std::path::PathBuf)> {
        let row = store::asset_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("file"))?;
        if row.deleted_at.is_some() {
            return Err(EngineError::invalid("That image is in the trash.", Some("asset")));
        }
        if row.kind != "image" {
            return Err(EngineError::invalid("Only images can be edited.", Some("asset")));
        }
        let path = self.inner.files.path(&row.hash, &row.ext);
        let (w, h) = match (row.width, row.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => image::image_dimensions(&path).map_err(|_| unreadable())?,
        };
        if w > MAX_EDGE || h > MAX_EDGE || w as u64 * h as u64 > MAX_PIXELS {
            return Err(EngineError::invalid(format!("This image is {w} x {h} px. Images up to {MAX_EDGE} px per side and 24 megapixels can be edited."), Some("asset")));
        }
        Ok(((w, h), path))
    }

    async fn store_helper(&self, user: &str, png: Vec<u8>, origin: &str, name: &str) -> Result<Asset> {
        let stored = self.inner.files.store_bytes(&png, Some("image/png"), Some(name), true).await?;
        self.insert_named_asset(user, &stored, HELPER_SOURCE, None, None, Some(name), Some(origin)).await
    }
}

fn unreadable_text() -> String {
    "This image format cannot be edited here. Use a PNG, JPEG, WebP or GIF.".into()
}

fn unreadable() -> EngineError {
    EngineError::invalid(unreadable_text(), Some("asset"))
}

fn encode(img: &DynamicImage) -> std::result::Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).map_err(|e| format!("Could not write the image: {e}"))?;
    Ok(out.into_inner())
}

/// Turns any painted mask into a black and white mask of `width` x `height` (white = change).
/// Transparent masks count their opaque pixels as paint; opaque masks count light pixels.
pub fn normalize_mask(bytes: &[u8], width: u32, height: u32) -> std::result::Result<GrayImage, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| "The mask is not an image.".to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| "The mask is not a readable PNG, JPEG or WebP image.".to_string())?;
    let rgba = img.to_rgba8();
    let by_alpha = rgba.pixels().any(|p| p.0[3] < 255);
    let gray = GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let value = if by_alpha { a } else { ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8 };
        Luma([value])
    });
    let sized = if gray.dimensions() == (width, height) {
        gray
    } else {
        image::imageops::resize(&gray, width, height, image::imageops::FilterType::Triangle)
    };
    let mut painted = false;
    let mask = GrayImage::from_fn(width, height, |x, y| {
        let on = sized.get_pixel(x, y).0[0] >= 128;
        painted |= on;
        Luma([if on { 255 } else { 0 }])
    });
    if !painted {
        return Err("Paint over the area to change first.".into());
    }
    Ok(mask)
}

/// The padded canvas (edge pixels repeated outward) and the mask of its new area.
pub fn expand(img: &DynamicImage, pad: Padding) -> std::result::Result<(DynamicImage, GrayImage), String> {
    let (w, h) = (img.width(), img.height());
    if pad == Padding::default() {
        return Err("Add space on at least one side.".into());
    }
    let (nw, nh) = (w as u64 + pad.left as u64 + pad.right as u64, h as u64 + pad.top as u64 + pad.bottom as u64);
    if nw > MAX_EDGE as u64 || nh > MAX_EDGE as u64 || nw * nh > MAX_PIXELS {
        return Err(format!("The expanded image would be {nw} x {nh} px. Keep it within {MAX_EDGE} px per side and 24 megapixels."));
    }
    let (nw, nh) = (nw as u32, nh as u32);
    let src = img.to_rgba8();
    let pick = |x: u32, y: u32| {
        let sx = (x as i64 - pad.left as i64).clamp(0, w as i64 - 1) as u32;
        let sy = (y as i64 - pad.top as i64).clamp(0, h as i64 - 1) as u32;
        *src.get_pixel(sx, sy)
    };
    let canvas = RgbaImage::from_fn(nw, nh, pick);
    // The mask reaches a little into the old image on every grown side.
    let overlap = (w.min(h) / 64).clamp(4, 32);
    let reach = |padded: u32| if padded > 0 { padded + overlap } else { 0 };
    let (l, t) = (reach(pad.left), reach(pad.top));
    let (r, b) = (nw - reach(pad.right).min(nw), nh - reach(pad.bottom).min(nh));
    let mask = GrayImage::from_fn(nw, nh, |x, y| Luma([if x < l || y < t || x >= r || y >= b { 255 } else { 0 }]));
    let canvas = if img.color().has_alpha() { DynamicImage::ImageRgba8(canvas) } else { DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(canvas).to_rgb8()) };
    Ok((canvas, mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(img: DynamicImage) -> Vec<u8> {
        encode(&img).unwrap()
    }

    #[test]
    fn canvas_strokes_become_a_full_size_white_mask() {
        // A half-size canvas export: transparent with one opaque stroke in the top-left quarter.
        let mut stroke = RgbaImage::new(50, 40);
        for y in 0..20 {
            for x in 0..25 {
                stroke.put_pixel(x, y, image::Rgba([242, 105, 61, 255]));
            }
        }
        let mask = normalize_mask(&png(DynamicImage::ImageRgba8(stroke)), 100, 80).unwrap();
        assert_eq!(mask.dimensions(), (100, 80));
        assert_eq!(mask.get_pixel(10, 10).0[0], 255);
        assert_eq!(mask.get_pixel(90, 70).0[0], 0);
        assert!(mask.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255), "the mask is strictly black and white");
    }

    #[test]
    fn opaque_masks_use_light_pixels_and_empty_masks_are_refused() {
        let mut bw = GrayImage::new(10, 10);
        bw.put_pixel(9, 9, Luma([255]));
        let mask = normalize_mask(&png(DynamicImage::ImageLuma8(bw)), 10, 10).unwrap();
        assert_eq!((mask.get_pixel(9, 9).0[0], mask.get_pixel(0, 0).0[0]), (255, 0));
        let blank = png(DynamicImage::ImageRgba8(RgbaImage::new(10, 10)));
        assert!(normalize_mask(&blank, 10, 10).unwrap_err().contains("Paint"));
        assert!(normalize_mask(b"not an image", 10, 10).is_err());
    }

    #[test]
    fn expanding_pads_with_edge_pixels_and_masks_only_the_new_area() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(256, 128, image::Rgb([10, 20, 30])));
        let (canvas, mask) = expand(&img, Padding { left: 64, right: 64, top: 0, bottom: 0 }).unwrap();
        assert_eq!((canvas.width(), canvas.height()), (384, 128));
        assert_eq!(mask.dimensions(), (384, 128));
        assert!(!canvas.color().has_alpha(), "an opaque source stays opaque");
        assert_eq!(canvas.to_rgb8().get_pixel(0, 0).0, [10, 20, 30]);
        // New area and a small overlap are white; the middle and the untouched sides are black.
        assert_eq!(mask.get_pixel(0, 64).0[0], 255);
        assert_eq!(mask.get_pixel(64 + 2, 64).0[0], 255);
        assert_eq!(mask.get_pixel(192, 64).0[0], 0);
        assert_eq!(mask.get_pixel(192, 0).0[0], 0, "no space was added at the top");
        assert_eq!(mask.get_pixel(383, 127).0[0], 255);
    }

    #[test]
    fn expanding_refuses_no_change_and_oversized_canvases() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(100, 100));
        assert!(expand(&img, Padding::default()).is_err());
        assert!(expand(&img, Padding { right: MAX_EDGE, ..Default::default() }).is_err());
    }
}
