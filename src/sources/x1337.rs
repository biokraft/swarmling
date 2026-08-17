use std::sync::LazyLock;

use futures::future::join_all;
use regex::Regex;

use super::magnet::parse_magnet;
use super::{build_magnet, SearchResult, Source, SourceError, SourceGroup};
use crate::util::format::{parse_size, unescape_entities};

/// Mirrors are tried in order; the site rotates which one is reachable.
const HOSTS: [&str; 4] = ["1337x.to", "1337x.st", "x1337x.ws", "1337xx.to"];

/// The listing page has no magnets, so each result costs a second request.
/// Four keeps a search fast enough to feel instant.
const MAX_DETAILS: usize = 4;

const STOP_WORDS: [&str; 7] = ["the", "a", "an", "of", "and", "or", "to"];

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)href="(/torrent/[^"]+)"[^>]*>([^<]+)</a>"#).expect("link regex is valid")
});
static SEEDS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)class="coll-2 seeds[^"]*">\s*(\d+)"#).expect("seeds regex is valid")
});
static LEECH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)class="coll-3 leeches[^"]*">\s*(\d+)"#).expect("leeches regex is valid")
});
static SIZE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)class="coll-4 size[^"]*">\s*([\d.]+\s*[KMGT]i?B)"#)
        .expect("size regex is valid")
});
static MAGNET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)magnet:\?xt=urn:btih:[^"'<>\s]+"#).expect("magnet regex is valid")
});
static ROW_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<tr[\s>]").expect("row split regex is valid"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub path: String,
    pub seeders: u32,
    pub leechers: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Movies,
    Tv,
}

/// Rows come out of the listing table only; anything before `table-list`
/// (nav, ads, an interstitial) is ignored, and a page without it yields none.
pub fn parse_rows(html: &str) -> Vec<Row> {
    let Some(start) = html.find("table-list") else {
        return vec![];
    };
    let table = &html[start..];
    let mut out = Vec::new();
    for tr in ROW_SPLIT_RE.split(table).skip(1) {
        let Some(link) = LINK_RE.captures(tr) else {
            continue;
        };
        out.push(Row {
            name: unescape_entities(link[2].trim()),
            path: link[1].to_string(),
            seeders: SEEDS_RE
                .captures(tr)
                .and_then(|c| c[1].parse().ok())
                .unwrap_or(0),
            leechers: LEECH_RE
                .captures(tr)
                .and_then(|c| c[1].parse().ok())
                .unwrap_or(0),
            size_bytes: SIZE_RE.captures(tr).map(|c| parse_size(&c[1])).unwrap_or(0),
        });
    }
    out
}

pub struct X1337 {
    pub hosts: Vec<String>,
    pub category: Category,
    pub id: &'static str,
}

impl X1337 {
    pub fn movies() -> Self {
        Self {
            hosts: HOSTS.iter().map(|h| format!("https://{h}")).collect(),
            category: Category::Movies,
            id: "x1337-movies",
        }
    }
    pub fn tv() -> Self {
        Self {
            hosts: HOSTS.iter().map(|h| format!("https://{h}")).collect(),
            category: Category::Tv,
            id: "x1337-tv",
        }
    }

    fn listing_path(&self, query: &str) -> String {
        let cat = match self.category {
            Category::Movies => "Movies",
            Category::Tv => "TV",
        };
        if query.is_empty() {
            match self.category {
                Category::Movies => "/popular-movies".to_string(),
                Category::Tv => "/popular-tv".to_string(),
            }
        } else {
            format!("/category-search/{}/{cat}/1/", query.replace(' ', "+"))
        }
    }

