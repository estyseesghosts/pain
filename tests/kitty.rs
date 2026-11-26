//! Kitty graphics conformance matrix. Every payload is generated in test
//! code; no external image files are used. These tests exercise the real
//! libghostty-vt parser, the render-state placement cache, the bounded PNG
//! decoder and the batched pixel-copy API.

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use pain::terminal::ghostty::{ghostty_vt_sys as sys, ImagePlacement, KittyConfig};
use pain::terminal::session::{NoopWakeup, SessionConfig, TerminalSession};

const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

fn session(cols: u16, rows: u16) -> TerminalSession {
    let session = TerminalSession::new(cols, rows, Box::new(NoopWakeup)).expect("session");
    session
        .apply_layout_geometry(cols, rows, CELL_W, CELL_H)
        .expect("geometry");
    session
}

fn session_with_kitty(cols: u16, rows: u16, kitty: KittyConfig) -> TerminalSession {
    let session = TerminalSession::new_with_config(
        SessionConfig {
            cols,
            rows,
            kitty,
            ..SessionConfig::default()
        },
        Box::new(NoopWakeup),
    )
    .expect("session");
    session
        .apply_layout_geometry(cols, rows, CELL_W, CELL_H)
        .expect("geometry");
    session
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

fn rgba_pixels(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        out.extend_from_slice(&pixel);
    }
    out
}

fn rgb_pixels(width: u32, height: u32, pixel: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..width * height {
        out.extend_from_slice(&pixel);
    }
    out
}

fn png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(pixels, width, height, ExtendedColorType::Rgba8)
        .expect("png encode");
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A valid zlib stream using stored (uncompressed) deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    } else {
        let mut chunks = data.chunks(65_535).peekable();
        while let Some(chunk) = chunks.next() {
            out.push(if chunks.peek().is_none() { 0x01 } else { 0x00 });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn feed(session: &TerminalSession, bytes: &[u8]) {
    session.write_vt(bytes);
    // Drain Kitty responses (if the client asked for them) so the writer queue
    // does not grow during the matrix.
    let _ = session.drain_writer(64);
}

fn snapshot(session: &TerminalSession) -> Vec<ImagePlacement> {
    let frame = session.render_snapshot().expect("render snapshot");
    let placements = frame.image_placements.as_ref().clone();
    session.mark_frame_complete().expect("frame complete");
    placements
}

fn transmit_rgba(session: &TerminalSession, id: u32, width: u32, height: u32, pixel: [u8; 4]) {
    feed(
        session,
        &apc(
            &format!("a=t,f=32,s={width},v={height},i={id},q=2"),
            &rgba_pixels(width, height, pixel),
        ),
    );
}

#[test]
fn raw_rgb_and_rgba_round_trip_through_the_render_state() {
    let session = session(20, 8);
    feed(
        &session,
        &apc(
            "a=T,f=24,s=2,v=1,i=1,C=1,q=2",
            &rgb_pixels(2, 1, [0x11, 0x22, 0x33]),
        ),
    );
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=2,C=1,q=2",
            &rgba_pixels(1, 1, [0x44, 0x55, 0x66, 0x77]),
        ),
    );
    let placements = snapshot(&session);
    assert_eq!(placements.len(), 2);
    let rgb = placements.iter().find(|p| p.image_id == 1).expect("rgb");
    let rgba = placements.iter().find(|p| p.image_id == 2).expect("rgba");

    let pixels = session.read_kitty_images(&[
        (rgb.image_id, rgb.image_generation),
        (rgba.image_id, rgba.image_generation),
    ]);
    assert_eq!(pixels.len(), 2);
    let rgb_pixels = pixels.iter().find(|p| p.image_id == 1).unwrap();
    assert_eq!(rgb_pixels.format, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB);
    let rgb_bytes = {
        let mut out = Vec::new();
        for _ in 0..2 {
            out.extend_from_slice(&[0x11, 0x22, 0x33]);
        }
        out
    };
    assert_eq!(rgb_pixels.pixels, rgb_bytes);
    let rgba_pixels = pixels.iter().find(|p| p.image_id == 2).unwrap();
    assert_eq!(rgba_pixels.format, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA);
    assert_eq!(rgba_pixels.pixels, vec![0x44, 0x55, 0x66, 0x77]);
}

