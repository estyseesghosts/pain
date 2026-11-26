//! Pure cell geometry and row batching used by `TerminalElement`.
//!
//! This module contains no GPUI or Ghostty calls. It turns the grid snapshot
//! produced by the FFI layer into text runs and quad spans, which keeps the
//! expensive decisions testable without opening a window. Wide-character
//! *display width* is a rendering policy here; the grapheme string itself is
//! always Ghostty's, so this is not a second terminal parser.

use crate::terminal::ghostty::{Cell, ColorSpec, RowSelection};

/// Font-size policy for the Ctrl+= / Ctrl+- commands. Kept here so the clamp
/// is testable without a GPUI window.
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 40.0;
pub const FONT_SIZE_STEP: f32 = 1.0;

#[must_use]
pub fn adjust_font_size(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// Row height for a font's measured line box.
///
/// GPUI centers the glyphs inside the line height passed to `ShapedLine::paint`.
/// Leading beyond `ascent + descent` therefore appears as a gap between
/// terminal rows, which breaks connected box-drawing borders in TUIs. Use the
/// font's own line box and clamp only for degenerate metrics.
#[must_use]
pub fn line_height_for(ascent: f32, descent: f32, font_size: f32) -> f32 {
    (ascent + descent).max(font_size).max(1.0)
}

/// Cell width and line height measured from the resolved GPUI mono font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub cell_width: f32,
    pub line_height: f32,
}

impl CellMetrics {
    #[must_use]
    pub fn new(cell_width: f32, line_height: f32) -> Self {
        assert!(cell_width.is_finite() && cell_width > 0.0);
        assert!(line_height.is_finite() && line_height > 0.0);
        Self {
            cell_width,
            line_height,
        }
    }

    /// Derive a terminal grid from the element's content bounds.
    #[must_use]
    pub fn grid_for(&self, width: f32, height: f32) -> (u16, u16) {
        let cols = (width / self.cell_width).floor() as usize;
        let rows = (height / self.line_height).floor() as usize;
        (
            u16::try_from(cols.max(1)).unwrap_or(u16::MAX),
            u16::try_from(rows.max(1)).unwrap_or(u16::MAX),
        )
    }

    /// Top-left corner of a grid cell, relative to `origin`.
    #[must_use]
    pub fn cell_origin(&self, origin: (f32, f32), col: u16, row: u16) -> (f32, f32) {
        (
            origin.0 + f32::from(col) * self.cell_width,
            origin.1 + f32::from(row) * self.line_height,
        )
    }

    /// Pixel rectangle of a span of `cells` grid columns on one row.
    #[must_use]
    pub fn span_bounds(
        &self,
        origin: (f32, f32),
        col: u16,
        row: u16,
        cells: u16,
    ) -> (f32, f32, f32, f32) {
        let (x, y) = self.cell_origin(origin, col, row);
        (x, y, f32::from(cells) * self.cell_width, self.line_height)
    }
}

/// The one canonical terminal geometry value. Prepaint measures it from the
/// terminal child's actual bounds and font metrics; rendering, hit-testing and
/// resize all consume the same numbers. There is deliberately no second
/// `MouseGeometry`/`ResizeGeometry`: copies drift during live drags.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerminalGeometry {
    /// Origin of the terminal content inside the Silvestris `WindowFrame`.
    pub origin: (f32, f32),
    /// Content bounds (never full window bounds).
    pub content_width: f32,
    pub content_height: f32,
    /// Logical pixels per cell.
    pub cell_width: f32,
    pub cell_height: f32,
    pub cols: u16,
    pub rows: u16,
    /// Device-pixel scale at this layer, when the platform reports one.
    pub scale: f32,
}

/// Input consumes the canonical geometry, not a parallel copy of its numbers.
pub type InputGeometry = TerminalGeometry;

impl TerminalGeometry {
    #[must_use]
    pub fn new(
        origin: (f32, f32),
        (content_width, content_height): (f32, f32),
        metrics: &CellMetrics,
        scale: f32,
    ) -> Self {
        let (cols, rows) = metrics.grid_for(content_width, content_height);
        Self {
            origin,
            content_width,
            content_height,
            cell_width: metrics.cell_width,
            cell_height: metrics.line_height,
            cols,
            rows,
            scale: if scale.is_finite() && scale > 0.0 { scale } else { 1.0 },
        }
    }

