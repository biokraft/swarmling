use std::path::PathBuf;
use std::sync::Arc;

use super::queue::{DownloadQueue, QueueEntry};
use crate::engine::TorrentEngine;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: usize,
    pub failed: Vec<(String, String)>,
}

/// Hand every persisted entry back to the engine, preserving whether it was
/// paused. An entry the engine rejects is reported and dropped: one unreadable
/// torrent must never stop the rest of the queue from coming back.
pub async fn restore(
    engine: Arc<dyn TorrentEngine>,
    download_dir: PathBuf,
    entries: Vec<QueueEntry>,
) -> (DownloadQueue, RestoreReport) {
    let mut report = RestoreReport::default();
    let mut kept = Vec::with_capacity(entries.len());

    for entry in entries {
        match engine
            .add_magnet(&entry.magnet, &download_dir, entry.paused)
            .await
        {
            Ok(returned_infohash) => {
                report.restored += 1;
                let mut entry = entry;
                // The engine's returned hash is authoritative: if it ever
                // differs from what was persisted, keep the entry matchable
                // against future engine calls rather than stranding it under
                // a stale hash.
                entry.infohash = returned_infohash;
                kept.push(entry);
            }
            Err(e) => report.failed.push((entry.infohash.clone(), e.to_string())),
        }
    }

    (
        DownloadQueue::with_entries(engine, download_dir, kept),
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fake::FakeEngine;
    use crate::engine::TorrentState;

    fn entry(hash: &str, paused: bool) -> QueueEntry {
        QueueEntry {
            infohash: hash.into(),
            magnet: format!("magnet:?xt=urn:btih:{hash}&dn=Example"),
            title: "Example".into(),
            added_unix: 1,
            paused,
        }
    }

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[tokio::test]
    async fn restores_entries_into_the_engine() {
        let engine = Arc::new(FakeEngine::new());
        let (queue, report) = restore(
            engine.clone(),
            PathBuf::from("/d"),
            vec![entry(HASH_A, false), entry(HASH_B, false)],
        )
        .await;

        assert_eq!(report.restored, 2);
        assert!(report.failed.is_empty());
        assert_eq!(queue.entries().len(), 2);
        assert_eq!(engine.snapshots().await.len(), 2);
    }

    #[tokio::test]
    async fn a_paused_entry_comes_back_paused() {
        let engine = Arc::new(FakeEngine::new());
        let (queue, _) = restore(
            engine.clone(),
            PathBuf::from("/d"),
            vec![entry(HASH_A, true)],
        )
        .await;

        assert!(queue.entries()[0].paused);
        assert_eq!(
            engine.snapshot(HASH_A).await.unwrap().state,
            TorrentState::Paused
        );
    }

    #[tokio::test]
    async fn one_bad_entry_does_not_stop_the_others() {
        let engine = Arc::new(FakeEngine::new());
        engine.fail_next_add("corrupt resume data");
        let (queue, report) = restore(
            engine.clone(),
            PathBuf::from("/d"),
            vec![entry(HASH_A, false), entry(HASH_B, false)],
        )
        .await;

        assert_eq!(report.restored, 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, HASH_A);
        assert_eq!(
            queue.entries().len(),
            1,
            "the failed entry is dropped from the queue"
        );
        assert_eq!(queue.entries()[0].infohash, HASH_B);
    }

    #[tokio::test]
    async fn restore_adopts_the_engines_returned_infohash() {
        let engine = Arc::new(FakeEngine::new());
        // Persisted entry has a stale infohash that doesn't match the magnet
        // it was built from; the engine derives the real one from the magnet.
        let mut stale = entry(HASH_A, false);
        stale.infohash = "cccccccccccccccccccccccccccccccccccccccc".into();

        let (queue, report) = restore(engine.clone(), PathBuf::from("/d"), vec![stale]).await;

        assert_eq!(report.restored, 1);
        assert_eq!(
            queue.entries()[0].infohash,
            HASH_A,
            "kept entry must adopt the engine's returned infohash"
        );
    }

    #[tokio::test]
    async fn restoring_nothing_yields_an_empty_queue() {
        let engine = Arc::new(FakeEngine::new());
        let (queue, report) = restore(engine, PathBuf::from("/d"), vec![]).await;
        assert_eq!(report.restored, 0);
        assert!(queue.entries().is_empty());
    }

    #[tokio::test]
    async fn a_restored_queue_saves_and_reloads_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let engine = Arc::new(FakeEngine::new());
        let (queue, _) = restore(engine, PathBuf::from("/d"), vec![entry(HASH_A, true)]).await;

        queue.save(&path).unwrap();
        assert_eq!(
            crate::download::persist::load_entries(&path),
            queue.entries()
        );
    }
}
