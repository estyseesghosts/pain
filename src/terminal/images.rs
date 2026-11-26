//! Bounded GPUI texture cache for Kitty images.
//!
//! Ghostty remains the authoritative CPU-side image store. This module only
//! owns the GPU-facing `RenderImage` objects for images that are referenced by
//! the current render-state placement list, keyed by
//! `(protocol, image_id, generation)`. A static image therefore costs zero
//! uploads and zero pixel reads across repaints.
#![allow(clippy::chunks_exact_to_as_chunks)]

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{point, px, size, Bounds, Pixels, RenderImage, Window};
use image::{Frame, ImageBuffer, Rgba};

use crate::terminal::ghostty::{ghostty_vt_sys as sys, ImagePlacement, KittyImagePixels};
use crate::terminal::session::{CountersHandle, TerminalSession};

/// One texture identity. The protocol is part of the key because image IDs are
/// only unique within a protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub protocol: i32,
    pub image_id: u32,
    pub generation: u64,
}

impl ImageKey {
    #[must_use]
    pub fn from_placement(placement: &ImagePlacement) -> Self {
        Self {
            protocol: placement.protocol,
            image_id: placement.image_id,
            generation: placement.image_generation,
        }
    }
}

struct CacheEntry {
    image: Arc<RenderImage>,
    bytes: usize,
    /// Frame index in which this entry was last referenced. One frame of grace
    /// prevents churn when an image is scrolled out and immediately back in.
    last_used_frame: u64,
}

/// One placement resolved for painting. `image_bounds` projects the complete
/// source image; `bounds` is the visible destination clipped by the terminal.
/// The pinned `Window::paint_image` derives the atlas subregion from the
/// difference, so the source pixels are never cropped or copied here.
#[derive(Clone)]
pub struct ImageDraw {
    pub image: Arc<RenderImage>,
    pub bounds: Bounds<Pixels>,
    pub image_bounds: Bounds<Pixels>,
}

/// Three already-ordered draw lists matching Ghostty's paint stages. Within
/// each list placements retain Ghostty's `(layer, z)` order.
#[derive(Default)]
pub struct ImageLayers {
    pub below_background: Vec<ImageDraw>,
    pub below_text: Vec<ImageDraw>,
    pub above_text: Vec<ImageDraw>,
}

pub struct ImageCache {
    entries: HashMap<ImageKey, CacheEntry>,
    frame: u64,
    bytes: usize,
    max_bytes: usize,
}

impl Default for ImageCache {
    fn default() -> Self {
        // Ghostty's image storage cap is 10 MB by default. Keeping one frame of
        // grace and generous headroom still bounds the GPUI side well below the
        // terminal's own long-lived budget.
        Self::new(64 * 1024 * 1024)
    }
}

impl ImageCache {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            bytes: 0,
            max_bytes: max_bytes.max(1),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn cached_bytes(&self) -> usize {
        self.bytes
    }

    /// Convert a Ghostty image to the byte order GPUI's polychrome atlas
    /// expects (BGRA). The GPUI buffer declares `Rgba<u8>` because that is the
    /// only constructor GPUI exposes, but the atlas uploads the raw bytes as
    /// `Bgra8Unorm`, which is what the pinned renderers actually use.
    #[must_use]
    pub fn to_bgra(image: &KittyImagePixels) -> Option<Vec<u8>> {
        let pixels = usize::try_from(image.width)
            .ok()?
            .checked_mul(usize::try_from(image.height).ok()?)?;
        let out_len = pixels.checked_mul(4)?;
        let expected = KittyImagePixels::expected_len(image.width, image.height, image.format)?;
        if image.pixels.len() != expected {
            return None;
        }
        let mut out = vec![0u8; out_len];
        match image.format {
            sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB => {
                for (dst, src) in out.chunks_exact_mut(4).zip(image.pixels.chunks_exact(3)) {
                    dst.copy_from_slice(&[src[2], src[1], src[0], 0xff]);
                }
            }
            sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => {
                for (dst, src) in out.chunks_exact_mut(4).zip(image.pixels.chunks_exact(4)) {
                    dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
                }
            }
            sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => {
                for (dst, src) in out.chunks_exact_mut(4).zip(image.pixels.iter()) {
                    dst.copy_from_slice(&[*src, *src, *src, 0xff]);
                }
            }
            sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => {
                for (dst, src) in out.chunks_exact_mut(4).zip(image.pixels.chunks_exact(2)) {
                    dst.copy_from_slice(&[src[0], src[0], src[0], src[1]]);
                }
            }
            _ => return None,
        }
        Some(out)
    }

