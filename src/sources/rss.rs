use std::sync::LazyLock;

use regex::Regex;

use super::magnet::parse_magnet;
use super::{build_magnet, SearchResult, SourceError};
use crate::util::format::{tag, unescape_entities};

static MAGNET_HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)href="(magnet:\?xt=urn:btih:[^"]+)""#).expect("magnet href regex is valid")
});

/// WordPress-style feeds carry a magnet link and a title and nothing else:
/// no swarm counts, no sizes. Rows without a magnet are dropped.
pub fn parse_rss_items(xml: &str, source_id: &'static str) -> Vec<SearchResult> {
    let mut out = Vec::new();
    for item in xml.split("<item>").skip(1) {
        let Some(href) = MAGNET_HREF_RE.captures(item) else {
            continue;
        };
        let raw = unescape_entities(&href[1]);
        let Some(parsed) = parse_magnet(&raw) else {
            continue;
        };
        let raw_title = tag(item, "title");
        let title = if raw_title.is_empty() {
            "Unknown Title".to_string()
        } else {
            unescape_entities(&raw_title)
        };
        let Some(magnet) = build_magnet(&parsed.infohash, &title, &parsed.trackers) else {
            continue;
        };
        out.push(SearchResult {
            magnet,
            title,
            size_bytes: 0,
            seeders: 0,
            leechers: 0,
            source_id,
        });
    }
    out
}

pub async fn fetch_wordpress_rss(
    base_url: &str,
    source_id: &'static str,
    query: &str,
) -> Result<Vec<SearchResult>, SourceError> {
    let query = query.trim();
    let request = if query.is_empty() {
        crate::util::net::client().get(format!("{base_url}/feed/"))
    } else {
        crate::util::net::client()
            .get(format!("{base_url}/"))
            .query(&[("s", query), ("feed", "rss2")])
    };
    let xml = request.send().await?.error_for_status()?.text().await?;
    Ok(parse_rss_items(&xml, source_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<rss><channel>
      <item>
        <title>Some Game &#8211; Repack</title>
        <description>&lt;a href="magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&amp;dn=Some+Game"&gt;magnet&lt;/a&gt;</description>
      </item>
      <item>
        <title>No Magnet Here</title>
        <description>nothing linkable</description>
      </item>
    </channel></rss>"#;

    #[test]
    fn parses_items_with_magnets_and_skips_the_rest() {
        let out = parse_rss_items(FEED, "fitgirl");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Some Game - Repack");
        assert!(out[0]
            .magnet
            .starts_with("magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12"));
        assert_eq!(out[0].seeders, 0);
        assert_eq!(out[0].size_bytes, 0);
        assert_eq!(out[0].source_id, "fitgirl");
    }

    #[test]
    fn empty_feed_yields_no_results() {
        assert!(parse_rss_items("<rss><channel></channel></rss>", "fitgirl").is_empty());
    }

    #[test]
    fn cdata_wrapped_title_loses_the_wrapper() {
        let feed = r#"<rss><channel>
          <item>
            <title><![CDATA[Some Game - Repack]]></title>
            <description>&lt;a href="magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12"&gt;magnet&lt;/a&gt;</description>
          </item>
        </channel></rss>"#;
        let out = parse_rss_items(feed, "fitgirl");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Some Game - Repack");
        assert!(!out[0].title.contains("CDATA"));
    }
}
