use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::engine::{EngineError, TorrentEngine, TorrentSnapshot};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub infohash: String,
    pub magnet: String,
    pub title: String,
    pub added_unix: u64,
    pub paused: bool,
    /// Which source the torrent was found through, when it was found by
    /// searching. `None` for a magnet the user pasted, which has no source.
    /// `serde(default)` is mandatory: a queue file written before this field
    /// existed must still load.
    #[serde(default)]
    pub source_id: Option<String>,
}

/// The list of downloads the user asked for, and the engine that carries them
/// out. The queue is the source of truth for intent; the engine is the source
/// of truth for progress.
pub struct DownloadQueue {
    engine: Arc<dyn TorrentEngine>,
    download_dir: PathBuf,
    entries: Mutex<Vec<QueueEntry>>,
    add_lock: AsyncMutex<()>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl DownloadQueue {
    pub fn new(engine: Arc<dyn TorrentEngine>, download_dir: PathBuf) -> Self {
        Self::with_entries(engine, download_dir, Vec::new())
    }

    pub fn with_entries(
        engine: Arc<dyn TorrentEngine>,
        download_dir: PathBuf,
        entries: Vec<QueueEntry>,
    ) -> Self {
        Self {
            engine,
            download_dir,
            entries: Mutex::new(entries),
            add_lock: AsyncMutex::new(()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<QueueEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn entries(&self) -> Vec<QueueEntry> {
        self.lock().clone()
    }

    pub fn download_dir(&self) -> &PathBuf {
        &self.download_dir
    }

    /// Adding a torrent already in the queue is a no-op that returns the
    /// existing infohash: a user pressing the key twice must not start a
    /// second copy.
    pub async fn add(
        &self,
        magnet: &str,
        title: &str,
        paused: bool,
    ) -> Result<String, EngineError> {
        let _guard = self.add_lock.lock().await;

        if let Some(existing) = self
            .lock()
            .iter()
            .find(|e| e.magnet == magnet)
            .map(|e| e.infohash.clone())
        {
            return Ok(existing);
        }

        let infohash = self
            .engine
            .add_magnet(magnet, &self.download_dir, paused)
            .await?;

        let mut entries = self.lock();
        if entries.iter().any(|e| e.infohash == infohash) {
            return Ok(infohash);
        }
        entries.push(QueueEntry {
            infohash: infohash.clone(),
            magnet: magnet.to_string(),
            title: title.to_string(),
            added_unix: now_unix(),
            paused,
            // This path takes a bare magnet and knows of no source. The TUI
            // supplies one through `Effect::AddToQueue`.
            source_id: None,
        });
        Ok(infohash)
    }

    /// Persisting is the caller's responsibility: call `save` afterwards.
    pub async fn pause(&self, infohash: &str) -> Result<(), EngineError> {
        self.engine.pause(infohash).await?;
        if let Some(e) = self.lock().iter_mut().find(|e| e.infohash == infohash) {
            e.paused = true;
        }
        Ok(())
    }

    /// Persisting is the caller's responsibility: call `save` afterwards.
    pub async fn resume(&self, infohash: &str) -> Result<(), EngineError> {
        self.engine.resume(infohash).await?;
        if let Some(e) = self.lock().iter_mut().find(|e| e.infohash == infohash) {
            e.paused = false;
        }
        Ok(())
    }

    /// The queue is the source of truth for what the user asked for, so an
    /// engine that has already forgotten this torrent must not strand the
    /// entry: drop it either way.
    pub async fn remove(&self, infohash: &str, delete_files: bool) -> Result<(), EngineError> {
        match self.engine.remove(infohash, delete_files).await {
            Ok(()) | Err(EngineError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        self.lock().retain(|e| e.infohash != infohash);
        Ok(())
    }

    pub async fn snapshots(&self) -> Vec<TorrentSnapshot> {
        self.engine.snapshots().await
    }

    /// Persist the current entries. Callers save after any mutation; the queue
    /// deliberately does not save itself, so tests never touch the filesystem.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        super::persist::save_entries(path, &self.entries())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fake::FakeEngine;
    use crate::engine::TorrentState;

    const MAGNET_A: &str = "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&dn=A";
    const MAGNET_B: &str = "magnet:?xt=urn:btih:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb&dn=B";

    fn queue() -> (Arc<FakeEngine>, DownloadQueue) {
        let engine = Arc::new(FakeEngine::new());
        let queue = DownloadQueue::new(engine.clone(), PathBuf::from("/downloads"));
        (engine, queue)
    }

    #[tokio::test]
    async fn add_registers_an_entry_and_starts_the_torrent() {
        let (engine, queue) = queue();
        let hash = queue.add(MAGNET_A, "Movie A", false).await.unwrap();
        assert_eq!(hash, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

        let entries = queue.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Movie A");
        assert_eq!(entries[0].magnet, MAGNET_A);
        assert!(!entries[0].paused);
        assert!(entries[0].added_unix > 0);
        assert_eq!(engine.added_magnets().len(), 1);
    }

    #[tokio::test]
    async fn adding_the_same_torrent_twice_is_idempotent() {
        let (engine, queue) = queue();
        let first = queue.add(MAGNET_A, "Movie A", false).await.unwrap();
        let second = queue.add(MAGNET_A, "Movie A again", false).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(queue.entries().len(), 1);
        assert_eq!(
            engine.added_magnets().len(),
            1,
            "must not re-add to the engine"
        );
    }

    #[tokio::test]
    async fn adding_paused_starts_the_torrent_pre_paused() {
        let (engine, queue) = queue();
        let hash = queue.add(MAGNET_A, "A", true).await.unwrap();

        assert!(
            queue.entries()[0].paused,
            "entry must record the paused intent"
        );
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Paused,
            "engine must have been told paused at add time, not corrected afterwards"
        );
    }

    #[tokio::test]
    async fn pause_and_resume_update_entry_and_engine() {
        let (engine, queue) = queue();
        let hash = queue.add(MAGNET_A, "A", false).await.unwrap();

        queue.pause(&hash).await.unwrap();
        assert!(queue.entries()[0].paused);
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Paused
        );

        queue.resume(&hash).await.unwrap();
        assert!(!queue.entries()[0].paused);
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Downloading
        );
    }

    #[tokio::test]
    async fn remove_drops_the_entry() {
        let (engine, queue) = queue();
        let hash = queue.add(MAGNET_A, "A", false).await.unwrap();
        queue.remove(&hash, false).await.unwrap();
        assert!(queue.entries().is_empty());
        assert!(engine.snapshots().await.is_empty());
    }

    #[tokio::test]
    async fn remove_succeeds_even_if_the_engine_forgot_the_torrent() {
        let (engine, queue) = queue();
        let hash = queue.add(MAGNET_A, "A", false).await.unwrap();
        engine.remove(&hash, false).await.unwrap(); // engine loses it behind the queue's back
        queue.remove(&hash, false).await.unwrap();
        assert!(queue.entries().is_empty());
    }

    #[tokio::test]
    async fn a_failed_add_leaves_no_entry_behind() {
        let (engine, queue) = queue();
        engine.fail_next_add("backend exploded");
        let err = queue.add(MAGNET_A, "A", false).await.unwrap_err();
        assert!(matches!(err, EngineError::Backend(_)));
        assert!(
            queue.entries().is_empty(),
            "a failed add must not leave a phantom entry"
        );
    }

    #[tokio::test]
    async fn with_entries_restores_without_touching_the_engine() {
        let engine = Arc::new(FakeEngine::new());
        let restored = vec![QueueEntry {
            infohash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            magnet: MAGNET_A.into(),
            title: "A".into(),
            added_unix: 1,
            paused: true,
            source_id: None,
        }];
        let queue = DownloadQueue::with_entries(engine.clone(), PathBuf::from("/d"), restored);
        assert_eq!(queue.entries().len(), 1);
        assert!(
            engine.added_magnets().is_empty(),
            "restoring is Task 4's job, not the constructor's"
        );
    }

    #[tokio::test]
    async fn snapshots_covers_every_live_torrent() {
        let (_engine, queue) = queue();
        queue.add(MAGNET_A, "A", false).await.unwrap();
        queue.add(MAGNET_B, "B", false).await.unwrap();
        assert_eq!(queue.snapshots().await.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_adds_of_same_magnet_are_serialized() {
        let engine = Arc::new(FakeEngine::new());
        let queue = Arc::new(DownloadQueue::new(
            engine.clone(),
            PathBuf::from("/downloads"),
        ));

        let q1 = queue.clone();
        let q2 = queue.clone();

        let (r1, r2) = tokio::join!(
            q1.add(MAGNET_A, "A first", false),
            q2.add(MAGNET_A, "A second", false)
        );

        let hash1 = r1.unwrap();
        let hash2 = r2.unwrap();

        assert_eq!(hash1, hash2, "both calls should return the same infohash");
        assert_eq!(
            engine.added_magnets().len(),
            1,
            "engine.add_magnet must be called exactly once"
        );
        assert_eq!(
            queue.entries().len(),
            1,
            "queue must have exactly one entry"
        );
    }
}
