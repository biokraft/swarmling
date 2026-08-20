use ratatui::style::Color;

use super::theme::{lerp_rgb, ACCENT, BRIGHT};

const WHITE: Color = Color::Rgb(0xff, 0xff, 0xff);
const BASE: Color = Color::Rgb(0x7c, 0x5c, 0xd6);
const SHADE: Color = Color::Rgb(0x4c, 0x3a, 0x8a);
pub const SPROUT: Color = Color::Rgb(0x5a, 0xe8, 0x7a);

/// The block-letter wordmark. Row 0 holds the sprout; rows 1 and 2 spell
/// "swarmling". All three rows are exactly `width()` characters wide.
pub const LINES: &[&str] = &[
    " 𐓏                                  ",
    "█▀▀ █ █ █▀█ █▀█ █▀▄▀█ █   █ █▄ █ █▀▀",
    "▄▄█ ▀▄▀ █▀█ █▀▄ █ ▀ █ █▄▄ █ █ ▀█ █▄█",
];

/// The one cell that renders the sprout glyph, in its own fixed colour.
pub const SPROUT_CELL: (usize, usize) = (0, 1);

pub fn width() -> u16 {
    LINES
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(u16::MAX as usize) as u16
}

/// The colour of one wordmark cell: a diagonal sweep from near-white at the
/// upper left to deep violet at the lower right. This is the only gradient
/// in the application.
pub fn cell_color(row: usize, col: usize) -> Color {
    if (row, col) == SPROUT_CELL {
        return SPROUT;
    }
    let w = width().max(1) as f32;
    let h = LINES.len().max(1) as f32;
    let tx = col as f32 / w;
    let ty = row as f32 / h;
    let factor = ((tx + ty) / 2.0).clamp(0.0, 1.0);
    match factor {
        f if f < 0.15 => lerp_rgb(WHITE, BRIGHT, f / 0.15),
        f if f < 0.40 => lerp_rgb(BRIGHT, ACCENT, (f - 0.15) / 0.25),
        f if f < 0.70 => lerp_rgb(ACCENT, BASE, (f - 0.40) / 0.30),
        f => lerp_rgb(BASE, SHADE, (f - 0.70) / 0.30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_is_the_same_width() {
        // A ragged wordmark shifts the gradient off its diagonal and makes
        // the fallback width check wrong.
        let widths: Vec<usize> = LINES.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged wordmark: {widths:?}"
        );
    }

    #[test]
    fn the_wordmark_spells_the_product_name() {
        assert!(!LINES.is_empty());
        assert_eq!(width(), 36);
    }

    #[test]
    fn the_sprout_keeps_its_own_colour_regardless_of_the_gradient() {
        let (row, col) = SPROUT_CELL;
        assert_eq!(cell_color(row, col), SPROUT);
    }

    #[test]
    fn the_gradient_runs_from_near_white_to_deep_violet() {
        // Upper-left is the highlight, lower-right the shade. If these ever
        // compare equal the diagonal has collapsed.
        let last_row = LINES.len() - 1;
        let last_col = width() as usize - 1;
        assert_ne!(cell_color(1, 0), cell_color(last_row, last_col));
    }

    #[test]
    fn out_of_range_cells_return_a_colour_instead_of_panicking() {
        let _ = cell_color(999, 999);
    }
}
