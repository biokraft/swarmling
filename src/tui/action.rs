//! The vocabulary the TUI speaks in: `Action`s go into the app, `Effect`s come
//! out. Both are plain data — nothing here performs IO or knows how a key is
//! spelled on a particular terminal.

use std::path::PathBuf;
use std::time::Instant;

use crate::download::queue::QueueEntry;
use crate::sources::SearchResult;

/// Everything that can move the app forward.
#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    Resize(u16, u16),
    /// The current time, supplied from outside so the app never reads a clock.
    Tick(Instant),
    Key(KeyAction),
    SearchResults {
        source_id: &'static str,
        reports_health: bool,
        results: Vec<SearchResult>,
    },
    SearchFailed {
        source_id: &'static str,
        error: String,
    },
    SearchFinished,
    Notice(String),
    /// The event loop created and persisted a new default download directory.
    DownloadDirChanged(PathBuf),
    /// The event loop rewrote the queue file; this is its new contents.
    QueueChanged(Vec<QueueEntry>),
    /// What the live session currently reports about each torrent it carries.
    SnapshotsUpdated(Vec<crate::engine::TorrentSnapshot>),
    /// The VPN guard's view of the tunnel changed.
    GuardChanged(crate::vpn::guard::GuardState),
}

/// A key press already resolved to its meaning. The terminal layer owns the
/// mapping from physical keys to these; the app only ever sees intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    // navigation
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Tab,
    Enter,
    Escape,
    // browser
    Help,
    FolderPrompt,
    Quit,
    // results
    EditSearch,
    EditFilter,
    CycleSort,
    ToggleHideDead,
    Download,
    DownloadTo,
    CopyMagnet,
    // downloads
    RemoveEntry,
    ClearQueue,
    StartDownload,
    PauseDownload,
    // text editing
    Insert(String),
    Backspace,
    Delete,
    Home,
    End,
    WordLeft,
    WordRight,
    DeleteWordBefore,
    DeleteWordAfter,
    KillToEnd,
    ClearField,
}

/// A description of IO the app wants performed. Adding to the queue is a
/// record of intent only — it appends to the queue file exactly as
/// `swarmling add` does, and starts no transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StartSearch(String),
    AddToQueue {
        infohash: String,
        magnet: String,
        title: String,
        dir: PathBuf,
        /// The source the torrent was found through, so the queue can show
        /// where it came from. `None` for a pasted magnet.
        source_id: Option<String>,
    },
    RemoveFromQueue(String),
    ClearQueue,
    StartDownload(String),
    PauseDownload(String),
    RemoveDownload {
        infohash: String,
        delete_files: bool,
    },
    CopyToClipboard(String),
    SaveDownloadDir(PathBuf),
    Quit,
}
