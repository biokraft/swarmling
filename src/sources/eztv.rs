use serde::Deserialize;

use super::magnet::parse_magnet;
use super::{build_magnet, SearchResult, Source, SourceError, SourceGroup};

#[derive(Deserialize)]
#[serde(untagged)]
enum Size {
    Text(String),
    Number(u64),
}

impl Size {
    fn bytes(&self) -> u64 {
        match self {
            Size::Text(s) => s.parse().unwrap_or(0),
            Size::Number(n) => *n,
        }
    }
}

#[derive(Deserialize)]
struct Torrent {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    magnet_url: Option<String>,
    #[serde(default)]
    seeds: u32,
    #[serde(default)]
    peers: u32,
    #[serde(default)]
    size_bytes: Option<Size>,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    torrents: Vec<Torrent>,
}

pub struct Eztv {
    pub base_url: String,
}

impl Eztv {
    pub fn new() -> Self {
        Self {
            base_url: "https://eztvx.to".into(),
        }
    }
}

impl Default for Eztv {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Source for Eztv {
    fn id(&self) -> &'static str {
        "eztv"
    }
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Tv]
    }
    /// The EZTV API has no search endpoint, so this source only answers the
    /// browse case (empty query). A real query is another source's job.
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        if !query.trim().is_empty() {
            return Ok(vec![]);
        }
        let env: Envelope = crate::util::net::client()
            .get(format!("{}/api/get-torrents", self.base_url))
            .query(&[("limit", "100"), ("page", "1")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut out = Vec::new();
        for t in env.torrents {
            let hash = t.hash.unwrap_or_default().to_lowercase();
            if hash.is_empty() {
                continue;
            }
            let title = t
                .title
                .filter(|s| !s.is_empty())
                .or_else(|| t.filename.filter(|s| !s.is_empty()))
                .unwrap_or_else(|| hash.clone());
            // Keep the site's own trackers (if any were embedded in its magnet)
            // and still add the shared defaults, matching every other source.
            let extra_trackers = t
                .magnet_url
                .as_deref()
                .filter(|m| !m.is_empty())
                .and_then(parse_magnet)
                .map(|parsed| parsed.trackers)
                .unwrap_or_default();
            let Some(magnet) = build_magnet(&hash, &title, &extra_trackers) else {
                continue;
            };
            out.push(SearchResult {
                title,
                magnet,
                size_bytes: t.size_bytes.map(|s| s.bytes()).unwrap_or(0),
                seeders: t.seeds,
                leechers: t.peers,
                source_id: "eztv",
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn browse_parses_torrents() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/get-torrents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "torrents": [
                    {"title": "Show S01E01", "hash": "ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                     "magnet_url": "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Show",
                     "seeds": 12, "peers": 3, "size_bytes": "734003200"},
                    {"title": "No Hash", "seeds": 1, "peers": 0, "size_bytes": 0}
                ]
            })))
            .mount(&server)
            .await;
        let mut src = Eztv::new();
        src.base_url = server.uri();
        let out = src.search("").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Show S01E01");
        assert_eq!(out[0].seeders, 12);
        assert_eq!(out[0].size_bytes, 734_003_200);
        assert_eq!(out[0].source_id, "eztv");
    }

    #[tokio::test]
    async fn a_query_returns_empty_without_requesting() {
        // No mock is mounted: any HTTP call would fail the test.
        let server = MockServer::start().await;
        let mut src = Eztv::new();
        src.base_url = server.uri();
        assert!(src.search("breaking bad").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn keeps_the_sites_own_tracker_and_adds_the_defaults() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/get-torrents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "torrents": [
                    {"title": "Show S01E01", "hash": "ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                     "magnet_url": "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12\
                        &tr=udp%3A%2F%2Feztv.example%3A80%2Fannounce",
                     "seeds": 12, "peers": 3, "size_bytes": "734003200"}
                ]
            })))
            .mount(&server)
            .await;
        let mut src = Eztv::new();
        src.base_url = server.uri();
        let out = src.search("").await.unwrap();
        assert_eq!(out.len(), 1);
        // the site's own tracker survives...
        assert!(out[0]
            .magnet
            .contains("udp%3A%2F%2Feztv.example%3A80%2Fannounce"));
        // ...and the shared defaults are still appended.
        assert!(out[0]
            .magnet
            .contains("udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"));
    }
}