    fn texture(image: &KittyImagePixels, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
        let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(image.width, image.height, bgra)?;
        Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
    }

    /// Resolve every Kitty placement to a cached texture, uploading only the
    /// missing `(protocol, image_id, generation)` keys. Also drops textures
    /// that were not referenced this frame or the previous one.
    pub fn prepare(
        &mut self,
        session: &TerminalSession,
        placements: &[ImagePlacement],
        geometry: &crate::terminal::geometry::TerminalGeometry,
        counters: &CountersHandle,
        window: &mut Window,
    ) -> ImageLayers {
        self.frame = self.frame.wrapping_add(1);
        let frame = self.frame;
        let mut layers = ImageLayers::default();
        let mut referenced: Vec<ImageKey> = Vec::new();
        for placement in placements {
            if placement.protocol != sys::GHOSTTY_IMAGE_PROTOCOL_KITTY {
                continue;
            }
            let key = ImageKey::from_placement(placement);
            if !referenced.contains(&key) {
                referenced.push(key);
            }
        }

        // Request only keys that are not already resident. The request list is
        // deduplicated again by `read_kitty_images` under one terminal lock.
        let mut missing: Vec<(u32, u64)> = Vec::new();
        for key in &referenced {
            if !self.entries.contains_key(key) && !missing.contains(&(key.image_id, key.generation))
            {
                missing.push((key.image_id, key.generation));
            }
        }
        let requested = missing;
        if !requested.is_empty() {
            for pixels in session.read_kitty_images(&requested) {
                let key = ImageKey {
                    protocol: sys::GHOSTTY_IMAGE_PROTOCOL_KITTY,
                    image_id: pixels.image_id,
                    generation: pixels.generation,
                };
                let Some(bgra) = Self::to_bgra(&pixels) else {
                    continue;
                };
                let bytes = bgra.len();
                let Some(image) = Self::texture(&pixels, bgra) else {
                    continue;
                };
                if let Some(old) = self.entries.remove(&key) {
                    self.bytes = self.bytes.saturating_sub(old.bytes);
                    counters.image_texture_eviction();
                    let _ = window.drop_image(old.image);
                }
                self.bytes = self.bytes.saturating_add(bytes);
                counters.image_texture_upload();
                self.entries.insert(
                    key,
                    CacheEntry {
                        image,
                        bytes,
                        last_used_frame: frame,
                    },
                );
            }
        }

        for key in &referenced {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.last_used_frame = frame;
            }
        }

        // Drop stale generations and images that left the referenced set. Keep
        // exactly one grace frame so scrolling back does not re-upload.
        let stale: Vec<ImageKey> = self
            .entries
            .iter()
            .filter(|(key, entry)| {
                !referenced.contains(key) && frame.wrapping_sub(entry.last_used_frame) > 1
            })
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                counters.image_texture_eviction();
                let _ = window.drop_image(entry.image);
            }
        }

        // Enforce the byte budget by evicting the least recently used
        // non-current entries. Current placements are kept even over budget so
        // they cannot flicker; the next frame's deletion will bring it down.
        while self.bytes > self.max_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(key, _)| !referenced.contains(key))
                .min_by_key(|(_, entry)| entry.last_used_frame)
                .map(|(key, _)| *key);
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                counters.image_texture_eviction();
                let _ = window.drop_image(entry.image);
            }
        }

        for placement in placements {
            if placement.protocol != sys::GHOSTTY_IMAGE_PROTOCOL_KITTY {
                continue;
            }
            let key = ImageKey::from_placement(placement);
            let Some(entry) = self.entries.get(&key) else {
                continue;
            };
            let (image_width, image_height) = {
                let size = entry.image.size(0);
                (size.width.0.max(0) as u32, size.height.0.max(0) as u32)
            };
            if let Some((bounds, image_bounds)) =
                project_placement(placement, image_width, image_height, geometry)
            {
                let draw = ImageDraw {
                    image: Arc::clone(&entry.image),
                    bounds,
                    image_bounds,
                };
                match placement.layer {
                    sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND => {
                        layers.below_background.push(draw);
                    }
                    sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT => {
                        layers.below_text.push(draw);
                    }
                    _ => layers.above_text.push(draw),
                }
            }
        }
        layers
    }
}

