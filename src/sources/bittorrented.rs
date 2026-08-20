use serde::Deserialize;

use super::magnet::{infohash_from_magnet, is_infohash};
use super::{build_magnet, SearchResult, Source, SourceError, SourceGroup};

/// The API rejects anything shorter, so an empty browse returns nothing
/// rather than erroring.
const MIN_QUERY: usize = 3;

#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    torrent_infohash: Option<String>,
    #[serde(default)]
    torrent_name: Option<String>,
    #[serde(default)]
    torrent_total_size: Option<u64>,
    #[serde(default)]
    torrent_seeders: Option<u32>,
    #[serde(default)]
    torrent_leechers: Option<u32>,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    results: Vec<Row>,
}

pub struct Bittorrented {
    pub base_url: String,
}

impl Bittorrented {
    pub fn new() -> Self {
        Self {
            base_url: "https://bittorrented.com".into(),
        }
    }
}

impl Default for Bittorrented {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Source for Bittorrented {
    fn id(&self) -> &'static str {
        "bittorrented"
    }
    /// A general index with a large DHT crawl. Only its video type is taken, so
    /// Games stays FitGirl's alone and Anime stays with its dedicated sources —
    /// the API cannot tell anime from any other video.
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Movies, SourceGroup::Tv]
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let query = query.trim();
        if query.len() < MIN_QUERY {
            return Ok(vec![]);
        }
        let env: Envelope = crate::util::net::client()
            .get(format!("{}/api/search/torrents", self.base_url))
            .query(&[
                ("q", query),
                ("type", "video"),
                ("limit", "50"),
                ("sortBy", "seeders"),
                ("sortOrder", "desc"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut out = Vec::new();
        for row in env.results {
            let infohash = row.torrent_infohash.unwrap_or_default().to_lowercase();
            if !is_infohash(&infohash) {
                continue;
            }
            let title = row
                .torrent_name
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| infohash.clone());
            let Some(magnet) = build_magnet(&infohash, &title, &[]) else {
                continue;
            };
            let Some(infohash) = infohash_from_magnet(&magnet) else {
                continue;
            };
            out.push(SearchResult {
                magnet,
                title,
                size_bytes: row.torrent_total_size.unwrap_or(0),
                seeders: row.torrent_seeders.unwrap_or(0),
                leechers: row.torrent_leechers.unwrap_or(0),
                source_id: "bittorrented",
                infohash,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_rows_and_drops_bad_infohashes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/search/torrents"))
            .and(query_param("q", "dune"))
            .and(query_param("type", "video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [
                    {"torrent_infohash": "ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                     "torrent_name": "Dune 2021", "torrent_total_size": 2147483648u64,
                     "torrent_seeders": 300, "torrent_leechers": 20},
                    {"torrent_infohash": "not-a-hash", "torrent_name": "Junk"},
                    {"torrent_name": "No hash at all"}
                ]
            })))
            .mount(&server)
            .await;
        let mut src = Bittorrented::new();
        src.base_url = server.uri();
        let out = src.search("dune").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Dune 2021");
        assert_eq!(out[0].seeders, 300);
        assert_eq!(out[0].size_bytes, 2_147_483_648);
        assert_eq!(out[0].source_id, "bittorrented");
    }

    #[tokio::test]
    async fn short_query_returns_empty_without_requesting() {
        let server = MockServer::start().await;
        let mut src = Bittorrented::new();
        src.base_url = server.uri();
        assert!(src.search("du").await.unwrap().is_empty());
        assert!(src.search("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn null_swarm_counts_become_zero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/search/torrents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [{"torrent_infohash": "ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                             "torrent_name": "X", "torrent_seeders": null, "torrent_leechers": null}]
            })))
            .mount(&server)
            .await;
        let mut src = Bittorrented::new();
        src.base_url = server.uri();
        let out = src.search("xyz").await.unwrap();
        assert_eq!(out[0].seeders, 0);
        assert_eq!(out[0].size_bytes, 0);
    }
}