    #[must_use]
    pub fn metrics(&self) -> CellMetrics {
        CellMetrics::new(self.cell_width, self.cell_height)
    }

    #[must_use]
    pub fn grid(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Cell under a point in window coordinates, clamped to the last complete
    /// cell. The remainder strips after the final column/row resolve to that
    /// final cell instead of an out-of-range index.
    #[must_use]
    pub fn cell_at(&self, position: (f32, f32)) -> (u16, u16) {
        let clamp = |value: f32, origin: f32, cell: f32, last: u16| -> u16 {
            if !value.is_finite() {
                return 0;
            }
            let raw = ((value - origin) / cell.max(f32::MIN_POSITIVE)).floor();
            if raw <= 0.0 {
                0
            } else if raw >= f32::from(last) {
                last
            } else {
                raw as u16
            }
        };
        (
            clamp(
                position.0,
                self.origin.0,
                self.cell_width,
                self.cols.saturating_sub(1),
            ),
            clamp(
                position.1,
                self.origin.1,
                self.cell_height,
                self.rows.saturating_sub(1),
            ),
        )
    }

    #[must_use]
    pub fn cell_origin(&self, col: u16, row: u16) -> (f32, f32) {
        self.metrics().cell_origin(self.origin, col, row)
    }

    #[must_use]
    pub fn span_bounds(&self, col: u16, row: u16, cells: u16) -> (f32, f32, f32, f32) {
        self.metrics().span_bounds(self.origin, col, row, cells)
    }

    /// Pixel size of the content area, used by Ghostty's mouse encoder.
    #[must_use]
    pub fn content_size_px(&self) -> (u32, u32) {
        (
            (f32::from(self.cols) * self.cell_width).max(1.0) as u32,
            (f32::from(self.rows) * self.cell_height).max(1.0) as u32,
        )
    }

    /// Cell size in terminal/device pixels. Ghostty's `CELL_OFFSET_*`,
    /// `PIXEL_WIDTH/HEIGHT` and size-report geometry all use these units, while
    /// GPUI drawing coordinates stay in logical pixels.
    #[must_use]
    pub fn cell_device_pixels(&self) -> (u32, u32) {
        let convert = |logical: f32| -> u32 {
            let scaled = (logical * self.scale).round();
            if !scaled.is_finite() || scaled < 1.0 {
                1
            } else if scaled >= u32::MAX as f32 {
                u32::MAX
            } else {
                scaled as u32
            }
        };
        (convert(self.cell_width), convert(self.cell_height))
    }
}

/// Everything that forces two adjacent cells into different text runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleKey {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub underline: u16,
    pub foreground: Option<(u8, u8, u8)>,
    pub background: Option<(u8, u8, u8)>,
    pub selected: bool,
}

impl StyleKey {
    #[must_use]
    pub fn from_cell(cell: &Cell) -> Self {
        let flags = cell.style.flags;
        Self {
            bold: cell.style.is_bold(),
            italic: flags & sys_flag::ITALIC != 0,
            faint: flags & sys_flag::FAINT != 0,
            blink: flags & sys_flag::BLINK != 0,
            inverse: flags & sys_flag::INVERSE != 0,
            strikethrough: flags & sys_flag::STRIKETHROUGH != 0,
            underline: cell.style.underline(),
            foreground: cell.foreground,
            background: cell.background,
            selected: cell.selected,
        }
    }
}

/// Bit values mirrored from `GHOSTTY_STYLE_FLAG_*`; kept here so batching tests
/// do not depend on the FFI crate's constants.
mod sys_flag {
    pub const ITALIC: u16 = 1 << 1;
    pub const FAINT: u16 = 1 << 2;
    pub const BLINK: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const STRIKETHROUGH: u16 = 1 << 6;
}