/// Project one placement into GPUI logical coordinates without copying or
/// cropping the image buffer. Returns `(visible_bounds, full_image_bounds)`.
///
/// `geometry.origin` is the terminal content origin in logical pixels and
/// `geometry.content_*` is the clip rectangle. Negative rows/columns are
/// supported.
#[must_use]
pub fn project_placement(
    placement: &ImagePlacement,
    image_width: u32,
    image_height: u32,
    geometry: &crate::terminal::geometry::TerminalGeometry,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    if placement.pixel_width == 0 || placement.pixel_height == 0 {
        return None;
    }
    let scale = geometry.scale.max(f32::MIN_POSITIVE);
    let dest_width = placement.pixel_width as f32 / scale;
    let dest_height = placement.pixel_height as f32 / scale;
    let dest_x = geometry.origin.0
        + placement.viewport_col as f32 * geometry.cell_width
        + placement.cell_offset_x as f32 / scale;
    let dest_y = geometry.origin.1
        + placement.viewport_row as f32 * geometry.cell_height
        + placement.cell_offset_y as f32 / scale;
    let source_width = if placement.source_width == 0 {
        image_width.max(1)
    } else {
        placement.source_width
    };
    let source_height = if placement.source_height == 0 {
        image_height.max(1)
    } else {
        placement.source_height
    };
    let scale_x = dest_width / source_width as f32;
    let scale_y = dest_height / source_height as f32;
    let image_origin_x = dest_x - placement.source_x as f32 * scale_x;
    let image_origin_y = dest_y - placement.source_y as f32 * scale_y;
    let full_width = image_width as f32 * scale_x;
    let full_height = image_height as f32 * scale_y;
    let image_bounds = Bounds::new(
        point(px(image_origin_x), px(image_origin_y)),
        size(px(full_width), px(full_height)),
    );
    let destination = Bounds::new(
        point(px(dest_x), px(dest_y)),
        size(px(dest_width), px(dest_height)),
    );
    let clip = Bounds::new(
        point(px(geometry.origin.0), px(geometry.origin.1)),
        size(px(geometry.content_width), px(geometry.content_height)),
    );
    let bounds = destination.intersect(&clip);
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    Some((bounds, image_bounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::geometry::{CellMetrics, TerminalGeometry};

    fn placement(layer: i32, z: i32, col: i32, row: i32) -> ImagePlacement {
        ImagePlacement {
            protocol: sys::GHOSTTY_IMAGE_PROTOCOL_KITTY,
            image_id: 7,
            image_generation: 3,
            layer,
            z,
            viewport_col: col,
            viewport_row: row,
            grid_cols: 2,
            grid_rows: 1,
            cell_offset_x: 0,
            cell_offset_y: 0,
            pixel_width: 30,
            pixel_height: 20,
            source_x: 0,
            source_y: 0,
            source_width: 10,
            source_height: 10,
            placement_id: 1,
            kind: sys::GHOSTTY_RENDER_STATE_KITTY_PLACEMENT_KIND_FIXED,
        }
    }

    fn geometry(scale: f32) -> TerminalGeometry {
        TerminalGeometry::new(
            (100.0, 50.0),
            (200.0, 100.0),
            &CellMetrics::new(10.0, 20.0),
            scale,
        )
    }

    #[test]
    fn bgra_conversion_matches_gpui_atlas_order() {
        let rgb = KittyImagePixels {
            image_id: 1,
            generation: 1,
            width: 1,
            height: 1,
            format: sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB,
            pixels: vec![0x11, 0x22, 0x33],
        };
        assert_eq!(
            ImageCache::to_bgra(&rgb).unwrap(),
            vec![0x33, 0x22, 0x11, 0xff]
        );

        let rgba = KittyImagePixels {
            image_id: 1,
            generation: 1,
            width: 1,
            height: 1,
            format: sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA,
            pixels: vec![0x11, 0x22, 0x33, 0x44],
        };
        assert_eq!(
            ImageCache::to_bgra(&rgba).unwrap(),
            vec![0x33, 0x22, 0x11, 0x44]
        );

        let gray = KittyImagePixels {
            image_id: 1,
            generation: 1,
            width: 1,
            height: 1,
            format: sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY,
            pixels: vec![0x80],
        };
        assert_eq!(
            ImageCache::to_bgra(&gray).unwrap(),
            vec![0x80, 0x80, 0x80, 0xff]
        );

        let gray_alpha = KittyImagePixels {
            image_id: 1,
            generation: 1,
            width: 1,
            height: 1,
            format: sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA,
            pixels: vec![0x40, 0x7f],
        };
        assert_eq!(
            ImageCache::to_bgra(&gray_alpha).unwrap(),
            vec![0x40, 0x40, 0x40, 0x7f]
        );
    }

    #[test]
    fn bgra_rejects_wrong_length() {
        let bad = KittyImagePixels {
            image_id: 1,
            generation: 1,
            width: 2,
            height: 2,
            format: sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA,
            pixels: vec![0; 3],
        };
        assert!(ImageCache::to_bgra(&bad).is_none());
    }

    #[test]
    fn projection_places_image_at_cell_and_offset() {
        let (bounds, full) =
            project_placement(&placement(2, 0, 2, 1), 10, 10, &geometry(1.0)).expect("visible");
        assert_eq!(f32::from(bounds.origin.x), 120.0);
        assert_eq!(f32::from(bounds.origin.y), 70.0);
        assert_eq!(f32::from(bounds.size.width), 30.0);
        assert_eq!(f32::from(bounds.size.height), 20.0);
        assert_eq!(f32::from(full.origin.x), 120.0);
        assert_eq!(f32::from(full.size.width), 30.0);
    }

    #[test]
    fn projection_clips_negative_rows_to_terminal_bounds() {
        let mut p = placement(2, 0, 0, -2);
        p.pixel_height = 80;
        let (bounds, full) = project_placement(&p, 10, 10, &geometry(1.0)).expect("partly visible");
        // Destination starts above the terminal and is clipped at its top.
        assert_eq!(f32::from(bounds.origin.y), 50.0);
        assert_eq!(f32::from(bounds.size.height), 40.0);
        // The full projected image still starts at the real (negative) position,
        // so GPUI derives the correct source subregion.
        assert_eq!(f32::from(full.origin.y), 10.0);
    }

    #[test]
    fn projection_is_empty_when_fully_outside() {
        assert!(project_placement(&placement(2, 0, 0, -10), 10, 10, &geometry(1.0)).is_none());
    }

    #[test]
    fn projection_hidpi_keeps_source_in_image_pixels() {
        // At 2x, a 30-device-pixel destination is 15 logical pixels. Source
        // coordinates remain image pixels and must not be divided by scale.
        let (bounds, full) =
            project_placement(&placement(2, 0, 0, 0), 20, 20, &geometry(2.0)).expect("visible");
        assert_eq!(f32::from(bounds.size.width), 15.0);
        assert_eq!(f32::from(bounds.size.height), 10.0);
        // scale_x = 15 / 10 = 1.5; full image = 20 * 1.5 = 30 logical px.
        assert_eq!(f32::from(full.size.width), 30.0);
        // scale_y = 10 / 10 = 1; the full image keeps its 20 image pixels.
        assert_eq!(f32::from(full.size.height), 20.0);
    }

    #[test]
    fn projection_at_150_percent_does_not_change_source_rectangles() {
        // The exact fractional case that used to floor and crop: 30 device
        // pixels become 20 logical pixels at 1.5x, not 19.
        let (bounds, full) =
            project_placement(&placement(2, 0, 0, 0), 20, 20, &geometry(1.5)).expect("visible");
        assert!((f32::from(bounds.size.width) - 20.0).abs() < 0.001);
        assert!((f32::from(bounds.size.height) - 40.0 / 3.0).abs() < 0.001);
        // scale_x = 20 / 10 = 2; the whole 20 px image projects to 40 logical.
        assert!((f32::from(full.size.width) - 40.0).abs() < 0.001);
        // source_y scale is 13.333/10, so 20 image pixels project to 26.667.
        assert!((f32::from(full.size.height) - 40.0 / 1.5).abs() < 0.001);
    }

    #[test]
    fn negative_columns_are_clipped_like_negative_rows() {
        let mut p = placement(2, 0, -2, 0);
        p.pixel_width = 80;
        let (bounds, full) = project_placement(&p, 10, 10, &geometry(1.0)).expect("partly visible");
        // Destination starts 20 logical px left of the terminal and is clipped.
        assert_eq!(f32::from(bounds.origin.x), 100.0);
        assert_eq!(f32::from(bounds.size.width), 60.0);
        assert_eq!(f32::from(full.origin.x), 80.0);
    }

    #[test]
    fn generation_change_replaces_the_texture_key() {
        // Same image id and protocol, new generation: the renderer must upload
        // a new texture rather than reuse the old pixels.
        let mut first = placement(2, 0, 0, 0);
        first.image_generation = 7;
        let mut replacement = first.clone();
        replacement.image_generation = 8;
        let old_key = ImageKey::from_placement(&first);
        let new_key = ImageKey::from_placement(&replacement);
        assert_ne!(old_key, new_key);
        assert_eq!(old_key.image_id, new_key.image_id);
        // A new protocol with the same numeric id is also a distinct texture.
        let mut other_protocol = first.clone();
        other_protocol.protocol = sys::GHOSTTY_IMAGE_PROTOCOL_KITTY + 1;
        assert_ne!(old_key, ImageKey::from_placement(&other_protocol));
    }

    #[test]
    fn alpha_is_preserved_for_both_alpha_formats() {
        for (format, pixels, expected_alpha) in [
            (
                sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA,
                vec![10, 20, 30, 40],
                40,
            ),
            (sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA, vec![50, 60], 60),
        ] {
            let image = KittyImagePixels {
                image_id: 1,
                generation: 1,
                width: 1,
                height: 1,
                format,
                pixels,
            };
            let bgra = ImageCache::to_bgra(&image).expect("converted");
            assert_eq!(bgra[3], expected_alpha, "alpha must survive BGRA swap");
            assert_eq!(bgra.len(), 4);
        }
    }

    #[test]
    fn source_rectangle_projects_the_whole_image_behind_destination() {
        let mut p = placement(2, 0, 0, 0);
        p.source_x = 5;
        p.source_y = 5;
        p.source_width = 5;
        p.source_height = 5;
        p.pixel_width = 50;
        p.pixel_height = 50;
        let (bounds, full) = project_placement(&p, 10, 10, &geometry(1.0)).expect("visible");
        // scale = 50/5 = 10; full image is 100x100 logical, starting 50 px
        // above/left of the destination because source started at (5,5).
        assert_eq!(f32::from(full.origin.x), 100.0 - 50.0);
        assert_eq!(f32::from(full.size.width), 100.0);
        assert_eq!(f32::from(bounds.origin.x), 100.0);
        assert_eq!(f32::from(bounds.size.width), 50.0);
    }

    #[test]
    fn layer_partition_is_stable_for_sorted_placements() {
        let p = |layer, z| placement(layer, z, 0, 0);
        let all = [
            p(sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND, -2),
            p(sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND, 0),
            p(sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT, -1),
            p(sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT, 0),
        ];
        let below_bg: Vec<_> = all
            .iter()
            .filter(|p| p.layer == sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND)
            .map(|p| p.z)
            .collect();
        let below_text: Vec<_> = all
            .iter()
            .filter(|p| p.layer == sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT)
            .map(|p| p.z)
            .collect();
        let above_text: Vec<_> = all
            .iter()
            .filter(|p| p.layer == sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT)
            .map(|p| p.z)
            .collect();
        assert_eq!(below_bg, vec![-2, 0]);
        assert_eq!(below_text, vec![-1]);
        assert_eq!(above_text, vec![0]);
    }
}