#[test]
fn png_payload_decodes_to_rgba_through_the_process_global_decoder() {
    let session = session(20, 8);
    let source = [
        0xff, 0x00, 0x00, 0xff, // red
        0x00, 0xff, 0x00, 0x80, // half-transparent green
        0x00, 0x00, 0xff, 0xff, // blue
        0xff, 0xff, 0xff, 0x00, // transparent white
    ];
    feed(
        &session,
        &apc("a=T,f=100,s=2,v=2,i=9,C=1,q=2", &png(2, 2, &source)),
    );
    let placements = snapshot(&session);
    let p = placements
        .iter()
        .find(|p| p.image_id == 9)
        .expect("png placement");
    let pixels = session.read_kitty_images(&[(9, p.image_generation)]);
    assert_eq!(pixels.len(), 1);
    assert_eq!(pixels[0].format, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA);
    assert_eq!(pixels[0].width, 2);
    assert_eq!(pixels[0].height, 2);
    assert_eq!(pixels[0].pixels, source);
}

#[test]
fn malformed_png_is_rejected_without_a_panic_or_poisoned_storage() {
    let session = session(20, 8);
    let mut bad = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bad.extend_from_slice(&[0u8; 32]);
    feed(&session, &apc("a=T,f=100,s=2,v=2,i=10,C=1,q=2", &bad));
    let placements = snapshot(&session);
    assert!(placements.iter().all(|p| p.image_id != 10));
    // A later valid PNG still works: a failure must not disable the callback.
    feed(
        &session,
        &apc("a=T,f=100,s=1,v=1,i=11,C=1,q=2", &png(1, 1, &[1, 2, 3, 4])),
    );
    assert!(snapshot(&session).iter().any(|p| p.image_id == 11));
}

#[test]
fn zlib_transmission_and_multi_chunk_assembly() {
    let session = session(20, 8);
    let pixels = rgba_pixels(3, 2, [0x10, 0x20, 0x30, 0x40]);
    let compressed = zlib_stored(&pixels);
    feed(&session, &apc("a=t,f=32,s=3,v=2,i=3,o=z,q=2", &compressed));
    // Multi-chunk direct transmission: the control header is only on the
    // first chunk; continuation chunks only repeat `m`.
    let chunks = b64(&rgba_pixels(2, 1, [1, 2, 3, 4]));
    // Chunk boundaries in the Kitty protocol fall on base64 group boundaries:
    // every non-final chunk is a complete base64 encoding fragment.
    let split = (chunks.len() / 2) / 4 * 4;
    assert!(split > 0 && split < chunks.len());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x1b_G");
    bytes
        .extend_from_slice(format!("a=t,f=32,s=2,v=1,i=4,m=1,q=2;{}", &chunks[..split]).as_bytes());
    bytes.extend_from_slice(b"\x1b\\");
    bytes.extend_from_slice(b"\x1b_G");
    bytes.extend_from_slice(format!("m=0;{}", &chunks[split..]).as_bytes());
    bytes.extend_from_slice(b"\x1b\\");
    feed(&session, &bytes);
    feed(&session, &control("a=p,i=3,p=30,q=2"));
    feed(&session, &control("a=p,i=4,p=40,q=2"));
    let placements = snapshot(&session);
    let zlib = placements.iter().find(|p| p.image_id == 3).expect("zlib");
    let chunked = placements
        .iter()
        .find(|p| p.image_id == 4)
        .expect("chunked");
    let read = session.read_kitty_images(&[
        (zlib.image_id, zlib.image_generation),
        (chunked.image_id, chunked.image_generation),
    ]);
    assert_eq!(read.len(), 2);
    assert_eq!(
        read.iter().find(|p| p.image_id == 3).unwrap().pixels,
        pixels
    );
    assert_eq!(
        read.iter().find(|p| p.image_id == 4).unwrap().pixels,
        rgba_pixels(2, 1, [1, 2, 3, 4])
    );
}

