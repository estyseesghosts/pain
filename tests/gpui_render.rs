//! GPUI window/render gates for Kitty images.
//!
//! These tests build a real GPUI window, run `TerminalView` through GPUI's
//! normal request-layout/prepaint/paint path, and inspect both the resulting
//! `Scene` and a CPU-rasterized frame. The renderer is intentionally a small
//! recording backend: it stores the exact BGRA bytes GPUI receives from the
//! texture atlas, which lets the tests prove crop, alpha, layer order,
//! negative-row clipping, 1.5x HiDPI geometry, and animated generation
//! replacement without requiring a GPU on CI.

#![cfg(feature = "gpui-tests")]

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use gpui::{
    point, px, size, AnyWindowHandle, AppContext, AtlasKey, AtlasTextureId, AtlasTile, Bounds,
    DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, HeadlessAppContext, LineLayout,
    MonochromeSprite, NoopTextSystem, Pixels, PlatformAtlas, PlatformHeadlessRenderer,
    PlatformTextSystem, Point, PolychromeSprite, PrimitiveBatch, RenderGlyphParams,
    Result as GpuiResult, ScaledPixels, Scene, Size, TextRenderingMode, TileId,
};
use image::{Rgba, RgbaImage};
use pain::gpui::render::{TerminalFont, TerminalView};
use pain::terminal::session::{NoopWakeup, TerminalSession};

/// GPUI's test platform owns some thread-local/global state. Keep every case
/// serial so one failed render cannot poison an unrelated window.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// GPUI's debug `Window::new` frame is large. Run each case on a dedicated
/// thread with a generous stack so the test harness's default thread stack
/// cannot turn a valid render into a `__chkstk` fault.
fn run_render_test(body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::Builder::new()
        .name("gpui-render-test".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let _guard = serial();
            body();
        })
        .expect("spawn GPUI render test thread");
    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn apc(control: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x1b_G");
    bytes.extend_from_slice(control.as_bytes());
    bytes.push(b';');
    bytes.extend_from_slice(b64(payload).as_bytes());
    bytes.extend_from_slice(b"\x1b\\");
    bytes
}

fn control(control: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x1b_G");
    bytes.extend_from_slice(control.as_bytes());
    bytes.extend_from_slice(b"\x1b\\");
    bytes
}

fn feed(session: &TerminalSession, bytes: &[u8]) {
    session.write_vt(bytes);
    let _ = session.drain_writer(64);
}

fn rgba(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        out.extend_from_slice(&pixel);
    }
    out
}

/// Four 2x2 quadrants in a 4x4 image: red, green, blue, white.
fn quadrants() -> Vec<u8> {
    let mut out = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let color = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 255],
            };
            out.extend_from_slice(&color);
        }
    }
    out
}

fn session() -> TerminalSession {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    // Keep the cursor out of the image pixels.
    session.write_vt(b"\x1b[?25l");
    session
}

fn move_to(session: &TerminalSession, row: u16, col: u16) {
    session.write_vt(format!("\x1b[{row};{col}H").as_bytes());
}

/// Transmit and display a raw RGBA image at the cursor without moving it.
fn display(
    session: &TerminalSession,
    id: u32,
    width: u32,
    height: u32,
    extra: &str,
    pixels: &[u8],
) {
    let control = format!(
        "a=T,f=32,s={width},v={height},i={id},C=1,q=2{}",
        if extra.is_empty() {
            String::new()
        } else {
            format!(",{extra}")
        }
    );
    feed(session, &apc(&control, pixels));
}

fn image_placement(session: &TerminalSession, id: u32) -> pain::terminal::ImagePlacement {
    let frame = session.render_snapshot().expect("render snapshot");
    let placement = frame
        .image_placements
        .iter()
        .find(|p| p.image_id == id)
        .cloned()
        .expect("placement");
    session.mark_frame_complete().expect("frame complete");
    placement
}

// ---------------------------------------------------------------------------
// A minimal recording atlas + CPU renderer.
// ---------------------------------------------------------------------------

