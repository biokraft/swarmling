use async_trait::async_trait;

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

pub fn magnet_from_infohash(infohash: &str, name: &str) -> String {
    let mut dn = String::new();
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                dn.push(b as char)
            }
            _ => dn.push_str(&format!("%{b:02X}")),
        }
    }
    format!("magnet:?xt=urn:btih:{}&dn={}", infohash.to_lowercase(), dn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_from_infohash_builds_uri() {
        let m = magnet_from_infohash("ABCDEF1234567890ABCDEF1234567890ABCDEF12", "Cool ISO");
        assert_eq!(
            m,
            "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Cool%20ISO"
        );
    }
}