#[test]
fn transmit_then_place_uses_placement_ids_and_offsets() {
    let session = session(20, 8);
    transmit_rgba(&session, 5, 2, 2, [1, 2, 3, 4]);
    feed(&session, &control("a=p,i=5,p=77,X=3,Y=4,c=3,r=2,q=2"));
    let placements = snapshot(&session);
    assert_eq!(placements.len(), 1);
    let p = &placements[0];
    assert_eq!(p.image_id, 5);
    assert_eq!(p.placement_id, 77);
    assert_eq!(p.cell_offset_x, 3);
    assert_eq!(p.cell_offset_y, 4);
    assert_eq!(p.grid_cols, 3);
    assert_eq!(p.grid_rows, 2);
}

#[test]
fn replacing_an_image_with_the_same_id_changes_generation_and_pixels() {
    let session = session(20, 8);
    transmit_rgba(&session, 6, 1, 1, [9, 9, 9, 255]);
    feed(&session, &control("a=p,i=6,p=1,q=2"));
    let first = snapshot(&session);
    let gen1 = first[0].image_generation;
    feed(&session, &control("a=d,d=i,i=6,q=2"));
    transmit_rgba(&session, 6, 1, 1, [1, 2, 3, 255]);
    feed(&session, &control("a=p,i=6,p=1,q=2"));
    let second = snapshot(&session);
    assert_eq!(second.len(), 1);
    assert_ne!(second[0].image_generation, gen1);
    let pixels = session.read_kitty_images(&[(6, second[0].image_generation)]);
    assert_eq!(pixels[0].pixels, vec![1, 2, 3, 255]);
    // The old generation is now a normal race, not an error and not pixels.
    assert!(session.read_kitty_images(&[(6, gen1)]).is_empty());
    assert!(session.snapshot_counters().image_generation_races >= 1);
}

#[test]
fn deletion_removes_placements_and_images() {
    let session = session(20, 8);
    transmit_rgba(&session, 7, 1, 1, [1, 1, 1, 1]);
    feed(&session, &control("a=p,i=7,p=7,q=2"));
    let placed = snapshot(&session);
    assert_eq!(placed.len(), 1);

    // Lowercase d=i removes the placement but keeps the image data. Read the
    // generation from a fresh placement rather than hard-coding 1: image
    // generations are not a test-local constant.
    feed(&session, &control("a=d,d=i,i=7,q=2"));
    assert!(snapshot(&session).is_empty());
    feed(&session, &control("a=p,i=7,p=7,q=2"));
    let retained = snapshot(&session);
    assert_eq!(retained.len(), 1);
    let retained_generation = retained[0].image_generation;
    assert_eq!(
        session.read_kitty_images(&[(7, retained_generation)]).len(),
        1
    );

    // Uppercase d=I frees the image data once it has no placements.
    feed(&session, &control("a=d,d=I,i=7,q=2"));
    assert!(snapshot(&session).is_empty());
    assert!(session
        .read_kitty_images(&[(7, retained_generation)])
        .is_empty());
}

#[test]
fn source_rectangles_and_destination_pixels_are_reported_verbatim() {
    let session = session(20, 8);
    let pixels = [
        0xff, 0, 0, 255, 0, 0xff, 0, 255, 0, 0, 0xff, 255, 255, 255, 255, 255,
    ];
    feed(&session, &apc("a=t,f=32,s=2,v=2,i=8,q=2", &pixels));
    feed(&session, &control("a=p,i=8,p=8,x=1,y=0,w=1,h=2,q=2"));
    let placements = snapshot(&session);
    let p = &placements[0];
    assert_eq!(p.source_x, 1);
    assert_eq!(p.source_y, 0);
    assert_eq!(p.source_width, 1);
    assert_eq!(p.source_height, 2);
    // Without explicit c/r, a source rectangle of 1x2 image pixels is drawn at
    // its native pixel size; device pixels are not derived from the cell grid.
    assert_eq!(p.pixel_width, 1);
    assert_eq!(p.pixel_height, 2);
}