struct Texture {
    size: Size<DevicePixels>,
    /// BGRA bytes exactly as GPUI's atlas receives them.
    bytes: Vec<u8>,
}

#[derive(Default)]
struct AtlasState {
    next: u32,
    tiles: HashMap<AtlasKey, AtlasTile>,
    textures: HashMap<u32, Texture>,
    inserts: usize,
    removes: usize,
}

#[derive(Default)]
struct RecordingAtlas {
    state: Mutex<AtlasState>,
}

impl RecordingAtlas {
    fn first_bgra(&self, texture_index: u32) -> Option<[u8; 4]> {
        let state = self.state.lock().expect("atlas");
        let texture = state.textures.get(&texture_index)?;
        texture.bytes.get(0..4).map(|b| [b[0], b[1], b[2], b[3]])
    }

    fn inserts(&self) -> usize {
        self.state.lock().expect("atlas").inserts
    }

    fn removes(&self) -> usize {
        self.state.lock().expect("atlas").removes
    }
}

impl PlatformAtlas for RecordingAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> GpuiResult<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> GpuiResult<Option<AtlasTile>> {
        let mut state = self.state.lock().expect("atlas");
        if let Some(tile) = state.tiles.get(&key) {
            return Ok(Some(*tile));
        }
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let index = state.next;
        state.next += 1;
        let tile = AtlasTile {
            texture_id: AtlasTextureId {
                index,
                kind: key.texture_kind(),
            },
            tile_id: TileId(index),
            padding: 0,
            bounds: Bounds {
                origin: Point::default(),
                size,
            },
        };
        state.tiles.insert(key, tile);
        state.textures.insert(
            index,
            Texture {
                size,
                bytes: bytes.into_owned(),
            },
        );
        state.inserts += 1;
        Ok(Some(tile))
    }

    fn remove(&self, key: &AtlasKey) {
        let mut state = self.state.lock().expect("atlas");
        if let Some(tile) = state.tiles.remove(key) {
            state.textures.remove(&tile.texture_id.index);
            state.removes += 1;
        }
    }
}

#[derive(Clone, Default)]
struct RecordedScene {
    images: Vec<PolychromeSprite>,
    monos: Vec<MonochromeSprite>,
}

struct RecordingRenderer {
    atlas: Arc<RecordingAtlas>,
    recorded: Arc<Mutex<RecordedScene>>,
}