    async fn fetch_text(url: String) -> Result<String, SourceError> {
        Ok(crate::util::net::client()
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    /// Walk the mirrors until one answers. Every mirror failing is an error:
    /// an empty result would read as "nothing matched", which is a lie.
    async fn fetch_listing(&self, path: &str) -> Result<(String, String), SourceError> {
        let mut last: Option<SourceError> = None;
        for host in &self.hosts {
            match Self::fetch_text(format!("{host}{path}")).await {
                Ok(html) => return Ok((host.clone(), html)),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| SourceError::BadResponse("1337x unreachable".into())))
    }
}

#[async_trait::async_trait]
impl Source for X1337 {
    fn id(&self) -> &'static str {
        self.id
    }
    fn groups(&self) -> &'static [SourceGroup] {
        match self.category {
            Category::Movies => &[SourceGroup::Movies],
            Category::Tv => &[SourceGroup::Tv],
        }
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SourceError> {
        let query = query.trim();
        let (host, html) = self.fetch_listing(&self.listing_path(query)).await?;

        let tokens: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        let meaningful: Vec<String> = tokens
            .iter()
            .filter(|t| !STOP_WORDS.contains(&t.as_str()))
            .cloned()
            .collect();
        let needles = if meaningful.is_empty() {
            tokens
        } else {
            meaningful
        };

        let mut rows = parse_rows(&html);
        if !needles.is_empty() {
            rows.retain(|r| {
                let name = r.name.to_lowercase();
                needles.iter().all(|t| name.contains(t.as_str()))
            });
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.seeders));
        rows.truncate(MAX_DETAILS);

        // The detail fetches are independent, so they run together; a row whose
        // page is missing or magnet-less drops out instead of failing the search.
        let id = self.id;
        let details = rows.into_iter().map(|row| {
            let url = format!("{host}{}", row.path);
            async move {
                let html = Self::fetch_text(url).await.ok()?;
                let raw = unescape_entities(MAGNET_RE.find(&html)?.as_str());
                let parsed = parse_magnet(&raw)?;
                Some(SearchResult {
                    magnet: build_magnet(&parsed.infohash, &row.name, &parsed.trackers),
                    title: row.name,
                    size_bytes: row.size_bytes,
                    seeders: row.seeders,
                    leechers: row.leechers,
                    source_id: id,
                })
            }
        });
        Ok(join_all(details).await.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LISTING: &str = r#"<table class="table-list">
      <tr><td class="coll-1 name"><a href="/torrent/111/Dune-2021/">Dune 2021 1080p</a></td>
          <td class="coll-2 seeds">300</td><td class="coll-3 leeches">20</td>
          <td class="coll-4 size">2.0 GB</td></tr>
      <tr><td class="coll-1 name"><a href="/torrent/222/Other-Film/">Other Film</a></td>
          <td class="coll-2 seeds">5</td><td class="coll-3 leeches">1</td>
          <td class="coll-4 size">700 MB</td></tr>
    </table>"#;

    const DETAIL: &str = r#"<div><a href="magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&amp;dn=Dune">Magnet</a></div>"#;

    #[test]
    fn parse_rows_reads_the_listing_table() {
        let rows = parse_rows(LISTING);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Dune 2021 1080p");
        assert_eq!(rows[0].path, "/torrent/111/Dune-2021/");
        assert_eq!(rows[0].seeders, 300);
        assert_eq!(rows[0].leechers, 20);
        assert_eq!(rows[0].size_bytes, 2_000_000_000);
    }

    #[test]
    fn parse_rows_returns_nothing_without_the_table() {
        assert!(parse_rows("<html><body>blocked</body></html>").is_empty());
    }

    #[tokio::test]
    async fn search_filters_by_token_and_fetches_detail_magnets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/category-search/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LISTING))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/torrent/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(DETAIL))
            .mount(&server)
            .await;

        let mut src = X1337::movies();
        src.hosts = vec![server.uri()];
        let out = src.search("dune").await.unwrap();
        assert_eq!(out.len(), 1); // "Other Film" does not contain the token
        assert_eq!(out[0].title, "Dune 2021 1080p");
        assert_eq!(out[0].seeders, 300);
        assert!(out[0]
            .magnet
            .starts_with("magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12"));
        assert_eq!(out[0].source_id, "x1337-movies");
    }

    #[tokio::test]
    async fn falls_over_to_the_next_host() {
        let dead = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&dead)
            .await;
        let live = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/category-search/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LISTING))
            .mount(&live)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/torrent/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(DETAIL))
            .mount(&live)
            .await;

        let mut src = X1337::movies();
        src.hosts = vec![dead.uri(), live.uri()];
        assert_eq!(src.search("dune").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn every_host_down_is_an_error() {
        let dead = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&dead)
            .await;
        let mut src = X1337::movies();
        src.hosts = vec![dead.uri()];
        assert!(src.search("dune").await.is_err());
    }
}