#[test]
fn all_three_layers_are_reported_and_sorted_by_layer_then_z() {
    let session = session(20, 8);
    // Below background: z < INT32_MIN/2. Below text: -1. Above text: >= 0.
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=20,z=-1073741825,C=1,q=2",
            &rgba_pixels(1, 1, [1, 0, 0, 255]),
        ),
    );
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=21,z=-1,C=1,q=2",
            &rgba_pixels(1, 1, [0, 1, 0, 255]),
        ),
    );
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=22,z=1,C=1,q=2",
            &rgba_pixels(1, 1, [0, 0, 1, 255]),
        ),
    );
    let placements = snapshot(&session);
    let layers: Vec<i32> = placements.iter().map(|p| p.layer).collect();
    assert_eq!(
        layers,
        vec![
            sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND,
            sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT,
            sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT,
        ]
    );
    let z: Vec<i32> = placements.iter().map(|p| p.z).collect();
    assert_eq!(z, vec![-1073741825, -1, 1]);
}

#[test]
fn alpha_survives_storage_and_placement_geometry_supports_negative_rows() {
    let session = session(20, 8);
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=30,C=1,q=2",
            &rgba_pixels(1, 1, [10, 20, 30, 40]),
        ),
    );
    let placements = snapshot(&session);
    let p = &placements[0];
    let pixels = session.read_kitty_images(&[(30, p.image_generation)]);
    assert_eq!(pixels[0].pixels[3], 40, "alpha must not be premultiplied");
    // Move the placement above the viewport by printing enough lines.
    for _ in 0..12 {
        feed(&session, b"\r\n");
    }
    let after = snapshot(&session);
    if let Some(moved) = after.iter().find(|p| p.image_id == 30) {
        assert!(moved.viewport_row < p.viewport_row);
        assert_eq!(moved.image_generation, p.image_generation);
    }
}

#[test]
fn unicode_placeholder_placement_comes_from_the_render_state() {
    let session = session(20, 8);
    feed(
        &session,
        &apc(
            "a=t,f=32,s=2,v=2,i=1,q=2",
            &rgba_pixels(2, 2, [1, 2, 3, 255]),
        ),
    );
    // Virtual placement: 2 columns x 2 rows, selected by foreground color 1.
    feed(&session, &control("a=p,i=1,U=1,c=2,r=2,q=2"));
    let mut text = String::from("\x1b[38;5;1m");
    for row in ['\u{0305}', '\u{030D}'] {
        for col in ['\u{0305}', '\u{030D}'] {
            text.push('\u{10EEEE}');
            text.push(row);
            text.push(col);
        }
        text.push_str("\x1b[39m\r\n\x1b[38;5;1m");
    }
    text.push_str("\x1b[39m");
    feed(&session, text.as_bytes());
    let placements = snapshot(&session);
    let virtuals: Vec<&ImagePlacement> = placements
        .iter()
        .filter(|p| p.kind == sys::GHOSTTY_RENDER_STATE_KITTY_PLACEMENT_KIND_VIRTUAL)
        .collect();
    assert!(
        !virtuals.is_empty(),
        "placeholder cells must yield virtual placements"
    );
    assert!(virtuals.iter().all(|p| p.image_id == 1));
    assert!(virtuals
        .iter()
        .all(|p| p.layer == sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT));
    assert!(virtuals.iter().all(|p| p.z == -1));
}

#[test]
fn relative_placement_tracks_its_parent() {
    let session = session(20, 8);
    transmit_rgba(&session, 50, 2, 2, [1, 0, 0, 255]);
    transmit_rgba(&session, 51, 1, 1, [0, 1, 0, 255]);
    feed(&session, &control("a=p,i=50,p=500,q=2"));
    feed(&session, &control("a=p,i=51,p=501,P=50,Q=500,H=5,V=5,q=2"));
    let placements = snapshot(&session);
    let relative = placements
        .iter()
        .find(|p| p.image_id == 51)
        .expect("relative placement");
    assert_eq!(
        relative.kind,
        sys::GHOSTTY_RENDER_STATE_KITTY_PLACEMENT_KIND_RELATIVE
    );
    assert!(placements.iter().any(|p| p.image_id == 50));
}

