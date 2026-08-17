use serde::Deserialize;

use super::{magnet_from_infohash, SearchResult, Source, SourceError, SourceGroup};

#[derive(Deserialize)]
struct Row {
    id: String,
    name: String,
    info_hash: String,
    seeders: String,
    leechers: String,
    size: String,
}

pub struct Apibay {
    pub base_url: String,
    pub id: &'static str,
    pub groups: &'static [SourceGroup],
}

impl Apibay {
    pub fn movies() -> Self {
        Self {
            base_url: "https://apibay.org".into(),
            id: "tpb-movies",
            groups: &[SourceGroup::Movies],
        }
    }
    pub fn tv() -> Self {
        Self {
            base_url: "https://apibay.org".into(),
            id: "tpb-tv",
            groups: &[SourceGroup::Tv],
        }
    }

    async fn fetch(&self, query: &str) -> Result<Vec<Row>, SourceError> {
        let url = format!("{}/q.php", self.base_url);
        let rows: Vec<Row> = crate::util::net::client()
            .get(url)
            .query(&[("q", query)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(rows)
    }
}

fn is_sentinel(rows: &[Row]) -> bool {
    rows.len() == 1 && rows[0].id == "0"
}

#[async_trait::async_trait]
impl Source for Apibay {
    fn id(&self) -> &'static str {
        self.id
    }
    fn groups(&self) -> &'static [SourceGroup] {
        self.groups
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let mut rows = self.fetch(query).await?;
        // apibay sometimes answers its no-results sentinel spuriously; retry once.
        if is_sentinel(&rows) {
            rows = self.fetch(query).await?;
        }
        if is_sentinel(&rows) {
            return Ok(vec![]);
        }
        Ok(rows
            .into_iter()
            .map(|r| SearchResult {
                magnet: magnet_from_infohash(&r.info_hash, &r.name),
                title: r.name,
                size_bytes: r.size.parse().unwrap_or(0),
                seeders: r.seeders.parse().unwrap_or(0),
                leechers: r.leechers.parse().unwrap_or(0),
                source_id: self.id,
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
    async fn parses_apibay_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q.php"))
            .and(query_param("q", "ubuntu"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id":"1","name":"ubuntu 24.04 iso","info_hash":"ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                 "seeders":"120","leechers":"4","size":"6114295808"}
            ])))
            .mount(&server)
            .await;
        let mut src = Apibay::movies();
        src.base_url = server.uri();
        let out = src.search("ubuntu").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "ubuntu 24.04 iso");
        assert_eq!(out[0].seeders, 120);
        assert_eq!(out[0].size_bytes, 6_114_295_808);
        assert!(out[0].magnet.starts_with("magnet:?xt=urn:btih:abcdef"));
    }

    #[tokio::test]
    async fn no_results_sentinel_yields_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q.php"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id":"0","name":"No results returned","info_hash":"0000000000000000000000000000000000000000",
                 "seeders":"0","leechers":"0","size":"0"}
            ])))
            .mount(&server)
            .await;
        let mut src = Apibay::movies();
        src.base_url = server.uri();
        let out = src.search("zzzz").await.unwrap();
        assert!(out.is_empty());
    }
}
