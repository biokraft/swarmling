pub mod fake;
pub mod librqbit_engine;

use std::path::Path;

pub type InfoHash = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentState {
    Checking,
    Downloading,
    Seeding,
    Paused,
    Errored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentSnapshot {
    pub infohash: InfoHash,
    pub name: String,
    pub state: TorrentState,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub error: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("no such torrent: {0}")]
    NotFound(String),
    #[error("not a usable magnet: {0}")]
    InvalidMagnet(String),
    #[error("torrent backend failed: {0}")]
    Backend(String),
}

/// Everything swarmling needs from a torrent backend, in swarmling's own
/// vocabulary. Keeping this deliberately small is what lets the queue, the
/// persistence layer and (later) the TUI be tested without a real peer.
#[async_trait::async_trait]
pub trait TorrentEngine: Send + Sync {
    async fn add_magnet(
        &self,
        magnet: &str,
        output_dir: &Path,
        paused: bool,
    ) -> Result<InfoHash, EngineError>;
    async fn pause(&self, infohash: &str) -> Result<(), EngineError>;
    async fn resume(&self, infohash: &str) -> Result<(), EngineError>;
    async fn remove(&self, infohash: &str, delete_files: bool) -> Result<(), EngineError>;
    async fn snapshot(&self, infohash: &str) -> Result<TorrentSnapshot, EngineError>;
    async fn snapshots(&self) -> Vec<TorrentSnapshot>;
}
