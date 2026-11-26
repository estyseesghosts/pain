//! Pure wheel-scroll and scrollbar policy.
//!
//! Ghostty owns selection snapshots and the viewport. Word/line click behavior
//! is now Ghostty's `GhosttySelectionGesture`, not a second Rust word-boundary
//! algorithm; this module only converts wheel deltas and maps a scrollbar thumb
//! position to a Ghostty viewport row.

/// GPUI reports positive Y for wheel up. Ghostty's viewport delta is negative
/// for up, so invert here. One notch is one row.
#[must_use]
pub fn wheel_to_ghostty_delta(up_rows: f64) -> isize {
    -(up_rows.round() as isize)
}

/// Terminal protocols report wheel up/down as buttons 4/5.
#[must_use]
pub fn wheel_button(up: bool) -> i32 {
    if up {
        crate::terminal::ghostty::ghostty_vt_sys::GHOSTTY_MOUSE_BUTTON_FOUR
    } else {
        crate::terminal::ghostty::ghostty_vt_sys::GHOSTTY_MOUSE_BUTTON_FIVE
    }
}

/// Scrollbar geometry derived purely from Ghostty's row counts. This is a view
/// over Ghostty state, never a second scroll model. `x`/`y` are the content
/// origin and `width`/`height` the content bounds in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarGeometry {
    pub track_x: f32,
    pub track_y: f32,
    pub track_width: f32,
    pub track_height: f32,
    pub thumb_y: f32,
    pub thumb_height: f32,
}

#[must_use]
pub fn scrollbar_geometry(
    total: u64,
    offset: u64,
    len: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<ScrollbarGeometry> {
    if total <= len || len == 0 || height <= 0.0 {
        return None;
    }
    let track_width = 8.0_f32.min(width.max(1.0));
    let track_x = x + width - track_width;
    let thumb_height = ((len as f64 / total as f64) * f64::from(height))
        .max(24.0)
        .min(f64::from(height)) as f32;
    let max_offset = total - len;
    let travel = height - thumb_height;
    let thumb_y =
        y + ((offset.min(max_offset) as f64 / max_offset as f64) * f64::from(travel)) as f32;
    Some(ScrollbarGeometry {
        track_x,
        track_y: y,
        track_width,
        track_height: height,
        thumb_y,
        thumb_height,
    })
}

/// Map a pointer Y position on the track to an absolute Ghostty scrollbar row.
#[must_use]
pub fn scrollbar_row_at(
    geometry: &ScrollbarGeometry,
    pointer_y: f32,
    total: u64,
    len: u64,
) -> usize {
    let max_offset = total.saturating_sub(len);
    if max_offset == 0 {
        return 0;
    }
    let travel = (geometry.track_height - geometry.thumb_height).max(1.0);
    let fraction = f64::from(
        ((pointer_y - geometry.thumb_height / 2.0 - geometry.track_y) / travel).clamp(0.0, 1.0),
    );
    (fraction * max_offset as f64) as usize
}

/// Convert a pixel wheel delta into a whole Ghostty viewport delta while
/// keeping the fractional remainder for the next event. Continuous trackpads
/// need this; a wheel notch normally arrives as whole lines already.
#[must_use]
pub fn accumulate_wheel(remainder: f64, up_pixels: f64, line_height_px: f32) -> (isize, f64) {
    let line_height = f64::from(line_height_px.max(1.0));
    let total = remainder + up_pixels / line_height;
    let whole = total.trunc();
    (wheel_to_ghostty_delta(whole), total - whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_direction_matches_ghostty_viewport() {
        assert_eq!(wheel_to_ghostty_delta(3.0), -3); // up into history
        assert_eq!(wheel_to_ghostty_delta(-2.0), 2); // down to active area
        assert_eq!(wheel_to_ghostty_delta(0.4), 0);
    }

    #[test]
    fn scrollbar_is_a_view_over_ghostty_rows() {
        assert_eq!(
            wheel_button(true),
            crate::terminal::ghostty::ghostty_vt_sys::GHOSTTY_MOUSE_BUTTON_FOUR
        );
        assert_eq!(
            wheel_button(false),
            crate::terminal::ghostty::ghostty_vt_sys::GHOSTTY_MOUSE_BUTTON_FIVE
        );
        assert!(scrollbar_geometry(100, 0, 100, 0.0, 0.0, 400.0, 200.0).is_none());
        let geometry =
            scrollbar_geometry(200, 50, 100, 0.0, 0.0, 400.0, 200.0).expect("history is present");
        assert!((geometry.track_x - 392.0).abs() < f32::EPSILON);
        assert!((geometry.thumb_height - 100.0).abs() < f32::EPSILON);
        assert!((geometry.thumb_y - 50.0).abs() < f32::EPSILON);
        assert_eq!(scrollbar_row_at(&geometry, 100.0, 200, 100), 50);
        assert_eq!(scrollbar_row_at(&geometry, 0.0, 200, 100), 0);
        assert_eq!(scrollbar_row_at(&geometry, 200.0, 200, 100), 100);
    }

    #[test]
    fn pixel_wheel_keeps_fractional_remainder() {
        let (rows, rest) = accumulate_wheel(0.0, 15.0, 10.0);
        assert_eq!(rows, -1);
        assert!((rest - 0.5).abs() < f64::EPSILON);
        let (rows, rest) = accumulate_wheel(rest, 5.0, 10.0);
        assert_eq!(rows, -1);
        assert!(rest.abs() < f64::EPSILON);
        // Downward scroll uses the same accumulation with a negative delta.
        let (rows, rest) = accumulate_wheel(0.0, -25.0, 10.0);
        assert_eq!(rows, 2);
        assert!((rest - -0.5).abs() < f64::EPSILON);
    }
}