/// A run of adjacent cells that can be shaped with one `shape_line` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRun {
    pub start_col: u16,
    pub cells: u16,
    pub text: String,
    pub style: StyleKey,
}

/// A contiguous run of non-default background cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BgRun {
    pub start_col: u16,
    pub cells: u16,
    pub color: (u8, u8, u8),
}

/// Display columns for a Ghostty grapheme cluster. A wide grapheme occupies
/// its own cell plus the following wide-tail cell; blank cells occupy one
/// column but are never shaped on their own.
#[must_use]
pub fn display_columns(text: &str) -> u16 {
    let Some(first) = text.chars().next() else {
        return 0;
    };
    let cp = u32::from(first);
    let wide = matches!(cp,
        0x1100..=0x115F
        | 0x2329..=0x232A
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD
    );
    if wide {
        2
    } else {
        1
    }
}

/// Batch one row into text runs. Empty cells become spaces inside a run so
/// GPUI's advance keeps later glyphs aligned, but a row of only empty cells
/// produces no runs at all and is never shaped.
#[must_use]
pub fn batch_row(cells: &[Cell]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut col = 0usize;
    while col < cells.len() {
        let cell = &cells[col];
        // A wide tail carries no grapheme of its own and is always absorbed
        // by the preceding wide cell in the inner loop below.
        let style = StyleKey::from_cell(cell);
        let mut text = if cell.text.is_empty() {
            String::from(" ")
        } else {
            cell.text.clone()
        };
        let mut span = 1u16;
        let mut next = col + 1;
        while next < cells.len() {
            let candidate = &cells[next];
            if candidate.text.is_empty() && display_columns(&cells[next - 1].text) == 2 {
                // The tail is a real grid column; keep a space in the shaped
                // string so later glyphs stay aligned even if the fallback
                // font advances the wide grapheme by only one cell.
                text.push(' ');
                span += 1;
                next += 1;
                continue;
            }
            if StyleKey::from_cell(candidate) != style {
                break;
            }
            text.push_str(if candidate.text.is_empty() {
                " "
            } else {
                &candidate.text
            });
            span += 1;
            next += 1;
        }
        if text.bytes().any(|b| b != b' ') {
            runs.push(TextRun {
                start_col: u16::try_from(col).unwrap_or(u16::MAX),
                cells: span,
                text,
                style,
            });
        }
        col = next;
    }
    runs
}

/// Coalesce non-default background colors into spans. Default cells produce
/// no quad because the whole content area is filled once by the element.
#[must_use]
pub fn background_runs(
    cells: &[Cell],
    default: ColorSpec,
    palette: &[(u8, u8, u8); 256],
) -> Vec<BgRun> {
    let resolve = |spec: ColorSpec| -> (u8, u8, u8) {
        match spec {
            ColorSpec::Palette(i) => palette.get(usize::from(i)).copied().unwrap_or((0, 0, 0)),
            ColorSpec::Rgb(r, g, b) => (r, g, b),
        }
    };
    let default_rgb = resolve(default);
    let effective = |cell: &Cell| -> (u8, u8, u8) {
        cell.background
            .or_else(|| cell.style.background.map(resolve))
            .unwrap_or(default_rgb)
    };
    let mut runs = Vec::new();
    let mut col = 0usize;
    while col < cells.len() {
        let color = effective(&cells[col]);
        if color == default_rgb {
            col += 1;
            continue;
        }
        let start = col;
        col += 1;
        while col < cells.len() && effective(&cells[col]) == color {
            col += 1;
        }
        runs.push(BgRun {
            start_col: u16::try_from(start).unwrap_or(u16::MAX),
            cells: u16::try_from(col - start).unwrap_or(u16::MAX),
            color,
        });
    }
    runs
}

/// Selection quad for a row, if any.
#[must_use]
pub fn selection_span(selection: Option<RowSelection>) -> Option<(u16, u16)> {
    selection.map(|s| (s.start_x, s.end_x.saturating_sub(s.start_x) + 1))
}

/// Effective foreground color for a run: Ghostty resolves palette indices,
/// so the resolved RGB on the cell is authoritative. `None` means use the
/// terminal's default foreground from the frame's `Colors`.
#[must_use]
pub fn run_foreground(run: &TextRun) -> Option<(u8, u8, u8)> {
    run.style.foreground
}

