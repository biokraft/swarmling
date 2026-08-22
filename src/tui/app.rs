//! The pure state machine at the centre of the TUI.
//!
//! `App::update` is the only way state changes. It never awaits, never reads a
//! file or the clipboard, and never reads the clock — the current time arrives
//! as `Action::Tick(now)`. Everything it wants done in the outside world comes
//! back as an [`Effect`] for the runtime to perform.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::download::queue::QueueEntry;
use crate::engine::TorrentSnapshot;
use crate::sources::magnet::parse_magnet;
use crate::sources::{registry::all_sources, SourceGroup};
use crate::tui::action::{Action, Effect, KeyAction};
use crate::tui::results::Results;
use crate::tui::textfield::TextField;
use crate::vpn::guard::GuardState;

/// How long a notice stays on screen.
const NOTICE_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Splash,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    All,
    Games,
    Movies,
    Tv,
    Anime,
    Downloads,
}

impl Section {
    /// The sidebar order, used by the up/down keys and by the renderer.
    pub const ORDER: [Section; 6] = [
        Section::All,
        Section::Games,
        Section::Movies,
        Section::Tv,
        Section::Anime,
        Section::Downloads,
    ];

    /// The results group this section filters to, if any.
    pub fn group(self) -> Option<SourceGroup> {
        match self {
            Section::Games => Some(SourceGroup::Games),
            Section::Movies => Some(SourceGroup::Movies),
            Section::Tv => Some(SourceGroup::Tv),
            Section::Anime => Some(SourceGroup::Anime),
            Section::All | Section::Downloads => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::All => "All",
            Section::Games => "Games",
            Section::Movies => "Movies",
            Section::Tv => "TV",
            Section::Anime => "Anime",
            Section::Downloads => "Downloads",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Sidebar,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The results list (or the queue list) has the keyboard.
    Normal,
    Search,
    Filter,
    /// One result is shown in full. It consumes its own `Escape`, and any
    /// event that replaces the list closes it.
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    /// The "set the default download folder" prompt, opened by `o`. It needs
    /// no selection and never queues anything.
    FolderPrompt,
    /// The per-torrent "download to…" prompt, opened by `D`. Its pending
    /// torrent lives in [`App::pending`] so the overlay itself stays `Copy`.
    DownloadTo,
}

/// A transient message shown to the user.
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    /// `None` until the first tick supplies a clock reading.
    pub expires_at: Option<Instant>,
}

/// A torrent waiting on the destination prompt.
#[derive(Debug, Clone)]
pub struct Pending {
    pub infohash: String,
    pub magnet: String,
    pub title: String,
    /// Shown in the prompt so the user can see which torrent they are about
    /// to put where.
    pub size_bytes: u64,
    pub source_id: Option<String>,
}

pub struct App {
    pub screen: Screen,
    pub section: Section,
    pub region: Region,
    pub mode: Mode,
    pub overlay: Overlay,
    pub field: TextField,
    pub results: Results,
    pub queue: Vec<QueueEntry>,
    pub queue_cursor: usize,
    pub download_dir: PathBuf,
    /// The destination last typed into the `D` prompt this session. It seeds
    /// the next `D` prompt but is never the default download folder.
    pub last_download_to: Option<PathBuf>,
    pub query: String,
    pub searching: bool,
    pub notice: Option<Notice>,
    pub now: Option<Instant>,
    pub should_quit: bool,
    pub size: (u16, u16),
    pub pending: Option<Pending>,
    /// Which groups each source belongs to, so streamed results can be
    /// classified without asking the registry again on every batch.
    groups: Vec<(&'static str, &'static [SourceGroup])>,
    /// The live session's latest report about each torrent it carries.
    snapshots: Vec<TorrentSnapshot>,
    /// The VPN guard's view of the tunnel.
    guard: GuardState,
}

impl App {
    pub fn new(download_dir: PathBuf, queue: Vec<QueueEntry>) -> App {
        App {
            screen: Screen::Splash,
            section: Section::All,
            region: Region::Sidebar,
            mode: Mode::Search,
            overlay: Overlay::None,
            field: TextField::new(),
            results: Results::new(),
            queue,
            queue_cursor: 0,
            download_dir,
            last_download_to: None,
            query: String::new(),
            searching: false,
            notice: None,
            now: None,
            should_quit: false,
            size: (80, 24),
            pending: None,
            groups: all_sources().iter().map(|s| (s.id(), s.groups())).collect(),
            snapshots: Vec::new(),
            guard: GuardState::Unprotected,
        }
    }

    pub fn notice_text(&self) -> Option<&str> {
        self.notice.as_ref().map(|n| n.text.as_str())
    }

    /// The live session's latest report about each torrent it carries.
    pub fn snapshots(&self) -> &[TorrentSnapshot] {
        &self.snapshots
    }

    /// The VPN guard's view of the tunnel.
    pub fn guard_state(&self) -> &GuardState {
        &self.guard
    }

    fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            expires_at: self.now.map(|n| n + NOTICE_TTL),
        });
    }

    fn groups_for(&self, source_id: &str) -> &'static [SourceGroup] {
        self.groups
            .iter()
            .find(|(id, _)| *id == source_id)
            .map(|(_, g)| *g)
            .unwrap_or(&[])
    }

    /// Leave the detail view. Called whenever the row it shows may no longer
    /// exist — a new search, a section change, a filter change — because a
    /// detail view of a vanished torrent is a false display.
    fn close_detail(&mut self) {
        if self.mode == Mode::Detail {
            self.mode = Mode::Normal;
        }
    }

    /// Move to a section, resetting the view state that belonged to the old one.
    pub fn set_section(&mut self, section: Section) {
        self.close_detail();
        self.section = section;
        self.results.set_filter("");
        self.results.set_group(section.group());
        if self.mode == Mode::Filter {
            self.mode = Mode::Normal;
            self.field.clear();
        }
        self.queue_cursor = self.queue_cursor.min(self.queue.len().saturating_sub(1));
    }

    fn is_queued(&self, infohash: &str) -> bool {
        self.queue.iter().any(|e| e.infohash == infohash)
    }

    /// Record the intent to download a magnet. Never starts a transfer.
    fn queue_magnet(
        &mut self,
        magnet: &str,
        title: Option<&str>,
        dir: PathBuf,
        source_id: Option<String>,
    ) -> Vec<Effect> {
        let Some(parsed) = parse_magnet(magnet) else {
            self.set_notice("That does not look like a valid magnet link");
            return Vec::new();
        };
        let title = title
            .map(|t| t.to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                if parsed.name.is_empty() {
                    parsed.infohash.clone()
                } else {
                    parsed.name.clone()
                }
            });
        if self.is_queued(&parsed.infohash) {
            self.set_notice(format!("Already in queue: {title}"));
            return Vec::new();
        }
        // No optimistic mutation: the queue file is the single source of
        // truth. The event loop writes it and reports back with
        // `Action::QueueChanged`, or with `Action::Notice` if it failed.
        self.set_notice(format!("Queued: {title}"));
        vec![Effect::AddToQueue {
            infohash: parsed.infohash,
            magnet: magnet.to_owned(),
            title,
            dir,
            source_id,
        }]
    }

    /// The single entry point. Dispatch order is deliberate and mirrored by the
    /// terminal key mapping: non-key actions, then overlays, then editing
    /// modes, then region and section commands.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Tick(now) => {
                self.now = Some(now);
                if let Some(notice) = self.notice.as_mut() {
                    match notice.expires_at {
                        None => notice.expires_at = Some(now + NOTICE_TTL),
                        Some(at) if now >= at => self.notice = None,
                        Some(_) => {}
                    }
                }
                Vec::new()
            }
            Action::Resize(w, h) => {
                self.size = (w, h);
                Vec::new()
            }
            Action::Quit => {
                self.should_quit = true;
                vec![Effect::Quit]
            }
            Action::Notice(text) => {
                self.set_notice(text);
                Vec::new()
            }
            Action::SearchResults {
                source_id,
                reports_health,
                results,
            } => {
                let groups = self.groups_for(source_id);
                // The list is being replaced; the detailed row may be gone.
                self.close_detail();
                self.results
                    .ingest(source_id, reports_health, groups, results);
                Vec::new()
            }
            Action::SearchFailed { source_id, error } => {
                self.results.fail(source_id, error);
                Vec::new()
            }
            Action::SearchFinished => {
                self.searching = false;
                Vec::new()
            }
            Action::DownloadDirChanged(dir) => {
                self.download_dir = dir;
                Vec::new()
            }
            Action::QueueChanged(queue) => {
                self.queue = queue;
                self.queue_cursor = self.queue_cursor.min(self.queue.len().saturating_sub(1));
                Vec::new()
            }
            Action::SnapshotsUpdated(snapshots) => {
                self.snapshots = snapshots;
                Vec::new()
            }
            Action::GuardChanged(guard) => {
                self.guard = guard;
                Vec::new()
            }
            Action::Key(key) => self.on_key(key),
        }
    }

    fn on_key(&mut self, key: KeyAction) -> Vec<Effect> {
        // 0. Quit is handled before anything else can swallow it. There must
        //    be no state the user can get stuck in, and both prompts route
        //    unknown keys into the text field.
        if key == KeyAction::Quit {
            self.should_quit = true;
            return vec![Effect::Quit];
        }
        // 1. An open overlay consumes the key.
        if self.overlay != Overlay::None {
            return self.on_key_overlay(key);
        }
        // 2. The detail view consumes its own Escape, so it can never also
        //    move the focus back to the sidebar.
        if self.mode == Mode::Detail {
            match key {
                KeyAction::Escape => {
                    self.mode = Mode::Normal;
                    return Vec::new();
                }
                // The same row, the same code path, the same effects.
                KeyAction::Download
                | KeyAction::DownloadTo
                | KeyAction::CopyMagnet
                | KeyAction::Help
                | KeyAction::Quit
                // Anything that replaces the list closes the view first.
                | KeyAction::EditSearch
                | KeyAction::EditFilter => return self.on_key_normal(key),
                // Nothing else may move the cursor out from under the view.
                _ => return Vec::new(),
            }
        }

        // 3. An editing mode routes text keys to the field.
        if self.mode == Mode::Search || self.mode == Mode::Filter {
            if let Some(effects) = self.on_key_editing(&key) {
                return effects;
            }
        }
        // 4. Region and section commands.
        self.on_key_normal(key)
    }

    fn on_key_overlay(&mut self, key: KeyAction) -> Vec<Effect> {
        match self.overlay {
            // Any key dismisses help, and does nothing else.
            Overlay::Help => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            Overlay::FolderPrompt => match key {
                KeyAction::Escape => {
                    self.overlay = Overlay::None;
                    self.field.clear();
                    Vec::new()
                }
                KeyAction::Enter => {
                    let dir = PathBuf::from(self.field.value().trim());
                    self.overlay = Overlay::None;
                    self.field.clear();
                    if dir.as_os_str().is_empty() {
                        self.set_notice("Enter a destination folder");
                        return Vec::new();
                    }
                    // `download_dir` changes only once the event loop has
                    // created and persisted the folder and told us so.
                    vec![Effect::SaveDownloadDir(dir)]
                }
                other => {
                    self.edit_field(&other);
                    Vec::new()
                }
            },
            Overlay::DownloadTo => match key {
                KeyAction::Escape => {
                    self.overlay = Overlay::None;
                    self.pending = None;
                    self.field.clear();
                    Vec::new()
                }
                KeyAction::Enter => {
                    let dir = PathBuf::from(self.field.value().trim());
                    self.overlay = Overlay::None;
                    self.field.clear();
                    let Some(pending) = self.pending.take() else {
                        return Vec::new();
                    };
                    if dir.as_os_str().is_empty() {
                        self.set_notice("Enter a destination folder");
                        return Vec::new();
                    }
                    // A per-torrent destination is not the new default.
                    self.last_download_to = Some(dir.clone());
                    let effects = self.queue_magnet(
                        &pending.magnet,
                        Some(&pending.title),
                        dir,
                        pending.source_id,
                    );
                    if !effects.is_empty() {
                        self.set_section(Section::Downloads);
                        self.region = Region::Content;
                    }
                    effects
                }
                other => {
                    self.edit_field(&other);
                    Vec::new()
                }
            },
            Overlay::None => Vec::new(),
        }
    }

    /// Apply a text-editing key to the field. Returns `true` if it was one.
    fn edit_field(&mut self, key: &KeyAction) -> bool {
        match key {
            KeyAction::Insert(text) => self.field.insert(text),
            KeyAction::Backspace => self.field.backspace(),
            KeyAction::Delete => self.field.delete(),
            KeyAction::Left => self.field.left(),
            KeyAction::Right => self.field.right(),
            KeyAction::Home => self.field.home(),
            KeyAction::End => self.field.end(),
            KeyAction::WordLeft => self.field.word_left(),
            KeyAction::WordRight => self.field.word_right(),
            KeyAction::DeleteWordBefore => self.field.delete_word_before(),
            KeyAction::DeleteWordAfter => self.field.delete_word_after(),
            KeyAction::KillToEnd => self.field.kill_to_end(),
            KeyAction::ClearField => self.field.clear(),
            _ => return false,
        }
        true
    }

    /// Keys while a field is being edited. `None` means "not mine, fall through".
    fn on_key_editing(&mut self, key: &KeyAction) -> Option<Vec<Effect>> {
        if self.edit_field(key) {
            if self.mode == Mode::Filter {
                let value = self.field.value().to_owned();
                self.results.set_filter(&value);
            }
            return Some(Vec::new());
        }
        match key {
            KeyAction::Enter => Some(self.submit_field()),
            // On the splash, Tab means "browse without searching": it
            // submits the field exactly as Enter does, so an empty field
            // opens the browser. In the browser's own search or filter box
            // it is swallowed rather than shifting the focus mid-edit.
            KeyAction::Tab => {
                if self.screen == Screen::Splash && self.mode == Mode::Search {
                    Some(self.submit_field())
                } else {
                    Some(Vec::new())
                }
            }
            KeyAction::Escape => {
                if self.mode == Mode::Filter {
                    self.results.set_filter("");
                    self.field.clear();
                    self.mode = Mode::Normal;
                    Some(Vec::new())
                } else if self.screen == Screen::Browser {
                    self.field.clear();
                    self.mode = Mode::Normal;
                    Some(Vec::new())
                } else {
                    // On the splash there is nothing behind the field.
                    None
                }
            }
            _ => None,
        }
    }

    fn submit_field(&mut self) -> Vec<Effect> {
        if self.mode == Mode::Filter {
            self.mode = Mode::Normal;
            self.region = Region::Content;
            return Vec::new();
        }
        let raw = self.field.value().trim().to_owned();
        self.close_detail();
        self.mode = Mode::Normal;
        self.screen = Screen::Browser;
        if raw.is_empty() {
            self.field.clear();
            return Vec::new();
        }
        // A pasted magnet is an intent to download, not a query.
        if parse_magnet(&raw).is_some() {
            self.field.clear();
            let dir = self.download_dir.clone();
            // A pasted magnet came from no source.
            let effects = self.queue_magnet(&raw, None, dir, None);
            self.set_section(Section::Downloads);
            self.region = Region::Content;
            return effects;
        }
        self.query = raw.clone();
        self.field.clear();
        self.results.clear();
        self.searching = true;
        self.region = Region::Content;
        vec![Effect::StartSearch(raw)]
    }

    fn page_size(&self) -> usize {
        usize::from(self.size.1.saturating_sub(8)).max(1)
    }

    fn move_selection(&mut self, delta: isize, page: bool) {
        match self.region {
            Region::Sidebar => {
                if page {
                    return;
                }
                let order = Section::ORDER;
                let current = order.iter().position(|s| *s == self.section).unwrap_or(0);
                let len = order.len() as isize;
                let next = (current as isize + delta).rem_euclid(len) as usize;
                if let Some(section) = order.get(next).copied() {
                    self.set_section(section);
                }
            }
            Region::Content => {
                if self.section == Section::Downloads {
                    let step = if page { self.page_size() as isize } else { 1 } * delta.signum();
                    let len = self.queue.len();
                    if len == 0 {
                        self.queue_cursor = 0;
                        return;
                    }
                    let next = (self.queue_cursor as isize + step).clamp(0, len as isize - 1);
                    self.queue_cursor = next.max(0) as usize;
                } else if page {
                    self.results.page(delta.signum(), self.page_size());
                } else {
                    self.results.move_cursor(delta, true);
                }
            }
        }
    }

    /// The selected row's magnet, title, and the source it came from.
    fn selected_magnet(&self) -> Option<(String, String, Option<String>)> {
        self.results.selected().map(|row| {
            (
                row.result.magnet.clone(),
                row.result.title.clone(),
                Some(row.result.source_id.to_owned()),
            )
        })
    }

    fn on_key_normal(&mut self, key: KeyAction) -> Vec<Effect> {
        match key {
            KeyAction::Quit => {
                self.should_quit = true;
                vec![Effect::Quit]
            }
            KeyAction::Help => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            KeyAction::Escape => {
                if self.region == Region::Content {
                    self.region = Region::Sidebar;
                } else {
                    self.screen = Screen::Splash;
                    self.mode = Mode::Search;
                    self.field.set_value(&self.query.clone());
                }
                Vec::new()
            }
            KeyAction::Tab => {
                self.region = match self.region {
                    Region::Sidebar => Region::Content,
                    Region::Content => Region::Sidebar,
                };
                Vec::new()
            }
            KeyAction::Left => {
                self.region = Region::Sidebar;
                Vec::new()
            }
            KeyAction::Right => {
                self.region = Region::Content;
                Vec::new()
            }
            KeyAction::Up => {
                self.move_selection(-1, false);
                Vec::new()
            }
            KeyAction::Down => {
                self.move_selection(1, false);
                Vec::new()
            }
            KeyAction::PageUp => {
                self.move_selection(-1, true);
                Vec::new()
            }
            KeyAction::PageDown => {
                self.move_selection(1, true);
                Vec::new()
            }
            KeyAction::Enter => {
                if self.region == Region::Sidebar {
                    self.region = Region::Content;
                    Vec::new()
                } else if self.section == Section::Downloads {
                    Vec::new()
                } else {
                    // A row must exist before there is anything to detail.
                    if self.results.selected().is_some() {
                        self.mode = Mode::Detail;
                    }
                    Vec::new()
                }
            }
            KeyAction::EditSearch => {
                self.close_detail();
                self.mode = Mode::Search;
                self.field.set_value(&self.query.clone());
                Vec::new()
            }
            KeyAction::EditFilter => {
                self.close_detail();
                self.mode = Mode::Filter;
                let current = self.results.filter().to_owned();
                self.field.set_value(&current);
                Vec::new()
            }
            KeyAction::CycleSort => {
                self.results.cycle_sort();
                Vec::new()
            }
            KeyAction::ToggleHideDead => {
                self.results.toggle_hide_dead();
                Vec::new()
            }
            KeyAction::Download => self.download_selected(None),
            KeyAction::FolderPrompt => {
                // Settings, not a download: no selection needed, and it can
                // never queue anything.
                self.overlay = Overlay::FolderPrompt;
                self.field
                    .set_value(self.download_dir.to_string_lossy().as_ref());
                self.field.end();
                Vec::new()
            }
            KeyAction::DownloadTo => {
                let Some((magnet, title, source_id)) = self.selected_magnet() else {
                    return Vec::new();
                };
                let Some(parsed) = parse_magnet(&magnet) else {
                    self.set_notice("That does not look like a valid magnet link");
                    return Vec::new();
                };
                let size_bytes = self
                    .results
                    .selected()
                    .map(|row| row.result.size_bytes)
                    .unwrap_or(0);
                self.pending = Some(Pending {
                    infohash: parsed.infohash,
                    magnet,
                    title,
                    size_bytes,
                    source_id,
                });
                self.overlay = Overlay::DownloadTo;
                let prefill = self
                    .last_download_to
                    .clone()
                    .unwrap_or_else(|| self.download_dir.clone());
                self.field.set_value(prefill.to_string_lossy().as_ref());
                self.field.end();
                Vec::new()
            }
            KeyAction::CopyMagnet => match self.selected_magnet() {
                Some((magnet, _, _)) => {
                    self.set_notice("Magnet copied");
                    vec![Effect::CopyToClipboard(magnet)]
                }
                None => Vec::new(),
            },
            KeyAction::RemoveEntry => {
                if self.section != Section::Downloads {
                    return Vec::new();
                }
                let Some(entry) = self.queue.get(self.queue_cursor) else {
                    return Vec::new();
                };
                let infohash = entry.infohash.clone();
                // Two effects, one intent: the queue file is the record of
                // what the user wants, and the session is what is actually
                // running. Dropping either half leaves them disagreeing.
                // `delete_files` stays false — removing a row is not consent
                // to delete what has already landed on disk.
                vec![
                    Effect::RemoveFromQueue(infohash.clone()),
                    Effect::RemoveDownload {
                        infohash,
                        delete_files: false,
                    },
                ]
            }
            KeyAction::ClearQueue => {
                if self.section != Section::Downloads || self.queue.is_empty() {
                    return Vec::new();
                }
                vec![Effect::ClearQueue]
            }
            KeyAction::StartDownload => {
                if self.section != Section::Downloads {
                    return Vec::new();
                }
                let Some(entry) = self.queue.get(self.queue_cursor) else {
                    return Vec::new();
                };
                vec![Effect::StartDownload(entry.infohash.clone())]
            }
            KeyAction::PauseDownload => {
                if self.section != Section::Downloads {
                    return Vec::new();
                }
                let Some(entry) = self.queue.get(self.queue_cursor) else {
                    return Vec::new();
                };
                vec![Effect::PauseDownload(entry.infohash.clone())]
            }
            KeyAction::Insert(_)
            | KeyAction::Backspace
            | KeyAction::Delete
            | KeyAction::Home
            | KeyAction::End
            | KeyAction::WordLeft
            | KeyAction::WordRight
            | KeyAction::DeleteWordBefore
            | KeyAction::DeleteWordAfter
            | KeyAction::KillToEnd
            | KeyAction::ClearField => Vec::new(),
        }
    }

    fn download_selected(&mut self, dir: Option<PathBuf>) -> Vec<Effect> {
        let Some((magnet, title, source_id)) = self.selected_magnet() else {
            return Vec::new();
        };
        let dir = dir.unwrap_or_else(|| self.download_dir.clone());
        self.queue_magnet(&magnet, Some(&title), dir, source_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_on_the_splash_browses_without_searching() {
        // The splash advertises "⇥ browse". Tab submits the field the same
        // way Enter does, so an empty field opens the browser and starts no
        // search at all.
        let mut app = App::new(std::path::PathBuf::from("/tmp/x"), Vec::new());
        assert_eq!(app.screen, Screen::Splash);
        let effects = app.update(Action::Key(KeyAction::Tab));
        assert_eq!(app.screen, Screen::Browser);
        assert_eq!(app.mode, Mode::Normal);
        assert!(effects.is_empty(), "an empty field must start no search");
        assert!(!app.searching);
    }

    #[test]
    fn tab_on_the_splash_runs_a_typed_query_like_enter() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/x"), Vec::new());
        app.update(Action::Key(KeyAction::Insert("ubuntu".into())));
        let effects = app.update(Action::Key(KeyAction::Tab));
        assert_eq!(effects, vec![Effect::StartSearch("ubuntu".into())]);
    }

    #[test]
    fn tab_does_not_shift_the_focus_out_of_the_browser_search_box() {
        let mut app = App::new(std::path::PathBuf::from("/tmp/x"), Vec::new());
        app.update(Action::Key(KeyAction::Enter));
        app.update(Action::Key(KeyAction::EditSearch));
        assert_eq!(app.mode, Mode::Search);
        let region = app.region;
        app.update(Action::Key(KeyAction::Tab));
        assert_eq!(app.mode, Mode::Search, "the search box lost the keyboard");
        assert_eq!(app.region, region);
    }

    fn app() -> App {
        App::new(
            std::path::PathBuf::from("/tmp/does-not-need-to-exist"),
            Vec::new(),
        )
    }

    #[test]
    fn submitting_a_query_leaves_the_splash_and_asks_for_a_search() {
        let mut a = app();
        assert_eq!(a.screen, Screen::Splash);
        a.update(Action::Key(KeyAction::Insert("ubuntu".into())));
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert_eq!(a.screen, Screen::Browser);
        assert!(matches!(effects.as_slice(), [Effect::StartSearch(q)] if q == "ubuntu"));
    }

    #[test]
    fn submitting_a_magnet_queues_it_instead_of_searching_for_it() {
        let mut a = app();
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x";
        a.update(Action::Key(KeyAction::Insert(magnet.into())));
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::StartSearch(_))),
            "a pasted magnet is an intent to download, not a query"
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::AddToQueue { infohash, dir, .. }
                    if infohash == "0123456789abcdef0123456789abcdef01234567"
                        && dir == &std::path::PathBuf::from("/tmp/does-not-need-to-exist")
            )),
            "the magnet's infohash and the default folder must reach the queue"
        );
        assert_eq!(a.section, Section::Downloads);
    }

    #[test]
    fn escape_walks_out_one_level_at_a_time() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter)); // to browser
        a.region = Region::Content;
        a.update(Action::Key(KeyAction::Escape));
        assert_eq!(a.region, Region::Sidebar);
        a.update(Action::Key(KeyAction::Escape));
        assert_eq!(a.screen, Screen::Splash);
    }

    #[test]
    fn any_key_closes_the_help_overlay_and_does_nothing_else() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::Key(KeyAction::Help));
        assert_eq!(a.overlay, Overlay::Help);
        let before = a.section;
        a.update(Action::Key(KeyAction::Down));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.section, before, "the dismissing key must not also act");
    }

    #[test]
    fn adding_a_torrent_already_in_the_queue_is_refused_with_a_notice() {
        let mut a = App::new(
            std::path::PathBuf::from("/tmp/x"),
            vec![crate::download::queue::QueueEntry {
                infohash: "0123456789abcdef0123456789abcdef01234567".into(),
                magnet: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into(),
                title: "already here".into(),
                added_unix: 0,
                paused: false,
                source_id: None,
                dir: None,
            }],
        );
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x";
        a.update(Action::Key(KeyAction::Insert(magnet.into())));
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::AddToQueue { .. })),
            "a duplicate must not be queued twice"
        );
        assert!(
            a.notice_text()
                .unwrap_or_default()
                .starts_with("Already in queue"),
            "the refusal must say why: {:?}",
            a.notice_text()
        );
        assert_eq!(a.queue.len(), 1, "the queue must be untouched");
    }

    #[test]
    fn a_notice_expires_on_a_tick_past_its_deadline_without_the_app_reading_a_clock() {
        let mut a = app();
        let t0 = std::time::Instant::now();
        a.update(Action::Tick(t0));
        a.update(Action::Notice("hello".into()));
        assert!(a.notice.is_some());
        a.update(Action::Tick(t0 + std::time::Duration::from_secs(1)));
        assert!(a.notice.is_some(), "a notice must survive a short tick");
        a.update(Action::Tick(t0 + std::time::Duration::from_secs(5)));
        assert!(a.notice.is_none(), "a notice must expire");
    }

    #[test]
    fn a_failed_source_surfaces_instead_of_looking_like_an_empty_result_set() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::SearchFailed {
            source_id: "nyaa",
            error: "timed out".into(),
        });
        assert_eq!(a.results.failures().len(), 1);
    }

    #[test]
    fn switching_section_clears_the_filter() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.results.set_filter("something");
        a.region = Region::Sidebar;
        a.update(Action::Key(KeyAction::Down)); // All -> Games
        assert_eq!(a.section, Section::Games);
        assert_eq!(a.results.filter(), "");
    }

    #[test]
    fn removing_a_queue_entry_never_asks_to_delete_a_file() {
        // Queue entries are records of intent. Removing one must not carry
        // any instruction to touch the user's disk.
        let mut a = App::new(
            std::path::PathBuf::from("/tmp/x"),
            vec![crate::download::queue::QueueEntry {
                infohash: "aa".repeat(20),
                magnet: "magnet:?xt=urn:btih:".to_owned() + &"aa".repeat(20),
                title: "t".into(),
                added_unix: 0,
                paused: false,
                source_id: None,
                dir: None,
            }],
        );
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        let effects = a.update(Action::Key(KeyAction::RemoveEntry));
        // Removing a row must take the torrent out of the live session as
        // well as out of the queue file: rewriting the file alone would leave
        // a running transfer nothing on screen refers to.
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::RemoveFromQueue(h), Effect::RemoveDownload { infohash, delete_files: false }]
                    if h == &"aa".repeat(20) && infohash == &"aa".repeat(20)
            ),
            "{effects:?}"
        );
    }

    #[test]
    fn the_quit_key_sets_should_quit_and_emits_the_quit_effect() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        assert!(!a.should_quit);
        let effects = a.update(Action::Key(KeyAction::Quit));
        assert!(a.should_quit);
        assert!(effects.iter().any(|e| matches!(e, Effect::Quit)));
    }

    fn row(magnet: &str, title: &str) -> crate::sources::SearchResult {
        crate::sources::SearchResult {
            title: title.into(),
            magnet: magnet.into(),
            size_bytes: 1,
            seeders: 1,
            leechers: 0,
            source_id: "nyaa",
            infohash: crate::sources::magnet::parse_magnet(magnet)
                .map(|m| m.infohash)
                .unwrap_or_default(),
        }
    }

    fn browsing_with_one_result() -> App {
        let mut a = app();
        a.update(Action::Key(KeyAction::Insert("ubuntu".into())));
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::SearchResults {
            source_id: "nyaa",
            reports_health: true,
            results: vec![row(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x",
                "a title",
            )],
        });
        a.region = Region::Content;
        a
    }

    #[test]
    fn the_download_to_prompt_queues_to_the_folder_the_user_typed() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::DownloadTo));
        assert_eq!(a.overlay, Overlay::DownloadTo);
        a.update(Action::Key(KeyAction::ClearField));
        a.update(Action::Key(KeyAction::Insert("/tmp/elsewhere".into())));
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert_eq!(a.overlay, Overlay::None);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::AddToQueue { dir, .. } if dir == &std::path::PathBuf::from("/tmp/elsewhere")
            )),
            "D must queue to the folder the user typed"
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SaveDownloadDir(_))),
            "a per-torrent destination is not the new default folder"
        );
        assert_eq!(
            a.download_dir,
            std::path::PathBuf::from("/tmp/does-not-need-to-exist")
        );
        assert_eq!(
            a.last_download_to,
            Some(std::path::PathBuf::from("/tmp/elsewhere"))
        );
        assert_eq!(
            a.section,
            Section::Downloads,
            "a successful D shows the queue"
        );
    }

    #[test]
    fn the_download_to_prompt_is_prefilled_with_the_last_destination_not_the_default() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::DownloadTo));
        a.update(Action::Key(KeyAction::ClearField));
        a.update(Action::Key(KeyAction::Insert("/tmp/elsewhere".into())));
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::Key(KeyAction::DownloadTo));
        assert_eq!(a.field.value(), "/tmp/elsewhere");
    }

    #[test]
    fn escaping_the_download_to_prompt_queues_nothing() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::DownloadTo));
        let effects = a.update(Action::Key(KeyAction::Escape));
        assert!(effects.is_empty());
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.pending.is_none());
        assert!(a.queue.is_empty());
    }

    #[test]
    fn filter_mode_edits_the_results_filter_and_escape_restores_it() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::EditFilter));
        a.update(Action::Key(KeyAction::Insert("nomatch".into())));
        assert_eq!(a.results.filter(), "nomatch");
        assert!(a.results.visible().is_empty());
        a.update(Action::Key(KeyAction::Escape));
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.results.filter(), "");
        assert_eq!(a.results.visible().len(), 1);
    }

    #[test]
    fn copying_a_magnet_asks_for_the_clipboard_and_nothing_else() {
        let mut a = browsing_with_one_result();
        let effects = a.update(Action::Key(KeyAction::CopyMagnet));
        assert!(matches!(effects.as_slice(), [Effect::CopyToClipboard(_)]));
    }

    #[test]
    fn sidebar_up_and_down_wrap_through_every_section() {
        let mut a = browsing_with_one_result();
        a.region = Region::Sidebar;
        a.update(Action::Key(KeyAction::Up));
        assert_eq!(a.section, Section::Downloads);
        a.update(Action::Key(KeyAction::Down));
        assert_eq!(a.section, Section::All);
    }

    #[test]
    fn removing_from_an_empty_queue_does_nothing() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        assert!(a.update(Action::Key(KeyAction::RemoveEntry)).is_empty());
        assert!(a.update(Action::Key(KeyAction::ClearQueue)).is_empty());
    }

    #[test]
    fn the_folder_prompt_opens_with_nothing_selected_and_no_results() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        assert!(a.results.visible().is_empty());
        assert!(a.results.selected().is_none());
        let effects = a.update(Action::Key(KeyAction::FolderPrompt));
        assert!(effects.is_empty());
        assert_eq!(
            a.overlay,
            Overlay::FolderPrompt,
            "o is a settings key: it must not need a selection"
        );
        assert_eq!(a.field.value(), "/tmp/does-not-need-to-exist");
    }

    #[test]
    fn submitting_the_folder_prompt_saves_the_folder_and_queues_nothing() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::FolderPrompt));
        a.update(Action::Key(KeyAction::ClearField));
        a.update(Action::Key(KeyAction::Insert("/tmp/newdefault".into())));
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert_eq!(
            effects.as_slice(),
            [Effect::SaveDownloadDir(std::path::PathBuf::from(
                "/tmp/newdefault"
            ))],
            "o sets the default folder and does nothing else"
        );
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(
            a.download_dir,
            std::path::PathBuf::from("/tmp/does-not-need-to-exist"),
            "the folder changes only when the event loop confirms it"
        );
    }

    #[test]
    fn escaping_the_folder_prompt_emits_nothing() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::FolderPrompt));
        let effects = a.update(Action::Key(KeyAction::Escape));
        assert!(effects.is_empty());
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(
            a.download_dir,
            std::path::PathBuf::from("/tmp/does-not-need-to-exist")
        );
    }

    #[test]
    fn the_event_loop_owns_the_queue_and_the_default_folder() {
        let mut a = app();
        a.update(Action::DownloadDirChanged(std::path::PathBuf::from(
            "/tmp/confirmed",
        )));
        assert_eq!(a.download_dir, std::path::PathBuf::from("/tmp/confirmed"));
        a.update(Action::QueueChanged(vec![
            crate::download::queue::QueueEntry {
                infohash: "aa".repeat(20),
                magnet: "magnet:?xt=urn:btih:".to_owned() + &"aa".repeat(20),
                title: "t".into(),
                added_unix: 0,
                paused: false,
                source_id: None,
                dir: None,
            },
        ]));
        assert_eq!(a.queue.len(), 1);
        a.update(Action::QueueChanged(Vec::new()));
        assert_eq!(a.queue.len(), 0);
        assert_eq!(a.queue_cursor, 0);
    }

    #[test]
    fn enter_on_a_result_opens_the_detail_view() {
        let mut a = browsing_with_one_result();
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert!(
            effects.is_empty(),
            "opening a detail view is not a download"
        );
        assert_eq!(a.mode, Mode::Detail);
    }

    #[test]
    fn enter_with_nothing_selected_does_not_open_a_detail_view() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Insert("ubuntu".into())));
        a.update(Action::Key(KeyAction::Enter));
        a.region = Region::Content;
        assert!(a.results.selected().is_none());
        let effects = a.update(Action::Key(KeyAction::Enter));
        assert!(effects.is_empty());
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn escape_leaves_the_detail_view_without_leaving_the_content_region() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::Enter));
        assert_eq!(a.mode, Mode::Detail);
        a.update(Action::Key(KeyAction::Escape));
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(
            a.region,
            Region::Content,
            "the detail view consumes its own Escape; it must not also move focus"
        );
    }

    #[test]
    fn downloading_from_the_detail_view_produces_the_same_effect_as_from_the_list() {
        let mut from_list = browsing_with_one_result();
        let listed = from_list.update(Action::Key(KeyAction::Download));

        let mut from_detail = browsing_with_one_result();
        from_detail.update(Action::Key(KeyAction::Enter));
        let detailed = from_detail.update(Action::Key(KeyAction::Download));

        assert_eq!(listed, detailed);
        assert!(!listed.is_empty());

        let mut copying = browsing_with_one_result();
        copying.update(Action::Key(KeyAction::Enter));
        assert!(matches!(
            copying
                .update(Action::Key(KeyAction::CopyMagnet))
                .as_slice(),
            [Effect::CopyToClipboard(_)]
        ));
    }

    #[test]
    fn a_new_search_closes_a_stale_detail_view() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::Enter));
        assert_eq!(a.mode, Mode::Detail);
        a.update(Action::SearchResults {
            source_id: "nyaa",
            reports_health: true,
            results: vec![row(
                "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef&dn=y",
                "a different torrent",
            )],
        });
        assert_eq!(
            a.mode,
            Mode::Normal,
            "a detail view of a row that may be gone is a false display"
        );
    }

    #[test]
    fn changing_the_section_or_the_filter_closes_the_detail_view() {
        let mut a = browsing_with_one_result();
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(Section::Games);
        assert_eq!(a.mode, Mode::Normal);

        let mut b = browsing_with_one_result();
        b.update(Action::Key(KeyAction::Enter));
        b.update(Action::Key(KeyAction::EditFilter));
        assert_eq!(b.mode, Mode::Filter);
    }
    fn entry(infohash: &str) -> crate::download::queue::QueueEntry {
        crate::download::queue::QueueEntry {
            infohash: infohash.to_string(),
            magnet: format!("magnet:?xt=urn:btih:{infohash}"),
            title: format!("torrent {infohash}"),
            added_unix: 0,
            paused: false,
            source_id: None,
            dir: None,
        }
    }

    fn snapshot(
        infohash: &str,
        progress_bytes: u64,
        total_bytes: u64,
    ) -> crate::engine::TorrentSnapshot {
        crate::engine::TorrentSnapshot {
            infohash: infohash.to_string(),
            name: format!("torrent {infohash}"),
            state: crate::engine::TorrentState::Downloading,
            progress_bytes,
            total_bytes,
            download_speed: 1024,
            upload_speed: 0,
            error: None,
        }
    }

    #[test]
    fn s_starts_the_highlighted_download() {
        let mut a = app();
        a.update(Action::QueueChanged(vec![entry("aa"), entry("bb")]));
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        let effects = a.update(Action::Key(KeyAction::StartDownload));
        assert!(
            effects.contains(&Effect::StartDownload("aa".to_string())),
            "expected a start for the highlighted entry: {effects:?}"
        );
    }

    #[test]
    fn p_pauses_the_highlighted_download() {
        let mut a = app();
        a.update(Action::QueueChanged(vec![entry("aa")]));
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        let effects = a.update(Action::Key(KeyAction::PauseDownload));
        assert!(
            effects.contains(&Effect::PauseDownload("aa".to_string())),
            "expected a pause: {effects:?}"
        );
    }

    #[test]
    fn start_and_pause_do_nothing_without_a_selection() {
        // In the Downloads section with an empty queue, so this actually
        // reaches the `queue.get(self.queue_cursor)` guard rather than
        // exiting earlier on the section check.
        let mut a = app();
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        assert!(a.update(Action::Key(KeyAction::StartDownload)).is_empty());
        assert!(a.update(Action::Key(KeyAction::PauseDownload)).is_empty());
    }

    #[test]
    fn snapshots_are_stored_for_the_downloads_panel() {
        let mut a = app();
        a.update(Action::SnapshotsUpdated(vec![snapshot("aa", 512, 1024)]));
        assert_eq!(a.snapshots().len(), 1);
        assert_eq!(a.snapshots()[0].progress_bytes, 512);
    }

    #[test]
    fn the_guard_state_is_stored() {
        use crate::vpn::guard::GuardState;
        let mut a = app();
        a.update(Action::GuardChanged(GuardState::Protected {
            device: "utun4".into(),
        }));
        assert_eq!(
            a.guard_state(),
            &GuardState::Protected {
                device: "utun4".to_string()
            }
        );
    }

    #[test]
    fn quit_is_reachable_from_every_overlay_and_mode() {
        // map_key already produces Quit everywhere and a mapper-level test
        // asserted it — but the App swallowed it: the help overlay dismissed
        // on any key, and the two prompts handed every unknown key to the
        // text field. Ctrl-C in an open prompt did nothing at all. The
        // assertion has to go through App::update to catch that.
        type SetUp = fn(&mut App);
        let states: [(&str, SetUp); 6] = [
            ("help overlay", |a: &mut App| a.overlay = Overlay::Help),
            ("folder prompt", |a: &mut App| {
                a.overlay = Overlay::FolderPrompt
            }),
            ("download-to prompt", |a: &mut App| {
                a.overlay = Overlay::DownloadTo
            }),
            ("search mode", |a: &mut App| a.mode = Mode::Search),
            ("filter mode", |a: &mut App| a.mode = Mode::Filter),
            ("detail view", |a: &mut App| a.mode = Mode::Detail),
        ];
        for (name, set_up) in states {
            let mut app = App::new(PathBuf::from("/tmp"), Vec::new());
            set_up(&mut app);
            let effects = app.update(Action::Key(KeyAction::Quit));
            assert!(app.should_quit, "{name}: quit was swallowed");
            assert_eq!(effects, vec![Effect::Quit], "{name}");
        }
    }
}
