use async_trait::async_trait;

use super::magnet::is_infohash;

pub enum SourceGroup {
    Games,
    Movies,
    Tv,
    Anime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub magnet: String,
    pub size_bytes: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub source_id: &'static str,
}

#[derive(thiserror::Error, Debug)]
pub enum SourceError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("unexpected response: {0}")]
    BadResponse(String),
}

#[async_trait]
pub trait Source: Send + Sync {
    fn id(&self) -> &'static str;
    fn groups(&self) -> &'static [SourceGroup];
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError>;
}

/// Public trackers attached to every magnet we build, so a torrent is findable
/// even where DHT is blocked. HTTP(S) entries are deliberate: DHT and udp://
/// are both UDP, which some VPN exits and strict NATs mangle.
pub const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.dler.org:6969/announce",
    "http://tracker.opentrackr.org:1337/announce",
    "http://tracker.openbittorrent.com:80/announce",
    "http://tracker.dler.org:6969/announce",
    "https://tracker.tamersunion.org:443/announce",
];

pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `extra_trackers` come first and win on duplicates: a torrent that carries its
/// own announce list means that list, and the public defaults are a fallback.
///
/// The single chokepoint that guarantees a well-formed magnet: an `infohash`
/// that isn't 40 hex characters is rejected outright (`None`) rather than
/// interpolated unescaped into the URI, so a hostile or malformed feed value
/// can never inject extra `&tr=`/`&dn=` parameters or emit a broken magnet.
pub fn build_magnet(infohash: &str, name: &str, extra_trackers: &[String]) -> Option<String> {
    if !is_infohash(infohash) {
        return None;
    }
    let mut magnet = format!(
        "magnet:?xt=urn:btih:{}&dn={}",
        infohash.to_lowercase(),
        percent_encode(name)
    );
    let mut seen = std::collections::HashSet::new();
    let all = extra_trackers
        .iter()
        .map(|t| t.trim())
        .chain(DEFAULT_TRACKERS.iter().copied());
    for tracker in all {
        if tracker.is_empty() || !seen.insert(tracker.to_string()) {
            continue;
        }
        magnet.push_str("&tr=");
        magnet.push_str(&percent_encode(tracker));
    }
    Some(magnet)
}

pub fn magnet_from_infohash(infohash: &str, name: &str) -> Option<String> {
    build_magnet(infohash, name, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_magnet_appends_default_trackers() {
        let m = build_magnet("ABCDEF1234567890ABCDEF1234567890ABCDEF12", "Cool ISO", &[])
            .expect("valid infohash builds a magnet");
        assert!(m.starts_with(
            "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Cool%20ISO"
        ));
        assert!(m.contains("&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"));
        assert_eq!(m.matches("&tr=").count(), DEFAULT_TRACKERS.len());
    }

    #[test]
    fn build_magnet_puts_extra_trackers_first_and_dedupes() {
        let extra = vec![
            "udp://private.example:6969/announce".to_string(),
            "udp://tracker.opentrackr.org:1337/announce".to_string(),
        ];
        let m = build_magnet("ABCDEF1234567890ABCDEF1234567890ABCDEF12", "X", &extra).unwrap();
        let first_tr = m.find("&tr=").unwrap();
        assert!(m[first_tr..].starts_with("&tr=udp%3A%2F%2Fprivate.example%3A6969%2Fannounce"));
        // the duplicate of a default tracker must not appear twice
        assert_eq!(
            m.matches("udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce")
                .count(),
            1
        );
        assert_eq!(m.matches("&tr=").count(), DEFAULT_TRACKERS.len() + 1);
    }

    #[test]
    fn magnet_from_infohash_still_builds_uri() {
        let m =
            magnet_from_infohash("ABCDEF1234567890ABCDEF1234567890ABCDEF12", "Cool ISO").unwrap();
        assert!(m.starts_with(
            "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Cool%20ISO"
        ));
    }

    #[test]
    fn build_magnet_rejects_an_injection_shaped_infohash() {
        // A hostile feed row's infohash field can't smuggle extra magnet params.
        let hostile = "x&tr=udp://attacker.example/announce";
        assert_eq!(build_magnet(hostile, "Name", &[]), None);
    }

    #[test]
    fn build_magnet_rejects_forty_char_non_hex_string() {
        let bogus = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert_eq!(bogus.len(), 40);
        assert_eq!(build_magnet(bogus, "Name", &[]), None);
    }
}
