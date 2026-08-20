use super::rss::fetch_wordpress_rss;
use super::{SearchResult, Source, SourceError, SourceGroup};

/// Games are the one category that can run code, so they come from FitGirl
/// alone. The WordPress feed carries no swarm data: every result reports zero
/// seeders, and the UI must not read that as a dead torrent.
pub struct Fitgirl {
    pub base_url: String,
}

impl Fitgirl {
    pub fn new() -> Self {
        Self {
            base_url: "https://fitgirl-repacks.site".into(),
        }
    }
}

impl Default for Fitgirl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Source for Fitgirl {
    fn id(&self) -> &'static str {
        "fitgirl"
    }
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Games]
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        fetch_wordpress_rss(&self.base_url, "fitgirl", query).await
    }

    /// This source's feed carries no swarm data, so `seeders` is hardcoded to
    /// 0 for every row. That is "unknown", not "dead" — the hide-dead filter
    /// must leave these rows alone. A source that parses a real seeder count
    /// from its response, even one that is sometimes null, keeps the default
    /// `true`.
    fn reports_health(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FEED: &str = r#"<rss><channel><item>
        <title>Some Game</title>
        <description>&lt;a href="magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12"&gt;m&lt;/a&gt;</description>
      </item></channel></rss>"#;

    #[tokio::test]
    async fn search_uses_the_search_feed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("s", "doom"))
            .and(query_param("feed", "rss2"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FEED))
            .mount(&server)
            .await;
        let mut src = Fitgirl::new();
        src.base_url = server.uri();
        let out = src.search("doom").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_id, "fitgirl");
    }

    #[tokio::test]
    async fn empty_query_browses_the_plain_feed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/feed/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FEED))
            .mount(&server)
            .await;
        let mut src = Fitgirl::new();
        src.base_url = server.uri();
        assert_eq!(src.search("").await.unwrap().len(), 1);
    }
}
