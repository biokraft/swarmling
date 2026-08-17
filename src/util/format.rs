use std::sync::LazyLock;

use regex::Regex;

static SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)([\d.]+)\s*([KMGT]?I?B)").expect("size regex is valid"));

/// Parse a human size as sites render it ("1.5 GiB", "700 MB", "512 B").
/// Binary suffixes are powers of 1024, decimal suffixes powers of 1000.
/// Anything unparseable is 0 — a missing size never fails a search.
pub fn parse_size(s: &str) -> u64 {
    let Some(caps) = SIZE_RE.captures(s) else {
        return 0;
    };
    let value: f64 = caps[1].parse().unwrap_or(0.0);
    let multiplier: f64 = match caps[2].to_uppercase().as_str() {
        "B" => 1.0,
        "KIB" => 1024.0,
        "MIB" => 1024f64.powi(2),
        "GIB" => 1024f64.powi(3),
        "TIB" => 1024f64.powi(4),
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => 1.0,
    };
    (value * multiplier).round() as u64
}

/// Decode the handful of entities the feeds actually emit. Deliberately not a
/// full HTML entity decoder: these are the ones upstream hit in practice.
pub fn unescape_entities(s: &str) -> String {
    s.replace("&#038;", "&")
        .replace("&#38;", "&")
        .replace("&amp;", "&")
        .replace("&#8211;", "-")
        .replace("&#8212;", "-")
        .replace("&#8217;", "'")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_handles_binary_and_decimal_units() {
        assert_eq!(parse_size("1 GiB"), 1_073_741_824);
        assert_eq!(parse_size("1.5 GiB"), 1_610_612_736);
        assert_eq!(parse_size("700 MB"), 700_000_000);
        assert_eq!(parse_size("512 B"), 512);
        assert_eq!(parse_size("2.2GB"), 2_200_000_000);
    }

    #[test]
    fn parse_size_is_case_insensitive_and_fails_soft() {
        assert_eq!(parse_size("4.7 gib"), 5_046_586_573);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("unknown"), 0);
    }

    #[test]
    fn unescape_entities_decodes_the_entities_feeds_use() {
        assert_eq!(unescape_entities("Fish &amp; Chips"), "Fish & Chips");
        assert_eq!(unescape_entities("Fish &#38; Chips"), "Fish & Chips");
        assert_eq!(unescape_entities("a &#8211; b"), "a - b");
        assert_eq!(unescape_entities("it&#8217;s"), "it's");
        assert_eq!(unescape_entities("&#8220;quoted&#8221;"), "\"quoted\"");
        assert_eq!(unescape_entities("&lt;tag&gt;"), "<tag>");
    }
}
