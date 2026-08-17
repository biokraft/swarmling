use super::{build_magnet, SearchResult, Source, SourceError, SourceGroup};
use crate::util::format::{parse_size, unescape_entities};

pub struct Nyaa {
    pub base_url: String,
}

impl Nyaa {
    pub fn new() -> Self {
        Self {
            base_url: "https://nyaa.si".into(),
        }
    }
}

impl Default for Nyaa {
    fn default() -> Self {
        Self::new()
    }
}

/// Pull one tag's text out of a single `<item>` block, unwrapping CDATA.
/// Hand-rolled rather than a full XML parse: the feed is flat and this keeps
/// a malformed row from failing the whole search.
fn tag(item: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let Some(start) = item.find(&open) else {
        return String::new();
    };
    let rest = &item[start + open.len()..];
    let Some(end) = rest.find(&close) else {
        return String::new();
    };
    rest[..end]
        .trim()
        .trim_start_matches("<![CDATA[")
        .trim_end_matches("]]>")
        .trim()
        .to_string()
}

#[async_trait::async_trait]
impl Source for Nyaa {
    fn id(&self) -> &'static str {
        "nyaa"
    }
    fn groups(&self) -> &'static [SourceGroup] {
        &[SourceGroup::Anime]
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let xml = crate::util::net::client()
            .get(format!("{}/", self.base_url))
            .query(&[
                ("page", "rss"),
                ("q", query.trim()),
                ("c", "0_0"),
                ("f", "0"),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let mut out = Vec::new();
        for item in xml.split("<item>").skip(1) {
            let infohash = tag(item, "nyaa:infoHash").to_lowercase();
            let title = unescape_entities(&tag(item, "title"));
            if infohash.is_empty() || title.is_empty() {
                continue;
            }
            out.push(SearchResult {
                magnet: build_magnet(&infohash, &title, &[]),
                title,
                size_bytes: parse_size(&tag(item, "nyaa:size")),
                seeders: tag(item, "nyaa:seeders").parse().unwrap_or(0),
                leechers: tag(item, "nyaa:leechers").parse().unwrap_or(0),
                source_id: "nyaa",
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

    const FEED: &str = r#"<rss><channel>
      <item>
        <title>[SubsGroup] Show - 01 [1080p]</title>
        <nyaa:infoHash>ABCDEF1234567890ABCDEF1234567890ABCDEF12</nyaa:infoHash>
        <nyaa:seeders>42</nyaa:seeders>
        <nyaa:leechers>7</nyaa:leechers>
        <nyaa:size>1.4 GiB</nyaa:size>
      </item>
      <item>
        <title>Broken row, no hash</title>
        <nyaa:seeders>1</nyaa:seeders>
      </item>
    </channel></rss>"#;

    #[tokio::test]
    async fn parses_nyaa_rss() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("page", "rss"))
            .and(query_param("q", "show"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FEED))
            .mount(&server)
            .await;
        let mut src = Nyaa::new();
        src.base_url = server.uri();
        let out = src.search("show").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "[SubsGroup] Show - 01 [1080p]");
        assert_eq!(out[0].seeders, 42);
        assert_eq!(out[0].leechers, 7);
        assert_eq!(out[0].size_bytes, 1_503_238_554);
        assert!(out[0]
            .magnet
            .starts_with("magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12"));
        assert_eq!(out[0].source_id, "nyaa");
    }

    #[tokio::test]
    async fn http_error_is_reported_not_panicked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let mut src = Nyaa::new();
        src.base_url = server.uri();
        assert!(src.search("show").await.is_err());
    }
}