/// Returns true when a frame's dirty state means all visible rows must be
/// rebuilt; partial and clean frames can reuse cached rows.
#[must_use]
pub fn is_full_rebuild(dirty: i32, dirty_full: i32) -> bool {
    dirty == dirty_full
}

/// Row-cache decision, kept independent of GPUI so the M1 acceptance rule is
/// testable without a window. Dirty rows from Ghostty are authoritative;
/// dirty PARTIAL rebuilds only those rows, CLEAN rebuilds none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowCache {
    rows: usize,
    generation: u64,
    valid: bool,
}

impl RowCache {
    #[must_use]
    pub fn new(rows: usize) -> Self {
        Self {
            rows,
            generation: 0,
            valid: false,
        }
    }

    /// Return the row indices that must be rebuilt for this frame.
    #[must_use]
    pub fn rows_to_rebuild(
        &mut self,
        generation: u64,
        dirty: i32,
        dirty_rows: &[u16],
    ) -> Vec<usize> {
        // A new output generation with no dirty flags has no visible change,
        // so it must not force a rebuild. Generation is recorded for
        // diagnostics and for the element's whole-frame cache check.
        let _ = generation;
        if !self.valid || is_full_rebuild(dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FULL) {
            self.valid = true;
            self.generation = generation;
            return (0..self.rows).collect();
        }
        if dirty == sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE {
            return Vec::new();
        }
        let mut out: Vec<usize> = dirty_rows
            .iter()
            .map(|y| usize::from(*y))
            .filter(|y| *y < self.rows)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

mod sys {
    pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: i32 = 0;
    pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: i32 = 2;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::ghostty::Style;

    fn cell(text: &str, bold: bool, bg: Option<ColorSpec>) -> Cell {
        Cell {
            text: text.into(),
            style: Style {
                flags: if bold { 1 } else { 0 },
                foreground: None,
                background: bg,
                underline_color: None,
            },
            foreground: None,
            background: None,
            selected: false,
            has_styling: bold,
        }
    }

    #[test]
    fn font_size_steps_are_bounded() {
        assert_eq!(adjust_font_size(12.0, FONT_SIZE_STEP), 13.0);
        assert_eq!(adjust_font_size(12.0, -FONT_SIZE_STEP), 11.0);
        assert_eq!(adjust_font_size(MIN_FONT_SIZE, -10.0), MIN_FONT_SIZE);
        assert_eq!(adjust_font_size(MAX_FONT_SIZE, 10.0), MAX_FONT_SIZE);
    }

    #[test]
    fn grid_and_cell_geometry_derive_from_metrics() {
        let m = CellMetrics::new(9.5, 19.0);
        assert_eq!(m.grid_for(190.0, 380.0), (20, 20));
        assert_eq!(m.grid_for(9.0, 18.0), (1, 1));
        assert_eq!(m.cell_origin((10.0, 20.0), 2, 3), (29.0, 77.0));
        assert_eq!(m.span_bounds((0.0, 0.0), 1, 2, 3), (9.5, 38.0, 28.5, 19.0));
    }

    #[test]
    fn line_height_has_no_extra_leading_between_rows() {
        // Representative mono metrics at 12px: 0.984em ascent + 0.273em descent.
        let ascent = 11.808;
        let descent = 3.276;
        let line_height = line_height_for(ascent, descent, 12.0);
        // A previous 1.25 multiplier gave GPUI 12.5% padding above and below
        // every glyph, which showed up as gaps between TUI borders.
        assert!((line_height - (ascent + descent)).abs() < f32::EPSILON);
        assert!(line_height < (ascent + descent) * 1.05);
    }

    #[test]
    fn line_height_clamps_degenerate_metrics() {
        assert_eq!(line_height_for(2.0, 1.0, 12.0), 12.0);
        assert_eq!(line_height_for(0.0, 0.0, 0.0), 1.0);
        assert_eq!(line_height_for(f32::NAN, f32::NAN, 12.0), 12.0);
    }

    #[test]
    fn adjacent_same_style_cells_share_one_run() {
        let cells = vec![cell("H", false, None), cell("i", false, None)];
        let runs = batch_row(&cells);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hi");
        assert_eq!((runs[0].start_col, runs[0].cells), (0, 2));
    }

    #[test]
    fn style_change_splits_runs() {
        let cells = vec![
            cell("a", false, None),
            cell("B", true, None),
            cell("c", false, None),
        ];
        let runs = batch_row(&cells);
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["a", "B", "c"]
        );
        assert!(runs[1].style.bold);
    }

    #[test]
    fn empty_row_is_never_shaped() {
        let cells = vec![cell("", false, None); 80];
        assert!(batch_row(&cells).is_empty());
    }

    #[test]
    fn wide_grapheme_absorbs_its_tail() {
        let cells = vec![
            cell("界", false, None),
            cell("", false, None),
            cell("x", false, None),
        ];
        assert_eq!(display_columns("界"), 2);
        let runs = batch_row(&cells);
        // Same-style neighbours share one run; the tail is a space in the
        // shaped string so `x` stays in its own grid column.
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "界 x");
        assert_eq!(runs[0].cells, 3);
        assert_eq!(runs[0].start_col, 0);
    }

