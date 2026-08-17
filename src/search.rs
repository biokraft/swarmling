use std::time::Duration;

use crate::sources::{SearchResult, Source};
use tokio::sync::mpsc;

/// Overall budget for a single source's search, covering every request it
/// makes internally (mirrors, detail fetches, retries). Without this, a
/// source that walks several hosts sequentially (1337x) can run far longer
/// than any single HTTP request's own timeout, keeping the whole fan-out
/// open until it gives up.
const SOURCE_TIMEOUT: Duration = Duration::from_secs(30);

pub enum SearchEvent {
    Results {
        source_id: &'static str,
        results: Vec<SearchResult>,
    },
    SourceFailed {
        source_id: &'static str,
        error: String,
    },
}

pub async fn search_all(sources: Vec<Box<dyn Source>>, query: &str) -> mpsc::Receiver<SearchEvent> {
    search_all_with_timeout(sources, query, SOURCE_TIMEOUT).await
}

async fn search_all_with_timeout(
    sources: Vec<Box<dyn Source>>,
    query: &str,
    timeout: Duration,
) -> mpsc::Receiver<SearchEvent> {
    let (tx, rx) = mpsc::channel(16);
    for source in sources {
        let tx = tx.clone();
        let query = query.to_owned();
        tokio::spawn(async move {
            let id = source.id();
            let event = match tokio::time::timeout(timeout, source.search(&query)).await {
                Ok(Ok(results)) => SearchEvent::Results {
                    source_id: id,
                    results,
                },
                Ok(Err(e)) => SearchEvent::SourceFailed {
                    source_id: id,
                    error: e.to_string(),
                },
                Err(_) => SearchEvent::SourceFailed {
                    source_id: id,
                    error: format!("{id} timed out after {timeout:?}"),
                },
            };
            let _ = tx.send(event).await;
        });
    }
    rx // all tx clones drop as tasks finish; recv() then yields None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::apibay::Apibay;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn streams_results_and_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q.php"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id":"1","name":"x","info_hash":"ABCDEF1234567890ABCDEF1234567890ABCDEF12",
                 "seeders":"1","leechers":"0","size":"100"}
            ])))
            .mount(&server)
            .await;

        let mut good = Apibay::movies();
        good.base_url = server.uri();
        let mut dead = Apibay::tv();
        dead.base_url = "http://127.0.0.1:1".into(); // nothing listens here

        let mut rx = search_all(vec![Box::new(good), Box::new(dead)], "x").await;
        let mut got_results = false;
        let mut got_failure = false;
        while let Some(ev) = rx.recv().await {
            match ev {
                SearchEvent::Results { source_id, results } => {
                    assert_eq!(source_id, "tpb-movies");
                    assert_eq!(results.len(), 1);
                    got_results = true;
                }
                SearchEvent::SourceFailed { source_id, .. } => {
                    assert_eq!(source_id, "tpb-tv");
                    got_failure = true;
                }
            }
        }
        assert!(got_results && got_failure);
    }

    struct NeverReturns;

    #[async_trait::async_trait]
    impl Source for NeverReturns {
        fn id(&self) -> &'static str {
            "never-returns"
        }
        fn groups(&self) -> &'static [crate::sources::SourceGroup] {
            &[]
        }
        async fn search(
            &self,
            _query: &str,
        ) -> Result<Vec<SearchResult>, crate::sources::SourceError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_source_that_never_returns_is_reported_failed_not_hung() {
        let mut rx =
            search_all_with_timeout(vec![Box::new(NeverReturns)], "x", Duration::from_millis(20))
                .await;
        let ev = rx.recv().await.expect("receiver must not hang forever");
        match ev {
            SearchEvent::SourceFailed { source_id, error } => {
                assert_eq!(source_id, "never-returns");
                assert!(error.contains("timed out"));
            }
            SearchEvent::Results { .. } => panic!("a source that never returns cannot succeed"),
        }
        assert!(rx.recv().await.is_none());
    }
}
