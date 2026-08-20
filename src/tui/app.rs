//! The pure state machine at the centre of the TUI.
//!
//! `App::update` is the only way state changes. It never awaits, never reads a
//! file or the clipboard, and never reads the clock — the current time arrives
//! as `Action::Tick(now)`. Everything it wants done in the outside world comes
//! back as an [`Effect`] for the runtime to perform.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::download::queue::QueueEntry;
use crate::sources::magnet::parse_magnet;
use crate::sources::{registry::all_sources, SourceGroup};
use crate::tui::action::{Action, Effect, KeyAction};
use crate::tui::results::Results;
use crate::tui::textfield::TextField;

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
    Normal,
    Search,
    Filter,
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
        }
    }

    pub fn notice_text(&self) -> Option<&str> {
        self.notice.as_ref().map(|n| n.text.as_str())
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

    /// Move to a section, resetting the view state that belonged to the old one.
    pub fn set_section(&mut self, section: Section) {
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
    fn queue_magnet(&mut self, magnet: &str, title: Option<&str>, dir: PathBuf) -> Vec<Effect> {
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
            Action::Key(key) => self.on_key(key),
        }
    }

    fn on_key(&mut self, key: KeyAction) -> Vec<Effect> {
        // 1. An open overlay consumes the key.
        if self.overlay != Overlay::None {
            return self.on_key_overlay(key);
        }
        // 2. An editing mode routes text keys to the field.
        if self.mode == Mode::Search || self.mode == Mode::Filter {
            if let Some(effects) = self.on_key_editing(&key) {
                return effects;
            }
        }
        // 3. Region and section commands.
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
                    let effects = self.queue_magnet(&pending.magnet, Some(&pending.title), dir);
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
            let effects = self.queue_magnet(&raw, None, dir);
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

    fn selected_magnet(&self) -> Option<(String, String)> {
        self.results
            .selected()
            .map(|row| (row.result.magnet.clone(), row.result.title.clone()))
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
                } else {
                    self.download_selected(None)
                }
            }
            KeyAction::EditSearch => {
                self.mode = Mode::Search;
                self.field.set_value(&self.query.clone());
                Vec::new()
            }
            KeyAction::EditFilter => {
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
                let Some((magnet, title)) = self.selected_magnet() else {
                    return Vec::new();
                };
                let Some(parsed) = parse_magnet(&magnet) else {
                    self.set_notice("That does not look like a valid magnet link");
                    return Vec::new();
                };
                self.pending = Some(Pending {
                    infohash: parsed.infohash,
                    magnet,
                    title,
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
                Some((magnet, _)) => {
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
                vec![Effect::RemoveFromQueue(infohash)]
            }
            KeyAction::ClearQueue => {
                if self.section != Section::Downloads || self.queue.is_empty() {
                    return Vec::new();
                }
                vec![Effect::ClearQueue]
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
        let Some((magnet, title)) = self.selected_magnet() else {
            return Vec::new();
        };
        let dir = dir.unwrap_or_else(|| self.download_dir.clone());
        self.queue_magnet(&magnet, Some(&title), dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            }],
        );
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(Section::Downloads);
        a.region = Region::Content;
        let effects = a.update(Action::Key(KeyAction::RemoveEntry));
        assert!(matches!(
            effects.as_slice(),
            [Effect::RemoveFromQueue(h)] if h == &"aa".repeat(20)
        ));
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
            },
        ]));
        assert_eq!(a.queue.len(), 1);
        a.update(Action::QueueChanged(Vec::new()));
        assert_eq!(a.queue.len(), 0);
        assert_eq!(a.queue_cursor, 0);
    }
}
