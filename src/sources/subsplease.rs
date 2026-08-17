use std::collections::HashMap;

use serde::Deserialize;

use super::magnet::parse_magnet;
use super::{build_magnet, SearchResult, Source, SourceError, SourceGroup};

const RES_PREFERENCE: [&str; 3] = ["1080", "720", "480"];

#[derive(Deserialize)]
struct Download {
    #[serde(default)]
    res: Option<String>,
    #[serde(default)]
    magnet: Option<String>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    show: Option<String>,
    #[serde(default)]
    episode: Option<String>,
    #[serde(default)]
    downloads: Vec<Download>,
}

pub struct Subsplease {
    pub base_url: String,
}

impl Subsplease {
    pub fn new() -> Self {
        Self {
            base_url: "https://subsplease.org".into(),
        }
    }
}

impl Default for Subsplease {
    fn default() -> Self {
        Self::new()
    }
}

fn pick_best(downloads: &[Download]) -> Option<&Download> {
    for want in RES_PREFERENCE {
        if let Some(d) = downloads
            .iter()
            .find(|d| d.res.as_deref() == Some(want) && d.magnet.is_some())
        {
            return Some(d);
        }
    }
    downloads.iter().find(|d| d.magnet.is_some())
}

/// `xl` is the exact length in bytes; the API exposes it only on some rows.
fn exact_length(magnet: &str) -> u64 {
    magnet
        .split(['?', '&'])
        .filter_map(|p| p.strip_prefix("xl="))
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Source for Subsplease {
    fn id(&self) -> &'static str {
        "subsplease"
    }
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Anime]
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let query = query.trim();
        let request = crate::util::net::client().get(format!("{}/api/", self.base_url));
        let request = if query.is_empty() {
            request.query(&[("tz", "UTC"), ("f", "latest")])
        } else {
            request.query(&[("tz", "UTC"), ("f", "search"), ("s", query)])
        };
        let body: serde_json::Value = request.send().await?.error_for_status()?.json().await?;

        // A JSON array (rather than the usual keyed object) is how the API says
        // "nothing found"; deserializing it as a map would be an error, not an
        // empty result.
        let Ok(entries) = serde_json::from_value::<HashMap<String, Entry>>(body) else {
            return Ok(vec![]);
        };

        let mut out = Vec::new();
        for entry in entries.values() {
            let Some(download) = pick_best(&entry.downloads) else {
                continue;
            };
            let Some(raw) = download.magnet.as_deref() else {
                continue;
            };
            let Some(parsed) = parse_magnet(raw) else {
                continue;
            };
            let show = entry.show.clone().unwrap_or_else(|| "Unknown".to_string());
            let episode = entry
                .episode
                .as_deref()
                .map(|e| format!(" - {e}"))
                .unwrap_or_default();
            let res = download.res.as_deref().unwrap_or("?");
            let title = format!("{show}{episode} [{res}p]");
            out.push(SearchResult {
                magnet: build_magnet(&parsed.infohash, &title, &parsed.trackers),
                title,
                size_bytes: exact_length(raw),
                seeders: 0,
                leechers: 0,
                source_id: "subsplease",
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
    async fn picks_the_highest_preferred_resolution() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(query_param("f", "search"))
            .and(query_param("s", "show"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "abc123": {
                    "show": "Some Show",
                    "episode": "01",
                    "downloads": [
                        {"res": "720", "magnet": "magnet:?xt=urn:btih:1111111111111111111111111111111111111111"},
                        {"res": "1080", "magnet": "magnet:?xt=urn:btih:2222222222222222222222222222222222222222&xl=1500000000"}
                    ]
                }
            })))
            .mount(&server)
            .await;
        let mut src = Subsplease::new();
        src.base_url = server.uri();
        let out = src.search("show").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Some Show - 01 [1080p]");
        assert!(out[0]
            .magnet
            .contains("btih:2222222222222222222222222222222222222222"));
        assert_eq!(out[0].size_bytes, 1_500_000_000);
        assert_eq!(out[0].seeders, 0);
        assert_eq!(out[0].source_id, "subsplease");
    }

    #[tokio::test]
    async fn array_response_means_no_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        let mut src = Subsplease::new();
        src.base_url = server.uri();
        assert!(src.search("nothing").await.unwrap().is_empty());
    }
}