    #[test]
    fn wide_tail_does_not_start_a_new_run() {
        let cells = vec![
            cell("界", false, None),
            cell("", false, None),
            cell("x", true, None),
        ];
        let runs = batch_row(&cells);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "界 ");
        assert_eq!(runs[0].cells, 2);
        assert_eq!(runs[1].start_col, 2);
        assert_eq!(runs[1].text, "x");
    }

    #[test]
    fn backgrounds_coalesce_and_skip_defaults() {
        let mut palette = [(0, 0, 0); 256];
        palette[1] = (9, 8, 7);
        palette[4] = (1, 2, 3);
        let cells = vec![
            cell("a", false, Some(ColorSpec::Palette(1))),
            cell("b", false, Some(ColorSpec::Palette(1))),
            cell("c", false, None),
        ];
        let runs = background_runs(&cells, ColorSpec::Palette(4), &palette);
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].start_col, runs[0].cells), (0, 2));
        assert_eq!(runs[0].color, (9, 8, 7));
    }

    #[test]
    fn row_cache_reuses_clean_frames_and_rebuilds_only_dirty_rows() {
        let mut cache = RowCache::new(24);
        // First frame has no valid cache: rebuild everything.
        assert_eq!(cache.rows_to_rebuild(1, 2, &[]).len(), 24);
        // Same generation, clean frame: no row is rebuilt.
        assert!(cache.rows_to_rebuild(1, 0, &[]).is_empty());
        // Partial frame: only Ghostty's dirty rows are rebuilt.
        assert_eq!(cache.rows_to_rebuild(2, 1, &[3, 9, 3]), vec![3, 9]);
        // A new output generation with no dirty rows still reuses the cache.
        assert!(cache.rows_to_rebuild(3, 0, &[]).is_empty());
        // Full dirty forces a complete rebuild.
        assert_eq!(cache.rows_to_rebuild(4, 2, &[1]).len(), 24);
    }

    #[test]
    fn canonical_geometry_clamps_partial_edge_strips_to_last_cell() {
        let metrics = CellMetrics::new(9.5, 19.0);
        let g = TerminalGeometry::new((10.0, 20.0), (96.0, 60.0), &metrics, 1.25);
        assert_eq!(g.grid(), (10, 3));
        assert_eq!(g.scale, 1.25);
        // Left/top outside, and the exact origin, both resolve to cell zero.
        assert_eq!(g.cell_at((-50.0, -50.0)), (0, 0));
        assert_eq!(g.cell_at((10.0, 20.0)), (0, 0));
        // The last complete cells start at 10+9*9.5 and 20+2*19.
        assert_eq!(g.cell_at((10.0 + 9.0 * 9.5, 20.0 + 2.0 * 19.0)), (9, 2));
        // Remainder strips after the last complete cell must clamp, not spill.
        assert_eq!(g.cell_at((10.0 + 96.0, 20.0 + 60.0)), (9, 2));
        assert_eq!(g.cell_at((10000.0, 10000.0)), (9, 2));
        assert_eq!(g.cell_at((f32::NAN, f32::INFINITY)), (0, 0));
    }

    #[test]
    fn geometry_tracks_horizontal_vertical_and_same_grid_changes() {
        let metrics = CellMetrics::new(10.0, 20.0);
        // Pure horizontal drag: rows stay fixed, columns cross boundaries.
        let a = TerminalGeometry::new((0.0, 0.0), (800.0, 240.0), &metrics, 1.0);
        let b = TerminalGeometry::new((7.0, 0.0), (825.0, 240.0), &metrics, 1.0);
        assert_eq!(a.grid(), (80, 12));
        assert_eq!(b.grid(), (82, 12));
        assert_eq!(a.cell_at((795.0, 5.0)), (79, 0));
        assert_eq!(b.cell_at((795.0, 5.0)), (78, 0));
        // Pure vertical resize.
        let c = TerminalGeometry::new((7.0, 0.0), (825.0, 300.0), &metrics, 1.0);
        assert_eq!(c.grid(), (82, 15));
        // Pixel-only resize inside the same grid keeps cols/rows and cell_at.
        let d = TerminalGeometry::new((7.0, 0.0), (824.0, 319.0), &metrics, 1.0);
        assert_eq!(c.grid(), d.grid());
        assert_eq!(c.cell_at((800.0, 290.0)), d.cell_at((800.0, 290.0)));
        // Minimum-size window is a valid 1x1 grid with no negative indices.
        let tiny = TerminalGeometry::new((0.0, 0.0), (1.0, 1.0), &metrics, 1.0);
        assert_eq!(tiny.grid(), (1, 1));
        assert_eq!(tiny.cell_at((500.0, 500.0)), (0, 0));
        // Font change: same content bounds, different cell pixels and grid.
        let big = CellMetrics::new(20.0, 40.0);
        let e = TerminalGeometry::new((0.0, 0.0), (800.0, 240.0), &big, 1.0);
        assert_eq!(e.grid(), (40, 6));
        assert_ne!(a.cell_width, e.cell_width);
    }

    #[test]
    fn selection_span_is_inclusive() {
        assert_eq!(
            selection_span(Some(RowSelection {
                start_x: 3,
                end_x: 6
            })),
            Some((3, 4))
        );
        assert_eq!(selection_span(None), None);
    }

    #[test]
    fn device_cell_pixels_follow_scale_while_the_grid_stays_put() {
        let metrics = CellMetrics::new(10.0, 20.0);
        let bounds = (1000.0, 500.0);
        let mut previous_pixels = None;
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let geometry = TerminalGeometry::new((0.0, 0.0), bounds, &metrics, scale);
            // Same logical bounds and metrics: DPI never changes the grid.
            assert_eq!(geometry.grid(), (100, 25), "grid changed at scale {scale}");
            let (width, height) = geometry.cell_device_pixels();
            assert_eq!(width, (10.0 * scale).round() as u32);
            assert_eq!(height, (20.0 * scale).round() as u32);
            assert!(width >= 1 && height >= 1);
            if let Some(previous) = previous_pixels {
                assert_ne!((width, height), previous, "scale {scale} was ignored");
            }
            previous_pixels = Some((width, height));
        }
        let at_125 = TerminalGeometry::new((0.0, 0.0), bounds, &metrics, 1.25);
        assert_eq!(at_125.cell_device_pixels(), (13, 25));
        let at_150 = TerminalGeometry::new((0.0, 0.0), bounds, &metrics, 1.5);
        assert_eq!(at_150.cell_device_pixels(), (15, 30));
    }

    #[test]
    fn device_cell_pixels_never_returns_zero() {
        let metrics = CellMetrics::new(0.4, 0.4);
        let geometry = TerminalGeometry::new((0.0, 0.0), (10.0, 10.0), &metrics, 0.25);
        assert_eq!(geometry.cell_device_pixels(), (1, 1));
    }
}
