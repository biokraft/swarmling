use std::sync::LazyLock;

use regex::Regex;

const BASE32_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

static XT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)xt=urn:btih:([a-f0-9]{40}|[a-z2-7]{32})").expect("xt regex is valid")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMagnet {
    pub infohash: String,
    pub name: String,
    pub trackers: Vec<String>,
}

fn base32_to_hex(b32: &str) -> Option<String> {
    let mut bits = 0u32;
    let mut value = 0u32;
    let mut out = String::new();
    for c in b32.to_uppercase().bytes() {
        let idx = BASE32_ALPHABET.iter().position(|&a| a == c)? as u32;
        value = (value << 5) | idx;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push_str(&format!("{:02x}", (value >> bits) & 0xff));
            value &= (1 << bits) - 1;
        }
    }
    (out.len() == 40).then_some(out)
}

/// Every infohash downstream is 40-char lowercase hex, whichever form arrived.
pub fn normalize_infohash(raw: &str) -> String {
    if raw.len() == 32 {
        base32_to_hex(raw).unwrap_or_else(|| raw.to_lowercase())
    } else {
        raw.to_lowercase()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.replace('+', " ").into_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn params<'a>(raw: &'a str, key: &str) -> Vec<&'a str> {
    let Some(query) = raw.split_once('?').map(|(_, q)| q) else {
        return vec![];
    };
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
        .collect()
}

/// Whether `s` is a well-formed 40-char hex BitTorrent infohash. The single
/// gate every caller must pass before an infohash reaches a built magnet.
pub fn is_infohash(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Extract the lowercase hex infohash from a magnet URI, if it has one.
///
/// Delegates to `parse_magnet` so magnet handling stays in one place, and
/// filters through `is_infohash` because `normalize_infohash` can return a
/// string that is not a valid hash — an invalid hash must drop the row
/// rather than reach the queue.
pub fn infohash_from_magnet(magnet: &str) -> Option<String> {
    parse_magnet(magnet)
        .map(|m| m.infohash)
        .filter(|h| is_infohash(h))
}

/// Returns None when the input carries no usable infohash — the caller drops
/// the row rather than building a broken magnet.
pub fn parse_magnet(raw: &str) -> Option<ParsedMagnet> {
    let xt = XT_RE.captures(raw)?.get(1)?.as_str();
    Some(ParsedMagnet {
        infohash: normalize_infohash(xt),
        name: params(raw, "dn")
            .first()
            .map(|v| percent_decode(v))
            .unwrap_or_default(),
        trackers: params(raw, "tr")
            .iter()
            .map(|v| percent_decode(v))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_magnet_with_name_and_trackers() {
        let raw = "magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12\
                   &dn=Some%20Show%20-%2001&tr=udp%3A%2F%2Ftracker.example%3A80%2Fannounce";
        let p = parse_magnet(raw).unwrap();
        assert_eq!(p.infohash, "abcdef1234567890abcdef1234567890abcdef12");
        assert_eq!(p.name, "Some Show - 01");
        assert_eq!(
            p.trackers,
            vec!["udp://tracker.example:80/announce".to_string()]
        );
    }

    #[test]
    fn converts_base32_infohash_to_hex() {
        // base32 "VLC3Y7RXTWAAAAAAAAAAAAAAAAAAAAAA" decodes to 20 bytes of hex.
        let p = parse_magnet("magnet:?xt=urn:btih:VLC3Y7RXTWAAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert_eq!(p.infohash.len(), 40);
        assert!(p.infohash.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(p.infohash.chars().all(|c| !c.is_ascii_uppercase()));
    }

    #[test]
    fn rejects_input_without_a_usable_infohash() {
        assert!(parse_magnet("magnet:?dn=no+hash+here").is_none());
        assert!(parse_magnet("not a magnet at all").is_none());
        assert!(parse_magnet("magnet:?xt=urn:btih:tooshort").is_none());
    }

    #[test]
    fn normalize_infohash_lowercases_hex_and_leaves_length() {
        assert_eq!(
            normalize_infohash("ABCDEF1234567890ABCDEF1234567890ABCDEF12"),
            "abcdef1234567890abcdef1234567890abcdef12"
        );
    }

    #[test]
    fn params_matches_keys_case_insensitively() {
        let raw = "magnet:?XT=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12\
                   &DN=Upper+Case&TR=udp%3A%2F%2Ftracker.example%3A80%2Fannounce";
        let p = parse_magnet(raw).unwrap();
        assert_eq!(p.name, "Upper Case");
        assert_eq!(
            p.trackers,
            vec!["udp://tracker.example:80/announce".to_string()]
        );
    }

    #[test]
    fn is_infohash_requires_forty_hex_chars() {
        assert!(is_infohash("abcdef1234567890abcdef1234567890abcdef12"));
        assert!(!is_infohash("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
        assert!(!is_infohash("abcdef"));
    }
}
