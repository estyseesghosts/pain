//! Pure selection-gesture and wheel-scroll policy.
//!
//! Ghostty owns selection snapshots and the viewport; this module only decides
//! which *visible* cells a double/triple click covers and how many rows a wheel
//! event means. Keeping it free of GPUI and FFI makes the policies testable in
//! headless builds and prevents a second terminal model from appearing.

use crate::terminal::ghostty::Cell;
use crate::terminal::geometry::display_columns;

/// Gesture selected by the number of consecutive clicks on one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionGesture {
    Character,
    Word,
    Line,
}

impl SelectionGesture {
    #[must_use]
    pub fn from_click_count(click_count: usize) -> Self {
        match click_count {
            0..=1 => Self::Character,
            2 => Self::Word,
            _ => Self::Line,
        }
    }
}

/// Inclusive column range for a double click. A word is a run of non-blank
/// graphemes; the empty wide-character tail cell belongs to its neighbour.
#[must_use]
pub fn word_bounds(cells: &[Cell], col: usize) -> (usize, usize) {
    if cells.is_empty() {
        return (0, 0);
    }
    let col = col.min(cells.len() - 1);
    // Treat the empty trailing cell of a wide grapheme as part of the word it
    // follows. A run of two or more blanks is padding, not a wide tail.
    let mut in_word = vec![false; cells.len()];
    let mut index = 0usize;
    while index < cells.len() {
        if cells[index].text.trim().is_empty() {
            index += 1;
            continue;
        }
        in_word[index] = true;
        // Only the tail of an actually wide grapheme belongs to the word;
        // ordinary blank padding ends it.
        let next = index + 1;
        if display_columns(&cells[index].text) == 2
            && next < cells.len()
            && cells[next].text.trim().is_empty()
        {
            in_word[next] = true;
            index = next + 1;
        } else {
            index = next;
        }
    }
    if !in_word[col] {
        return (col, col);
    }
    let mut start = col;
    while start > 0 && in_word[start - 1] {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < cells.len() && in_word[end + 1] {
        end += 1;
    }
    (start, end)
}

/// Inclusive column range for a triple click: the used part of the row.
#[must_use]
pub fn line_bounds(cells: &[Cell]) -> (usize, usize) {
    if cells.is_empty() {
        return (0, 0);
    }
    let mut start = 0usize;
    while start < cells.len() && cells[start].text.trim().is_empty() {
        start += 1;
    }
    if start == cells.len() {
        return (0, cells.len() - 1);
    }
    let mut end = cells.len() - 1;
    while end > start && cells[end].text.trim().is_empty() {
        end -= 1;
    }
    (start, end)
}

/// GPUI reports positive Y for wheel up. Ghostty's viewport delta is negative
/// for up, so invert here. One notch is one row.
#[must_use]
pub fn wheel_to_ghostty_delta(up_rows: f64) -> isize {
    -(up_rows.round() as isize)
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
    use crate::terminal::ghostty::Style;

    fn cells(text: &[&str]) -> Vec<Cell> {
        text.iter()
            .map(|text| Cell {
                text: (*text).to_owned(),
                style: Style::default(),
                foreground: Some((255, 255, 255)),
                background: None,
                selected: false,
                has_styling: false,
            })
            .collect()
    }

    #[test]
    fn click_count_selects_gesture() {
        assert_eq!(SelectionGesture::from_click_count(1), SelectionGesture::Character);
        assert_eq!(SelectionGesture::from_click_count(2), SelectionGesture::Word);
        assert_eq!(SelectionGesture::from_click_count(3), SelectionGesture::Line);
        assert_eq!(SelectionGesture::from_click_count(9), SelectionGesture::Line);
    }

    #[test]
    fn double_click_selects_one_word() {
        let row = cells(&[" ", "f", "o", "o", " ", "b", "a", "r"]);
        assert_eq!(word_bounds(&row, 2), (1, 3));
        assert_eq!(word_bounds(&row, 6), (5, 7));
        assert_eq!(word_bounds(&row, 0), (0, 0));
        assert_eq!(word_bounds(&[], 3), (0, 0));
    }

    #[test]
    fn wide_tail_does_not_split_a_word() {
        // Ghostty reports each two-column grapheme as text plus an empty tail.
        let row = cells(&["漢", "", "字", "", " "]);
        assert_eq!(word_bounds(&row, 2), (0, 3));
        assert_eq!(word_bounds(&row, 4), (4, 4));
    }

    #[test]
    fn triple_click_selects_used_part_of_row() {
        let padded = cells(&[" ", " ", "l", "s", " ", " "]);
        assert_eq!(line_bounds(&padded), (2, 3));
        let blank = cells(&[" ", " "]);
        assert_eq!(line_bounds(&blank), (0, 1));
        assert_eq!(line_bounds(&[]), (0, 0));
    }

    #[test]
    fn wheel_direction_matches_ghostty_viewport() {
        assert_eq!(wheel_to_ghostty_delta(3.0), -3); // up into history
        assert_eq!(wheel_to_ghostty_delta(-2.0), 2); // down to active area
        assert_eq!(wheel_to_ghostty_delta(0.4), 0);
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
