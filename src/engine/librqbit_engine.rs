use std::path::{Path, PathBuf};
use std::sync::Arc;

use librqbit::api::TorrentIdOrHash;
use librqbit::{
    AddTorrent, AddTorrentOptions, ManagedTorrent, Session, SessionOptions,
    SessionPersistenceConfig,
};

use super::{EngineError, InfoHash, TorrentEngine, TorrentSnapshot, TorrentState};
use crate::sources::magnet::is_infohash;

fn backend<E: std::fmt::Display>(e: E) -> EngineError {
    EngineError::Backend(e.to_string())
}

/// Only a well-formed 40-char hex infohash is accepted here. Without this
/// guard, `TorrentIdOrHash::parse` happily interprets a bare integer like
/// "0" as a session INDEX rather than a hash, so a typo such as `rm 0` could
/// resolve to an unrelated torrent that happens to sit at index 0.
fn id_or_hash(infohash: &str) -> Result<TorrentIdOrHash, EngineError> {
    if !is_infohash(infohash) {
        return Err(EngineError::NotFound(infohash.to_string()));
    }
    TorrentIdOrHash::try_from(infohash).map_err(|_| EngineError::NotFound(infohash.to_string()))
}

/// The only part of swarmling that knows librqbit exists. Everything above it
/// speaks the `TorrentEngine` vocabulary, which is what keeps the download
/// logic testable without a real peer.
pub struct LibrqbitEngine {
    session: Arc<Session>,
}

impl LibrqbitEngine {
    /// `download_dir` is where torrent data lands (librqbit's
    /// `default_output_folder`); `session_dir` is where librqbit persists its
    /// own session state (resume data, torrent list) as JSON so a restart can
    /// pick torrents back up. Without `persistence` set, `fastresume: true` is
    /// a no-op: there is no session state file to resume from.
    pub async fn new(download_dir: PathBuf, session_dir: PathBuf) -> Result<Self, EngineError> {
        let opts = SessionOptions {
            fastresume: true,
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(session_dir),
            }),
            client_name_and_version: Some(format!("swarmling {}", env!("CARGO_PKG_VERSION"))),
            ..Default::default()
        };
        let session = Session::new_with_opts(download_dir, opts)
            .await
            .map_err(backend)?;
        Ok(Self { session })
    }

    fn handle(&self, infohash: &str) -> Result<Arc<ManagedTorrent>, EngineError> {
        let id = id_or_hash(infohash)?;
        self.session
            .get(id)
            .ok_or_else(|| EngineError::NotFound(infohash.to_string()))
    }
}

fn to_snapshot(handle: &Arc<ManagedTorrent>) -> TorrentSnapshot {
    let stats = handle.stats();
    // Id20::as_string() is the documented hex encoder (hex::encode of the raw
    // bytes) and is what yields plain, lowercase, 40-character hex here.
    let infohash = handle.info_hash().as_string();
    let state = if stats.error.is_some() {
        TorrentState::Errored
    } else if handle.is_paused() {
        TorrentState::Paused
    } else if matches!(
        stats.state,
        librqbit::TorrentStatsState::Initializing { .. }
    ) {
        // While librqbit reports Initializing it is still verifying on-disk
        // data, so labelling it "Seeding" would claim more than we actually
        // know. "Checking" is the honest label until verification completes,
        // which is also why this check must come before the `finished` one.
        TorrentState::Checking
    } else if stats.finished {
        TorrentState::Seeding
    } else {
        TorrentState::Downloading
    };
    // LiveStats::download_speed/upload_speed are `Speed` (MiB/s internally —
    // librqbit's own Display renders it as "{:.2} MiB/s") with an
    // `as_bytes()` helper that converts to bytes/sec directly, so we don't
    // need to fall back to a 0 placeholder here.
    let (download_speed, upload_speed) = stats
        .live
        .as_ref()
        .map(|live| (live.download_speed.as_bytes(), live.upload_speed.as_bytes()))
        .unwrap_or((0, 0));
    TorrentSnapshot {
        name: handle.name().unwrap_or_else(|| infohash.clone()),
        infohash,
        state,
        progress_bytes: stats.progress_bytes,
        total_bytes: stats.total_bytes,
        download_speed,
        upload_speed,
        error: stats.error.clone(),
    }
}

#[async_trait::async_trait]
impl TorrentEngine for LibrqbitEngine {
    async fn add_magnet(
        &self,
        magnet: &str,
        output_dir: &Path,
        paused: bool,
    ) -> Result<InfoHash, EngineError> {
        let opts = AddTorrentOptions {
            paused,
            output_folder: Some(output_dir.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let response = self
            .session
            .add_torrent(AddTorrent::from_url(magnet), Some(opts))
            .await
            .map_err(backend)?;
        let handle = response
            .into_handle()
            .ok_or_else(|| EngineError::InvalidMagnet(magnet.to_string()))?;
        Ok(handle.info_hash().as_string())
    }

    async fn pause(&self, infohash: &str) -> Result<(), EngineError> {
        let handle = self.handle(infohash)?;
        self.session.pause(&handle).await.map_err(backend)
    }

    async fn resume(&self, infohash: &str) -> Result<(), EngineError> {
        let handle = self.handle(infohash)?;
        self.session.unpause(&handle).await.map_err(backend)
    }

    async fn remove(&self, infohash: &str, delete_files: bool) -> Result<(), EngineError> {
        // Look the handle up first so an infohash the session has already
        // forgotten (or an invalid one) surfaces as `NotFound` rather than a
        // generic `Backend` error from `session.delete` — `DownloadQueue::remove`
        // only forgives `NotFound`, so mapping every delete failure to
        // `Backend` would turn a forgotten torrent into a permanent orphan.
        let handle = self.handle(infohash)?;
        let id = TorrentIdOrHash::Hash(handle.info_hash());
        self.session.delete(id, delete_files).await.map_err(backend)
    }

    async fn snapshot(&self, infohash: &str) -> Result<TorrentSnapshot, EngineError> {
        Ok(to_snapshot(&self.handle(infohash)?))
    }

    async fn snapshots(&self) -> Vec<TorrentSnapshot> {
        self.session
            .with_torrents(|torrents| torrents.map(|(_, h)| to_snapshot(h)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_or_hash_accepts_a_well_formed_infohash() {
        let hash = "abcdef1234567890abcdef1234567890abcdef12";
        let id = id_or_hash(hash).unwrap();
        assert!(matches!(id, TorrentIdOrHash::Hash(_)));
    }

    #[test]
    fn id_or_hash_rejects_a_bare_integer_index() {
        // This is the exact footgun C2 guards against: librqbit's own parser
        // treats a bare integer as a session index, so `rm 0` must never
        // reach it.
        let err = id_or_hash("0").unwrap_err();
        assert!(matches!(err, EngineError::NotFound(_)));
    }

    #[test]
    fn id_or_hash_rejects_garbage_and_wrong_length_strings() {
        assert!(matches!(
            id_or_hash("not-a-hash").unwrap_err(),
            EngineError::NotFound(_)
        ));
        assert!(matches!(
            id_or_hash("abcdef").unwrap_err(),
            EngineError::NotFound(_)
        ));
    }
}