impl PlatformHeadlessRenderer for RecordingRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        _size: Size<DevicePixels>,
    ) -> GpuiResult<RgbaImage> {
        *self.recorded.lock().expect("recorded scene") = RecordedScene {
            images: scene.polychrome_sprites.clone(),
            monos: scene.monochrome_sprites.clone(),
        };
        Ok(rasterize(scene, &self.atlas))
    }

    fn render_scene(&mut self, _scene: &Scene, _size: Size<DevicePixels>) -> GpuiResult<()> {
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

fn fill_rect(
    image: &mut RgbaImage,
    rect: Bounds<ScaledPixels>,
    mask: Bounds<ScaledPixels>,
    color: [u8; 4],
) {
    let clip = intersect(rect, mask);
    for y in clip.2..clip.3 {
        for x in clip.0..clip.1 {
            blend(image.get_pixel_mut(x as u32, y as u32), color, 1.0);
        }
    }
}

/// `(left, right, top, bottom)` in device pixels, clamped to the origin.
fn intersect(a: Bounds<ScaledPixels>, b: Bounds<ScaledPixels>) -> (i32, i32, i32, i32) {
    let left = a.origin.x.0.max(b.origin.x.0).max(0.0);
    let top = a.origin.y.0.max(b.origin.y.0).max(0.0);
    let right = (a.origin.x.0 + a.size.width.0).min(b.origin.x.0 + b.size.width.0);
    let bottom = (a.origin.y.0 + a.size.height.0).min(b.origin.y.0 + b.size.height.0);
    (
        left.floor() as i32,
        right.ceil() as i32,
        top.floor() as i32,
        bottom.ceil() as i32,
    )
}

fn blend(dst: &mut Rgba<u8>, src: [u8; 4], opacity: f32) {
    let sa = (f32::from(src[3]) / 255.0) * opacity.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = f32::from(dst[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        *dst = Rgba([0, 0, 0, 0]);
        return;
    }
    let mix = |s: u8, d: u8| {
        (((f32::from(s) / 255.0) * sa + (f32::from(d) / 255.0) * da * (1.0 - sa)) / oa * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    *dst = Rgba([
        mix(src[2], dst[0]),
        mix(src[1], dst[1]),
        mix(src[0], dst[2]),
        (oa * 255.0).round() as u8,
    ]);
}

fn rasterize(scene: &Scene, atlas: &RecordingAtlas) -> RgbaImage {
    let mut width = 1.0f32;
    let mut height = 1.0f32;
    let mut include = |bounds: Bounds<ScaledPixels>| {
        width = width.max(bounds.origin.x.0 + bounds.size.width.0);
        height = height.max(bounds.origin.y.0 + bounds.size.height.0);
    };
    for quad in &scene.quads {
        include(quad.bounds);
    }
    for sprite in &scene.monochrome_sprites {
        include(sprite.bounds);
    }
    for sprite in &scene.polychrome_sprites {
        include(sprite.bounds);
    }
    let mut image = RgbaImage::from_pixel(
        width.ceil().max(1.0) as u32,
        height.ceil().max(1.0) as u32,
        Rgba([0, 0, 0, 0]),
    );

    // `batches()` is GPUI's actual cross-kind paint order. Sorting by the
    // spatial `order` field alone would not prove below-text vs above-text,
    // because primitives at the same order are batched by kind.
    for batch in scene.batches() {
        match batch {
            PrimitiveBatch::Quads(range) => {
                for quad in &scene.quads[range] {
                    let Some(color) = quad.background.as_solid() else {
                        continue;
                    };
                    let rgba = color.to_rgb();
                    let bytes = [
                        (rgba.r * 255.0).round() as u8,
                        (rgba.g * 255.0).round() as u8,
                        (rgba.b * 255.0).round() as u8,
                        (rgba.a * 255.0).round() as u8,
                    ];
                    fill_rect(&mut image, quad.bounds, quad.content_mask.bounds, bytes);
                }
            }
            PrimitiveBatch::MonochromeSprites { range, .. } => {
                for sprite in &scene.monochrome_sprites[range] {
                    let rgba = sprite.color.to_rgb();
                    let bytes = [
                        (rgba.r * 255.0).round() as u8,
                        (rgba.g * 255.0).round() as u8,
                        (rgba.b * 255.0).round() as u8,
                        (rgba.a * 255.0).round() as u8,
                    ];
                    // The test text system has no real glyph raster. A solid
                    // glyph box is enough to prove text is above/below an
                    // image.
                    fill_rect(&mut image, sprite.bounds, sprite.content_mask.bounds, bytes);
                }
            }
            PrimitiveBatch::PolychromeSprites { range, .. } => {
                for sprite in &scene.polychrome_sprites[range] {
                    draw_image(&mut image, sprite, atlas);
                }
            }
            _ => {}
        }
    }
    image
}

fn draw_image(image: &mut RgbaImage, sprite: &PolychromeSprite, atlas: &RecordingAtlas) {
    let index = sprite.tile.texture_id.index;
    let Some(texture) = atlas
        .state
        .lock()
        .expect("atlas")
        .textures
        .get(&index)
        .map(|texture| (texture.size, texture.bytes.clone()))
    else {
        return;
    };
    let (texture_size, bytes) = texture;
    let dest = sprite.bounds;
    let tile = sprite.tile.bounds;
    let clip = intersect(dest, sprite.content_mask.bounds);
    if dest.size.width.0 <= 0.0 || dest.size.height.0 <= 0.0 {
        return;
    }
    for y in clip.2..clip.3 {
        for x in clip.0..clip.1 {
            let u = (x as f32 + 0.5 - dest.origin.x.0) / dest.size.width.0;
            let v = (y as f32 + 0.5 - dest.origin.y.0) / dest.size.height.0;
            let sx = (tile.origin.x.0 as f32 + u * tile.size.width.0 as f32)
                .floor()
                .clamp(0.0, (texture_size.width.0 - 1) as f32) as usize;
            let sy = (tile.origin.y.0 as f32 + v * tile.size.height.0 as f32)
                .floor()
                .clamp(0.0, (texture_size.height.0 - 1) as f32) as usize;
            let offset = (sy * texture_size.width.0.max(0) as usize + sx) * 4;
            let Some(pixel) = bytes.get(offset..offset + 4) else {
                continue;
            };
            blend(
                image.get_pixel_mut(x as u32, y as u32),
                [pixel[0], pixel[1], pixel[2], pixel[3]],
                sprite.opacity,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// `NoopTextSystem` shapes text but reports zero-sized glyph rasters, so GPUI
/// culls every glyph before it reaches the scene. Give each glyph a small
/// opaque raster so the layer-order test has real text primitives between the
/// image stages; terminal cell metrics still come from the Noop metrics.
struct PaintTextSystem(NoopTextSystem);

impl PlatformTextSystem for PaintTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> GpuiResult<()> {
        self.0.add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        self.0.all_font_names()
    }

    fn font_id(&self, descriptor: &Font) -> GpuiResult<FontId> {
        self.0.font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.0.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> GpuiResult<Bounds<f32>> {
        self.0.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> GpuiResult<Size<f32>> {
        self.0.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.0.glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> GpuiResult<Bounds<DevicePixels>> {
        Ok(Bounds {
            origin: Point::new(DevicePixels(0), DevicePixels(-12)),
            size: size(DevicePixels(8), DevicePixels(12)),
        })
    }

    fn rasterize_glyph(
        &self,
        _params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> GpuiResult<(Size<DevicePixels>, Vec<u8>)> {
        let width = raster_bounds.size.width.0.max(0) as usize;
        let height = raster_bounds.size.height.0.max(0) as usize;
        Ok((raster_bounds.size, vec![0xff; width * height * 4]))
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.0.layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.0.recommended_rendering_mode(font_id, font_size)
    }
}

struct Rendered {
    images: Vec<PolychromeSprite>,
    monos: Vec<MonochromeSprite>,
    pixels: RgbaImage,
}

struct Harness {
    app: HeadlessAppContext,
    window: AnyWindowHandle,
    session: Arc<TerminalSession>,
    atlas: Arc<RecordingAtlas>,
    recorded: Arc<Mutex<RecordedScene>>,
}

impl Harness {
    fn new(session: TerminalSession, scale: f32) -> Self {
        let atlas = Arc::new(RecordingAtlas::default());
        let recorded = Arc::new(Mutex::new(RecordedScene::default()));
        let factory_atlas = Arc::clone(&atlas);
        let factory_recorded = Arc::clone(&recorded);
        let mut app = HeadlessAppContext::with_platform(
            Arc::new(PaintTextSystem(NoopTextSystem::new())),
            Arc::new(()),
            move || {
                Ok(Some(Box::new(RecordingRenderer {
                    atlas: Arc::clone(&factory_atlas),
                    recorded: Arc::clone(&factory_recorded),
                })
                    as Box<dyn PlatformHeadlessRenderer>))
            },
        );
        let session = Arc::new(session);
        let build_session = Arc::clone(&session);
        let handle = app
            .open_window(size(px(640.0), px(360.0)), move |window, cx| {
                cx.new(|cx| {
                    TerminalView::new(
                        build_session,
                        TerminalFont::new("monospace", 14.0),
                        window,
                        cx,
                    )
                })
            })
            .expect("open headless window");
        let window: AnyWindowHandle = *handle;
        app.update_window(window, |_, window, _| {
            window.set_scale_factor(scale);
        })
        .expect("set scale factor");
        app.run_until_parked();
        let mut harness = Self {
            app,
            window,
            session,
            atlas,
            recorded,
        };
        // `open_window` draws once at the test platform's default scale. Apply
        // the requested scale before the caller feeds content, otherwise the
        // first real draw resizes cell pixels after the text/image writes.
        let _ = harness.draw();
        harness
    }

    fn draw(&mut self) -> Rendered {
        let recorded = Arc::clone(&self.recorded);
        self.app
            .update_window(self.window, move |_, window, cx| {
                window.draw(cx).clear(cx);
                let pixels = window.render_to_image().expect("render scene to image");
                let scene = recorded.lock().expect("recorded scene").clone();
                Rendered {
                    images: scene.images,
                    monos: scene.monos,
                    pixels,
                }
            })
            .expect("draw window")
    }
}

fn center_bounds(bounds: Bounds<ScaledPixels>) -> (u32, u32) {
    (
        (bounds.origin.x.0 + bounds.size.width.0 / 2.0)
            .floor()
            .max(0.0) as u32,
        (bounds.origin.y.0 + bounds.size.height.0 / 2.0)
            .floor()
            .max(0.0) as u32,
    )
}

fn pixel_at(image: &RgbaImage, bounds: Bounds<ScaledPixels>) -> [u8; 4] {
    let (x, y) = center_bounds(bounds);
    let pixel = image.get_pixel(
        x.min(image.width().saturating_sub(1)),
        y.min(image.height().saturating_sub(1)),
    );
    [pixel[0], pixel[1], pixel[2], pixel[3]]
}

fn close(actual: [u8; 4], expected: [u8; 4], tolerance: i32) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (i32::from(*a) - i32::from(e)).abs() <= tolerance)
}

fn find_by_source(rendered: &Rendered, atlas: &RecordingAtlas, bgra: [u8; 4]) -> PolychromeSprite {
    rendered
        .images
        .iter()
        .copied()
        .find(|sprite| atlas.first_bgra(sprite.tile.texture_id.index) == Some(bgra))
        .expect("image sprite with expected source color")
}

fn overlaps(a: Bounds<ScaledPixels>, b: Bounds<ScaledPixels>) -> bool {
    let left = a.origin.x.0.max(b.origin.x.0);
    let top = a.origin.y.0.max(b.origin.y.0);
    let right = (a.origin.x.0 + a.size.width.0).min(b.origin.x.0 + b.size.width.0);
    let bottom = (a.origin.y.0 + a.size.height.0).min(b.origin.y.0 + b.size.height.0);
    right > left && bottom > top
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn gpui_crop_uses_source_rectangle() {
    run_render_test(|| {
        // Top-left source quadrant: the atlas sub-tile must be the 2x2 top-left
        // corner of the 4x4 texture, and the visible pixels must be red.
        let mut h = Harness::new(session(), 1.0);
        move_to(&h.session, 2, 2);
        display(
            &h.session,
            10,
            4,
            4,
            "x=0,y=0,w=2,h=2,c=4,r=4",
            &quadrants(),
        );
        let rendered = h.draw();
        assert_eq!(rendered.images.len(), 1);
        let sprite = rendered.images[0];
        assert_eq!(sprite.tile.bounds.origin.x.0, 0);
        assert_eq!(sprite.tile.bounds.origin.y.0, 0);
        assert_eq!(sprite.tile.bounds.size.width.0, 2);
        assert_eq!(sprite.tile.bounds.size.height.0, 2);
        assert!(
            close(
                pixel_at(&rendered.pixels, sprite.bounds),
                [255, 0, 0, 255],
                2
            ),
            "top-left source quadrant must render red"
        );

        // Bottom-right source quadrant: GPUI derives a sub-tile offset of (2,2)
        // from the difference between the projected image and destination bounds.
        let mut h = Harness::new(session(), 1.0);
        move_to(&h.session, 2, 2);
        display(
            &h.session,
            11,
            4,
            4,
            "x=2,y=2,w=2,h=2,c=4,r=4",
            &quadrants(),
        );
        let rendered = h.draw();
        assert_eq!(rendered.images.len(), 1);
        let sprite = rendered.images[0];
        assert_eq!(sprite.tile.bounds.origin.x.0, 2);
        assert_eq!(sprite.tile.bounds.origin.y.0, 2);
        assert_eq!(sprite.tile.bounds.size.width.0, 2);
        assert_eq!(sprite.tile.bounds.size.height.0, 2);
        assert!(
            close(
                pixel_at(&rendered.pixels, sprite.bounds),
                [255, 255, 255, 255],
                2
            ),
            "bottom-right source quadrant must render white"
        );
    });
}

#[test]
fn gpui_alpha_blends_with_the_terminal_background() {
    run_render_test(|| {
        let mut h = Harness::new(session(), 1.0);
        move_to(&h.session, 8, 8);
        display(
            &h.session,
            20,
            1,
            1,
            "c=1,r=1",
            &rgba(1, 1, [255, 0, 0, 128]),
        );
        let rendered = h.draw();
        assert_eq!(rendered.images.len(), 1);
        let sprite = rendered.images[0];

        let image = pixel_at(&rendered.pixels, sprite.bounds);
        let background = pixel_at(
            &rendered.pixels,
            Bounds {
                origin: point(
                    ScaledPixels(sprite.bounds.origin.x.0 + sprite.bounds.size.width.0 + 20.0),
                    ScaledPixels(sprite.bounds.origin.y.0),
                ),
                size: size(ScaledPixels(1.0), ScaledPixels(1.0)),
            },
        );
        let alpha = 128.0 / 255.0;
        let expected = [
            (255.0 * alpha + f32::from(background[0]) * (1.0 - alpha)).round() as u8,
            (0.0 * alpha + f32::from(background[1]) * (1.0 - alpha)).round() as u8,
            (0.0 * alpha + f32::from(background[2]) * (1.0 - alpha)).round() as u8,
            255,
        ];
        assert!(
        close(image, expected, 3),
        "alpha must survive the BGRA cache and blend with background: image={image:?} bg={background:?} expected={expected:?}"
    );
        assert_ne!(image[0], 255, "a dropped alpha would produce pure red");
    });
}

#[test]
fn gpui_below_text_is_under_glyphs_and_above_text_is_over_them() {
    run_render_test(|| {
        let mut h = Harness::new(session(), 1.0);

        // Two block glyphs at columns 0 and 6 on the first row.
        h.session.write_vt(b"\x1b[1;1H\xe2\x96\x88");
        h.session.write_vt(b"\x1b[1;7H\xe2\x96\x88");

        // Below-text red image at column 0, above-text blue image at column 6,
        // and a third below-text red image at column 12 with no glyph over it.
        move_to(&h.session, 1, 1);
        display(
            &h.session,
            30,
            1,
            1,
            "z=-1,c=1,r=1",
            &rgba(1, 1, [255, 0, 0, 255]),
        );
        move_to(&h.session, 1, 7);
        display(
            &h.session,
            31,
            1,
            1,
            "z=0,c=1,r=1",
            &rgba(1, 1, [0, 0, 255, 255]),
        );
        move_to(&h.session, 1, 13);
        display(
            &h.session,
            32,
            1,
            1,
            "z=0,c=1,r=1",
            &rgba(1, 1, [255, 0, 0, 255]),
        );

        let rendered = h.draw();
        assert_eq!(rendered.images.len(), 3);
        let below = rendered
            .images
            .iter()
            .copied()
            .find(|sprite| {
                h.atlas.first_bgra(sprite.tile.texture_id.index) == Some([0, 0, 255, 255])
                    && rendered
                        .monos
                        .iter()
                        .any(|mono| overlaps(mono.bounds, sprite.bounds))
            })
            .expect("red below-text image over a glyph");
        let above = find_by_source(&rendered, &h.atlas, [255, 0, 0, 255]);
        let below_mono = rendered
            .monos
            .iter()
            .copied()
            .find(|mono| overlaps(mono.bounds, below.bounds))
            .expect("below-text image overlaps the block glyph");
        let above_mono = rendered
            .monos
            .iter()
            .copied()
            .find(|mono| overlaps(mono.bounds, above.bounds))
            .expect("above-text image overlaps the block glyph");

        // Pixel proof from GPUI's actual batch order: the below-text cell shows
        // the text color, while the above-text cell shows the image color.
        // above-text cell shows the image color.
        let below_pixel = pixel_at(&rendered.pixels, below_mono.bounds.intersect(&below.bounds));
        let text_rgba = below_mono.color.to_rgb();
        let text_pixel = [
            (text_rgba.r * 255.0).round() as u8,
            (text_rgba.g * 255.0).round() as u8,
            (text_rgba.b * 255.0).round() as u8,
            (text_rgba.a * 255.0).round() as u8,
        ];
        assert!(
            close(below_pixel, text_pixel, 3),
            "text must cover the below-text image: pixel={below_pixel:?} text={text_pixel:?}"
        );
        let above_pixel = pixel_at(&rendered.pixels, above_mono.bounds.intersect(&above.bounds));
        assert!(
            close(above_pixel, [0, 0, 255, 255], 3),
            "above-text image must cover the glyph: pixel={above_pixel:?}"
        );
    });
}

#[test]
fn gpui_negative_row_clipping_shows_the_visible_bottom_half() {
    run_render_test(|| {
        let mut h = Harness::new(session(), 1.0);
        // First frame applies the window geometry to the terminal.
        let _ = h.draw();
        let rows = h
            .session
            .render_snapshot()
            .expect("initial snapshot")
            .meta
            .rows;
        h.session.mark_frame_complete().expect("frame complete");

        // Two-pixel image: top red, bottom green. Place it two rows before the
        // bottom, then scroll it up until only its bottom row remains visible.
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[255, 0, 0, 255]);
        pixels.extend_from_slice(&[0, 255, 0, 255]);
        pixels.extend_from_slice(&[0, 255, 0, 255]);
        move_to(&h.session, rows.saturating_sub(1), 1);
        display(&h.session, 40, 2, 2, "c=2,r=2", &pixels);

        // The cursor starts on the second-to-last row. One newline reaches the
        // bottom and the remaining rows-1 newlines scroll the image to -1.
        for _ in 0..rows {
            feed(&h.session, b"\r\n");
        }

        let rendered = h.draw();
        assert_eq!(rendered.images.len(), 1);
        let sprite = rendered.images[0];
        let placement = image_placement(&h.session, 40);
        assert_eq!(
            placement.viewport_row, -1,
            "image must be clipped at the top"
        );
        assert!(
            sprite.bounds.origin.y.0.abs() < 0.5,
            "clipped image must start at the terminal top: {:?}",
            sprite.bounds
        );
        let full_height = placement.pixel_height as f32;
        assert!(
            sprite.bounds.size.height.0 < full_height - 1.0,
            "negative row must clip the top of the image: visible={} full={}",
            sprite.bounds.size.height.0,
            full_height
        );
        assert!(
            close(
                pixel_at(&rendered.pixels, sprite.bounds),
                [0, 255, 0, 255],
                3
            ),
            "the visible bottom half must be green, not the clipped red top"
        );
    });
}

#[test]
fn gpui_hidpi_150_percent_keeps_device_pixel_geometry() {
    run_render_test(|| {
        let setup = |scale: f32| {
            let mut h = Harness::new(session(), scale);
            move_to(&h.session, 3, 3);
            display(&h.session, 50, 4, 4, "c=4,r=4", &quadrants());
            let rendered = h.draw();
            let placement = image_placement(&h.session, 50);
            let sprite = rendered.images[0];
            (rendered, placement, sprite)
        };

        let (_, placement_1x, sprite_1x) = setup(1.0);
        let (_, placement_15x, sprite_15x) = setup(1.5);

        // Cell size is rounded to whole device pixels before Ghostty sees it.
        // Derive the logical cell width from the 1x run and verify the 1.5x run
        // rounds exactly once, not by flooring the logical destination.
        let cells = 4.0;
        let cell_width_1x = placement_1x.pixel_width as f32 / cells;
        let expected_15x_width = ((cell_width_1x * 1.5).round() * cells) as u32;
        assert_eq!(
            placement_15x.pixel_width, expected_15x_width,
            "1.5x cell pixels must be rounded device pixels"
        );
        assert_ne!(
            placement_15x.pixel_width, placement_1x.pixel_width,
            "HiDPI must change Ghostty's pixel geometry"
        );

        // GPUI draws the image at exactly the device-pixel destination Ghostty
        // reported, and the full source tile is uncropped.
        assert!(
            (sprite_1x.bounds.size.width.0 - placement_1x.pixel_width as f32).abs() <= 1.0,
            "1x scene width {} != placement {}",
            sprite_1x.bounds.size.width.0,
            placement_1x.pixel_width
        );
        assert!(
            (sprite_15x.bounds.size.width.0 - placement_15x.pixel_width as f32).abs() <= 1.0,
            "1.5x scene width {} != placement {}",
            sprite_15x.bounds.size.width.0,
            placement_15x.pixel_width
        );
        assert_eq!(sprite_15x.tile.bounds.size.width.0, 4);
        assert_eq!(sprite_15x.tile.bounds.size.height.0, 4);
        assert!(
            sprite_15x.tile.bounds.origin.x.0 == 0 && sprite_15x.tile.bounds.origin.y.0 == 0,
            "1.5x must not crop the source rectangle"
        );
    });
}

#[test]
fn gpui_animated_generation_replaces_the_gpu_texture() {
    run_render_test(|| {
        let mut h = Harness::new(session(), 1.0);
        // Root frame red, second frame blue after 40 ms.
        display(
            &h.session,
            60,
            1,
            1,
            "c=1,r=1",
            &rgba(1, 1, [255, 0, 0, 255]),
        );
        feed(
            &h.session,
            &apc(
                "a=f,f=32,s=1,v=1,i=60,r=2,z=40,q=2",
                &rgba(1, 1, [0, 0, 255, 255]),
            ),
        );

        let first = h.draw();
        assert_eq!(first.images.len(), 1);
        let first_sprite = first.images[0];
        let first_texture = first_sprite.tile.texture_id.index;
        assert!(close(
            pixel_at(&first.pixels, first_sprite.bounds),
            [255, 0, 0, 255],
            3
        ));

        // Start only after the first frame is captured so wall-clock time
        // cannot advance the image before the red root is observed.
        feed(&h.session, &control("a=a,i=60,s=3,q=2"));
        // Advance exactly one animation frame; the next prepaint sees a new
        // Ghostty image generation and must upload a new texture.
        let next = h
            .session
            .tick_kitty_animation(0)
            .expect("animation tick")
            .expect("next deadline");
        assert!(next > 0);
        // Stop playback so prepaint cannot advance a second time before the
        // generation change is drawn.
        feed(&h.session, &control("a=a,i=60,s=1,q=2"));

        let second = h.draw();
        assert_eq!(second.images.len(), 1);
        let second_sprite = second.images[0];
        let second_texture = second_sprite.tile.texture_id.index;
        assert_ne!(
            first_texture, second_texture,
            "a new generation must use a new GPUI texture"
        );
        assert!(close(
            pixel_at(&second.pixels, second_sprite.bounds),
            [0, 0, 255, 255],
            3
        ));

        // The cache keeps exactly one grace frame, then drops the stale texture.
        // Bump the terminal generation so prepaint cannot take its cached-frame
        // fast path and must run the image cache's eviction pass.
        h.session.write_vt(b"");
        let _ = h.draw();
        assert!(
            h.atlas.removes() >= 1,
            "old generation texture must be evicted from the atlas"
        );
        assert!(
            h.atlas.inserts() >= 2,
            "both generations must be uploaded exactly once"
        );
    });
}
