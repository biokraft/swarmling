use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::{EngineError, InfoHash, TorrentEngine, TorrentSnapshot, TorrentState};
use crate::sources::magnet::parse_magnet;

#[derive(Default)]
struct Inner {
    torrents: HashMap<InfoHash, TorrentSnapshot>,
    added_magnets: Vec<String>,
    fail_next_add: Option<String>,
}

/// An in-memory stand-in for a real torrent backend. It never opens a socket
/// and never touches the filesystem, which is what makes the rest of the
/// download logic testable at all.
#[derive(Default)]
pub struct FakeEngine {
    inner: Mutex<Inner>,
}

impl FakeEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Make the next `add_magnet` fail with a backend error, once.
    pub fn fail_next_add(&self, message: &str) {
        self.lock().fail_next_add = Some(message.to_string());
    }

    pub fn set_progress(&self, infohash: &str, progress_bytes: u64, total_bytes: u64) {
        if let Some(t) = self.lock().torrents.get_mut(infohash) {
            t.progress_bytes = progress_bytes;
            t.total_bytes = total_bytes;
        }
    }

    pub fn finish(&self, infohash: &str) {
        if let Some(t) = self.lock().torrents.get_mut(infohash) {
            t.progress_bytes = t.total_bytes;
            t.state = TorrentState::Seeding;
        }
    }

    pub fn added_magnets(&self) -> Vec<String> {
        self.lock().added_magnets.clone()
    }
}

#[async_trait::async_trait]
impl TorrentEngine for FakeEngine {
    async fn add_magnet(
        &self,
        magnet: &str,
        _output_dir: &Path,
        paused: bool,
    ) -> Result<InfoHash, EngineError> {
        // Yield to allow other tasks to run; a real engine would await here.
        tokio::task::yield_now().await;

        let parsed =
            parse_magnet(magnet).ok_or_else(|| EngineError::InvalidMagnet(magnet.to_string()))?;
        let mut inner = self.lock();
        if let Some(message) = inner.fail_next_add.take() {
            return Err(EngineError::Backend(message));
        }
        inner.added_magnets.push(magnet.to_string());
        let name = if parsed.name.is_empty() {
            parsed.infohash.clone()
        } else {
            parsed.name.clone()
        };
        inner.torrents.insert(
            parsed.infohash.clone(),
            TorrentSnapshot {
                infohash: parsed.infohash.clone(),
                name,
                state: if paused {
                    TorrentState::Paused
                } else {
                    TorrentState::Downloading
                },
                progress_bytes: 0,
                total_bytes: 0,
                download_speed: 0,
                upload_speed: 0,
                error: None,
            },
        );
        Ok(parsed.infohash)
    }

    async fn pause(&self, infohash: &str) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let t = inner
            .torrents
            .get_mut(infohash)
            .ok_or_else(|| EngineError::NotFound(infohash.to_string()))?;
        t.state = TorrentState::Paused;
        Ok(())
    }

    async fn resume(&self, infohash: &str) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let t = inner
            .torrents
            .get_mut(infohash)
            .ok_or_else(|| EngineError::NotFound(infohash.to_string()))?;
        // A finished torrent resumes to seeding, not to downloading.
        t.state = if t.total_bytes > 0 && t.progress_bytes >= t.total_bytes {
            TorrentState::Seeding
        } else {
            TorrentState::Downloading
        };
        Ok(())
    }

    async fn remove(&self, infohash: &str, _delete_files: bool) -> Result<(), EngineError> {
        self.lock()
            .torrents
            .remove(infohash)
            .map(|_| ())
            .ok_or_else(|| EngineError::NotFound(infohash.to_string()))
    }

    async fn snapshot(&self, infohash: &str) -> Result<TorrentSnapshot, EngineError> {
        self.lock()
            .torrents
            .get(infohash)
            .cloned()
            .ok_or_else(|| EngineError::NotFound(infohash.to_string()))
    }

    async fn snapshots(&self) -> Vec<TorrentSnapshot> {
        let mut out: Vec<TorrentSnapshot> = self.lock().torrents.values().cloned().collect();
        out.sort_by(|a, b| a.infohash.cmp(&b.infohash)); // deterministic for tests
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const MAGNET: &str = "magnet:?xt=urn:btih:abcdef1234567890abcdef1234567890abcdef12&dn=Example";

    #[tokio::test]
    async fn add_magnet_records_and_returns_the_infohash() {
        let engine = FakeEngine::new();
        let hash = engine
            .add_magnet(MAGNET, Path::new("/downloads"), false)
            .await
            .unwrap();
        assert_eq!(hash, "abcdef1234567890abcdef1234567890abcdef12");
        assert_eq!(engine.added_magnets(), vec![MAGNET.to_string()]);
        let snap = engine.snapshot(&hash).await.unwrap();
        assert_eq!(snap.state, TorrentState::Downloading);
        assert_eq!(snap.name, "Example");
    }

    #[tokio::test]
    async fn add_magnet_paused_starts_paused() {
        let engine = FakeEngine::new();
        let hash = engine
            .add_magnet(MAGNET, Path::new("/downloads"), true)
            .await
            .unwrap();
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Paused
        );
    }

    #[tokio::test]
    async fn rejects_a_magnet_without_a_usable_infohash() {
        let engine = FakeEngine::new();
        let err = engine
            .add_magnet("not a magnet", Path::new("/d"), false)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidMagnet(_)));
    }

    #[tokio::test]
    async fn pause_resume_and_remove_move_state() {
        let engine = FakeEngine::new();
        let hash = engine
            .add_magnet(MAGNET, Path::new("/d"), false)
            .await
            .unwrap();

        engine.pause(&hash).await.unwrap();
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Paused
        );

        engine.resume(&hash).await.unwrap();
        assert_eq!(
            engine.snapshot(&hash).await.unwrap().state,
            TorrentState::Downloading
        );

        engine.remove(&hash, false).await.unwrap();
        assert!(matches!(
            engine.snapshot(&hash).await,
            Err(EngineError::NotFound(_))
        ));
        assert!(engine.snapshots().await.is_empty());
    }

    #[tokio::test]
    async fn operations_on_an_unknown_hash_are_not_found() {
        let engine = FakeEngine::new();
        assert!(matches!(
            engine.pause("deadbeef").await,
            Err(EngineError::NotFound(_))
        ));
        assert!(matches!(
            engine.resume("deadbeef").await,
            Err(EngineError::NotFound(_))
        ));
        assert!(matches!(
            engine.remove("deadbeef", false).await,
            Err(EngineError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn test_hooks_drive_progress_and_completion() {
        let engine = FakeEngine::new();
        let hash = engine
            .add_magnet(MAGNET, Path::new("/d"), false)
            .await
            .unwrap();

        engine.set_progress(&hash, 512, 1024);
        let snap = engine.snapshot(&hash).await.unwrap();
        assert_eq!((snap.progress_bytes, snap.total_bytes), (512, 1024));

        engine.finish(&hash);
        let snap = engine.snapshot(&hash).await.unwrap();
        assert_eq!(snap.state, TorrentState::Seeding);
        assert_eq!(snap.progress_bytes, snap.total_bytes);
    }

    #[tokio::test]
    async fn fail_next_add_surfaces_a_backend_error_once() {
        let engine = FakeEngine::new();
        engine.fail_next_add("disk on fire");
        let err = engine
            .add_magnet(MAGNET, Path::new("/d"), false)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Backend(m) if m == "disk on fire"));
        // the failure is one-shot: the next add succeeds
        assert!(engine
            .add_magnet(MAGNET, Path::new("/d"), false)
            .await
            .is_ok());
    }
}