#[test]
fn resize_reflow_and_alternate_screen_keep_the_placement_model_coherent() {
    let session = session(20, 8);
    feed(
        &session,
        &apc(
            "a=T,f=32,s=2,v=2,i=60,C=1,q=2",
            &rgba_pixels(2, 2, [4, 5, 6, 255]),
        ),
    );
    let before = snapshot(&session);
    assert_eq!(before.len(), 1);
    session
        .apply_layout_geometry(30, 12, CELL_W, CELL_H)
        .unwrap();
    let resized = snapshot(&session);
    assert_eq!(resized.len(), 1);
    assert_eq!(resized[0].image_generation, before[0].image_generation);

    // Alternate screen has its own storage.
    feed(&session, b"\x1b[?1049h");
    assert!(snapshot(&session).is_empty());
    feed(
        &session,
        &apc(
            "a=T,f=32,s=1,v=1,i=61,C=1,q=2",
            &rgba_pixels(1, 1, [7, 8, 9, 255]),
        ),
    );
    assert!(snapshot(&session).iter().any(|p| p.image_id == 61));
    feed(&session, b"\x1b[?1049l");
    let primary = snapshot(&session);
    assert!(primary.iter().any(|p| p.image_id == 60));
    assert!(primary.iter().all(|p| p.image_id != 61));
}

#[test]
fn animation_frames_advance_on_tick_and_stop_without_a_loop() {
    let session = session(20, 8);
    // Root frame is red, frame 2 is blue with a 40 ms gap.
    feed(
        &session,
        &apc(
            "a=t,f=32,s=1,v=1,i=70,q=2",
            &rgba_pixels(1, 1, [255, 0, 0, 255]),
        ),
    );
    feed(
        &session,
        &apc(
            "a=f,f=32,s=1,v=1,i=70,r=2,z=40,q=2",
            &rgba_pixels(1, 1, [0, 0, 255, 255]),
        ),
    );
    feed(&session, &control("a=p,i=70,p=700,q=2"));
    let placements = snapshot(&session);
    let gen_before = placements
        .iter()
        .find(|p| p.image_id == 70)
        .unwrap()
        .image_generation;
    feed(&session, &control("a=a,i=70,s=3,q=2"));

    // One tick advances at most one frame and returns the next deadline.
    let next = session
        .tick_kitty_animation(0)
        .expect("tick")
        .expect("deadline");
    assert!(next > 0);
    assert!(session.read_kitty_images(&[(70, gen_before)]).is_empty());
    let placements = snapshot(&session);
    let gen_after = placements
        .iter()
        .find(|p| p.image_id == 70)
        .unwrap()
        .image_generation;
    assert_ne!(gen_after, gen_before);
    let pixels = session.read_kitty_images(&[(70, gen_after)]);
    assert_eq!(pixels[0].pixels, vec![0, 0, 255, 255]);

    // Stopping is immediate; a stale deadline must not resurrect it.
    feed(&session, &control("a=a,i=70,s=1,q=2"));
    assert!(session.tick_kitty_animation(next).expect("tick").is_none());

    // Resuming schedules a new deadline; stopping again clears it.
    feed(&session, &control("a=a,i=70,s=3,q=2"));
    assert!(session
        .tick_kitty_animation(next)
        .expect("resume tick")
        .is_some());
    feed(&session, &control("a=a,i=70,s=1,q=2"));
    assert!(session
        .tick_kitty_animation(next)
        .expect("stop tick")
        .is_none());

    let counters = session.snapshot_counters();
    assert!(counters.kitty_animation_ticks >= 4);
    assert!(counters.kitty_animation_frames >= 1);
}

