use ratatui::style::Color;

pub const ACCENT: Color = Color::Rgb(0xa7, 0x8b, 0xfa);
pub const TEXT: Color = Color::Rgb(0xe9, 0xe4, 0xf5);
pub const ALT: Color = Color::Rgb(0xb9, 0xa7, 0xe6);
pub const GOOD: Color = Color::Rgb(0x86, 0xd6, 0xa2);
pub const WARN: Color = Color::Rgb(0xf0, 0xc5, 0x60);
pub const BAD: Color = Color::Rgb(0xee, 0x7d, 0x92);
pub const BRIGHT: Color = Color::Rgb(0xd8, 0xb4, 0xfe);
pub const RULE: Color = Color::Rgb(0x6b, 0x65, 0x77);

pub mod icon {
    pub const DONE: &str = "✓";
    pub const ERROR: &str = "✗";
    pub const PENDING: &str = "·";
    pub const POINTER: &str = "❯";
    pub const DOT: &str = "·";
    pub const WARN: &str = "⚠";
    pub const BAR: &str = "▌";
    pub const DOWN: &str = "↓";
    pub const UP: &str = "↑";
    pub const PEER: &str = "•";
    pub const PAUSE: &str = "⏸";
    pub const ASC: &str = "▴";
    pub const DESC: &str = "▾";
}

/// Linear interpolation between two RGB colours. Non-RGB colours (the
/// terminal's own palette entries) have no channel values to blend, so they
/// are returned unchanged rather than guessed at.
pub fn lerp_rgb(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let t = t.clamp(0.0, 1.0);
    let mix =
        |x: u8, y: u8| -> u8 { (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8 };
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

/// The short tag and colour shown in every result, queue, and detail row.
pub fn source_tag(id: &str) -> (&'static str, Color) {
    match id {
        "fitgirl" => ("FG", ACCENT),
        "yts" => ("YTS", GOOD),
        "eztv" => ("EZTV", WARN),
        "nyaa" => ("NYAA", BRIGHT),
        "subsplease" => ("SUB", ALT),
        "tpb-movies" | "tpb-tv" => ("TPB", Color::Rgb(0x5f, 0xd0, 0xc5)),
        "x1337-movies" | "x1337-tv" => ("1337", Color::Rgb(0xf6, 0xa5, 0x5c)),
        "bittorrented" => ("BT", Color::Rgb(0x7d, 0xb8, 0xf0)),
        _ => ("•", ALT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_the_agreed_pastel_violet() {
        assert_eq!(ACCENT, Color::Rgb(0xa7, 0x8b, 0xfa));
        assert_eq!(TEXT, Color::Rgb(0xe9, 0xe4, 0xf5));
        assert_eq!(ALT, Color::Rgb(0xb9, 0xa7, 0xe6));
        assert_eq!(GOOD, Color::Rgb(0x86, 0xd6, 0xa2));
        assert_eq!(WARN, Color::Rgb(0xf0, 0xc5, 0x60));
        assert_eq!(BAD, Color::Rgb(0xee, 0x7d, 0x92));
        assert_eq!(BRIGHT, Color::Rgb(0xd8, 0xb4, 0xfe));
        assert_eq!(RULE, Color::Rgb(0x6b, 0x65, 0x77));
    }

    #[test]
    fn lerp_hits_both_ends_exactly_and_the_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(255, 255, 255);
        assert_eq!(lerp_rgb(a, b, 0.0), a);
        assert_eq!(lerp_rgb(a, b, 1.0), b);
        assert_eq!(lerp_rgb(a, b, 0.5), Color::Rgb(128, 128, 128));
    }

    #[test]
    fn lerp_clamps_out_of_range_factors_instead_of_wrapping() {
        // A factor outside 0..=1 must not produce a garbage colour by
        // overflowing the channel arithmetic.
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(255, 255, 255);
        assert_eq!(lerp_rgb(a, b, -5.0), a);
        assert_eq!(lerp_rgb(a, b, 5.0), b);
    }

    #[test]
    fn every_registered_source_has_its_own_tag() {
        for source in crate::sources::registry::all_sources() {
            let (tag, _) = source_tag(source.id());
            assert_ne!(tag, "•", "{} has no tag", source.id());
            assert!(tag.len() <= 4, "{} tag is too wide", source.id());
        }
    }

    #[test]
    fn an_unknown_source_falls_back_instead_of_panicking() {
        let (tag, colour) = source_tag("not-a-real-source");
        assert_eq!(tag, "•");
        assert_eq!(colour, ALT);
    }
}
