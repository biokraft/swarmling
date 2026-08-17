use serde::Deserialize;

use super::{magnet_from_infohash, SearchResult, Source, SourceError, SourceGroup};

#[derive(Deserialize)]
struct Torrent {
    hash: String,
    quality: String,
    size_bytes: u64,
    seeds: u32,
    peers: u32,
}

#[derive(Deserialize)]
struct Movie {
    title_long: String,
    #[serde(default)]
    torrents: Vec<Torrent>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(default)]
    movies: Vec<Movie>,
}

#[derive(Deserialize)]
struct Envelope {
    data: Data,
}

pub struct Yts {
    pub base_url: String,
}

impl Yts {
    pub fn new() -> Self {
        Self {
            base_url: "https://yts.mx".into(),
        }
    }
}

impl Default for Yts {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Source for Yts {
    fn id(&self) -> &'static str {
        "yts"
    }
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Movies]
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let url = format!("{}/api/v2/list_movies.json", self.base_url);
        let env: Envelope = crate::util::net::client()
            .get(url)
            .query(&[("query_term", query)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(env
            .data
            .movies
            .into_iter()
            .flat_map(|m| {
                let title = m.title_long;
                m.torrents
                    .into_iter()
                    .map(move |t| SearchResult {
                        title: format!("{title} [{}]", t.quality),
                        magnet: magnet_from_infohash(&t.hash, &title),
                        size_bytes: t.size_bytes,
                        seeders: t.seeds,
                        leechers: t.peers,
                        source_id: "yts",
                    })
                    .collect::<Vec<_>>()
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_yts_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/list_movies.json"))
            .and(query_param("query_term", "dune"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {"movies": [{
                    "title_long": "Dune (2021)",
                    "torrents": [
                        {"hash": "ABCDEF1234567890ABCDEF1234567890ABCDEF12", "quality": "1080p",
                         "size_bytes": 2147483648u64, "seeds": 300, "peers": 20}
                    ]
                }]}
            })))
            .mount(&server)
            .await;
        let mut src = Yts::new();
        src.base_url = server.uri();
        let out = src.search("dune").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Dune (2021) [1080p]");
        assert_eq!(out[0].seeders, 300);
    }

    #[tokio::test]
    async fn missing_movies_field_yields_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/list_movies.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": {}})))
            .mount(&server)
            .await;
        let mut src = Yts::new();
        src.base_url = server.uri();
        assert!(src.search("zzzz").await.unwrap().is_empty());
    }
}