#[test]
fn animation_composition_merges_a_source_frame() {
    let session = session(20, 8);
    // Root is 2x1 RGB: red then blue.
    feed(
        &session,
        &apc("a=t,f=24,s=2,v=1,i=71,q=2", &[255, 0, 0, 0, 0, 255]),
    );
    // Frame 2 starts as a white pixel on a transparent 2x1 canvas.
    feed(
        &session,
        &apc("a=f,i=71,f=24,s=1,v=1,z=40,q=2", &[255, 255, 255]),
    );
    // Compose root pixel (1,0) onto frame 2 at (0,0).
    feed(&session, &control("a=c,i=71,r=1,c=2,w=1,h=1,X=1,x=0,q=2"));
    feed(&session, &control("a=p,i=71,p=710,q=2"));
    feed(&session, &control("a=a,i=71,s=3,q=2"));

    let before = snapshot(&session)
        .into_iter()
        .find(|p| p.image_id == 71)
        .expect("placement")
        .image_generation;
    let next = session
        .tick_kitty_animation(0)
        .expect("tick")
        .expect("deadline");
    assert!(next > 0);
    let after = snapshot(&session)
        .into_iter()
        .find(|p| p.image_id == 71)
        .expect("placement")
        .image_generation;
    assert_ne!(after, before);
    let pixels = session.read_kitty_images(&[(71, after)]);
    assert_eq!(pixels.len(), 1);
    assert_eq!(pixels[0].format, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA);
    assert_eq!(
        pixels[0].pixels,
        vec![0, 0, 255, 255, 0, 0, 0, 0],
        "composed frame is blue at x=0 and transparent at x=1"
    );
    feed(&session, &control("a=a,i=71,s=1,q=2"));
}

#[test]
fn storage_limit_rejects_oversized_images_and_zero_disables_graphics() {
    let limited = session_with_kitty(
        20,
        8,
        KittyConfig {
            storage_limit_bytes: 8,
            ..KittyConfig::default()
        },
    );
    feed(
        &limited,
        &apc(
            "a=T,f=32,s=2,v=2,i=90,C=1,q=2",
            &rgba_pixels(2, 2, [1, 2, 3, 4]),
        ),
    );
    assert!(snapshot(&limited).is_empty());
    let (limit, file, _shared) = limited.kitty_policy_state().expect("policy");
    assert_eq!(limit, 8);
    assert!(!file);

    let disabled = session_with_kitty(
        20,
        8,
        KittyConfig {
            enabled: false,
            ..KittyConfig::default()
        },
    );
    feed(
        &disabled,
        &apc(
            "a=T,f=32,s=1,v=1,i=91,C=1,q=2",
            &rgba_pixels(1, 1, [1, 2, 3, 4]),
        ),
    );
    assert!(snapshot(&disabled).is_empty());
    let (limit, file, shared) = disabled.kitty_policy_state().expect("policy");
    assert_eq!(limit, 0);
    assert!(!file);
    assert!(!shared);
    assert!(!disabled.kitty_config().is_graphics_enabled());
}

#[test]
fn transport_media_policy_is_explicit_and_finite() {
    let default = KittyConfig::default();
    assert!(default.is_graphics_enabled());
    assert!(!default.arbitrary_file);
    assert!(default.temp_file_directory.is_none());
    assert_eq!(default.shared_memory, cfg!(target_os = "linux"));
    let bounded = KittyConfig {
        storage_limit_bytes: u64::MAX,
        ..KittyConfig::default()
    }
    .validated();
    assert!(bounded.storage_limit_bytes <= KittyConfig::MAX_STORAGE_LIMIT);
    let file_enabled = KittyConfig {
        arbitrary_file: true,
        ..KittyConfig::default()
    }
    .validated();
    assert!(
        file_enabled.arbitrary_file,
        "advanced option must be expressible"
    );

    let session = session_with_kitty(20, 8, file_enabled);
    let (limit, file, _shared) = session.kitty_policy_state().expect("policy");
    assert!(limit > 0);
    assert!(file);
}
