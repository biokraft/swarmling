//! Maps a physical key press to a [`KeyAction`], given the parts of [`App`]
//! state that change what a key means. Pure lookup: no state, no IO.
//!
//! [`App`]: crate::tui::app::App

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::tui::action::KeyAction;
use crate::tui::app::{Mode, Overlay, Region, Section};

/// The slice of [`App`](crate::tui::app::App) state that changes what a key
/// means. Cheap to copy so callers can build it fresh on every key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub mode: Mode,
    pub overlay: Overlay,
    pub region: Region,
    pub section: Section,
}

/// Whether the field currently owns the keyboard: typed characters go into a
/// text buffer rather than firing a command.
fn is_editing(ctx: Context) -> bool {
    matches!(ctx.mode, Mode::Search | Mode::Filter)
        || matches!(ctx.overlay, Overlay::FolderPrompt | Overlay::DownloadTo)
}

/// Readline-style editing keys shared by the search box, the filter box and
/// both text-entry overlays.
fn editing_key(key: KeyEvent) -> Option<KeyAction> {
    match (key.modifiers, key.code) {
        // Ctrl-modified readline bindings.
        (KeyModifiers::CONTROL, KeyCode::Char('a')) => Some(KeyAction::Home),
        (KeyModifiers::CONTROL, KeyCode::Char('e')) => Some(KeyAction::End),
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => Some(KeyAction::ClearField),
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => Some(KeyAction::KillToEnd),
        (KeyModifiers::CONTROL, KeyCode::Char('w')) => Some(KeyAction::DeleteWordBefore),
        (KeyModifiers::CONTROL, KeyCode::Left) => Some(KeyAction::WordLeft),
        (KeyModifiers::CONTROL, KeyCode::Right) => Some(KeyAction::WordRight),
        (KeyModifiers::CONTROL, KeyCode::Delete) => Some(KeyAction::DeleteWordAfter),

        // Unmodified control keys.
        (KeyModifiers::NONE, KeyCode::Backspace) => Some(KeyAction::Backspace),
        (KeyModifiers::NONE, KeyCode::Delete) => Some(KeyAction::Delete),
        (KeyModifiers::NONE, KeyCode::Home) => Some(KeyAction::Home),
        (KeyModifiers::NONE, KeyCode::End) => Some(KeyAction::End),
        (KeyModifiers::NONE, KeyCode::Left) => Some(KeyAction::Left),
        (KeyModifiers::NONE, KeyCode::Right) => Some(KeyAction::Right),
        (KeyModifiers::NONE, KeyCode::Enter) => Some(KeyAction::Enter),
        (KeyModifiers::NONE, KeyCode::Esc) => Some(KeyAction::Escape),
        // Tab leaves the field rather than typing into it. On the splash the
        // app reads it as "browse without searching"; elsewhere it is
        // swallowed so it cannot move the focus mid-edit.
        (KeyModifiers::NONE, KeyCode::Tab) => Some(KeyAction::Tab),

        // Plain text: only NONE or SHIFT may reach the buffer. Anything else
        // unbound here (e.g. Alt, or a stray control character) falls
        // through to `None` — a stray control byte in the search field would
        // be sent to a source as part of the query.
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            Some(KeyAction::Insert(c.to_string()))
        }

        _ => None,
    }
}

/// Keys handled by the help overlay. It shows no editable field, so only
/// dismissal keys are bound.
fn help_overlay_key(key: KeyEvent) -> Option<KeyAction> {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => Some(KeyAction::Escape),
        (KeyModifiers::NONE, KeyCode::Char('?')) => Some(KeyAction::Help),
        (KeyModifiers::NONE, KeyCode::Char('q')) => Some(KeyAction::Quit),
        _ => None,
    }
}

/// Keys bound while browsing (no overlay, not editing): navigation, plus
/// the section-specific command set.
fn browsing_key(key: KeyEvent, ctx: Context) -> Option<KeyAction> {
    if ctx.mode == Mode::Detail {
        return detail_key(key);
    }
    if let Some(action) = navigation_key(key) {
        return Some(action);
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Char('?')) => Some(KeyAction::Help),
        (KeyModifiers::NONE, KeyCode::Char('o')) => Some(KeyAction::FolderPrompt),
        (KeyModifiers::NONE, KeyCode::Char('q')) => Some(KeyAction::Quit),
        _ => section_key(key, ctx.section),
    }
}

/// The detail view: leave it, or act on the torrent it shows. It is not an
/// editing context, so a plain character is a command, never text.
fn detail_key(key: KeyEvent) -> Option<KeyAction> {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => Some(KeyAction::Escape),
        // Enter is deliberately unbound here: the detail view is already the
        // opened row, and the App does not forward it. A key that maps to an
        // action nothing handles is a dead key that looks alive.
        (KeyModifiers::NONE, KeyCode::Char('d')) => Some(KeyAction::Download),
        (KeyModifiers::SHIFT, KeyCode::Char('D')) => Some(KeyAction::DownloadTo),
        (KeyModifiers::NONE, KeyCode::Char('y')) => Some(KeyAction::CopyMagnet),
        (KeyModifiers::NONE, KeyCode::Char('?')) => Some(KeyAction::Help),
        (KeyModifiers::NONE, KeyCode::Char('q')) => Some(KeyAction::Quit),
        _ => None,
    }
}

/// Arrow-like navigation, bound the same way regardless of section.
fn navigation_key(key: KeyEvent) -> Option<KeyAction> {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => Some(KeyAction::Up),
        (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => Some(KeyAction::Down),
        (KeyModifiers::NONE, KeyCode::Char('h') | KeyCode::Left) => Some(KeyAction::Left),
        (KeyModifiers::NONE, KeyCode::Char('l') | KeyCode::Right) => Some(KeyAction::Right),
        (KeyModifiers::NONE, KeyCode::PageUp) => Some(KeyAction::PageUp),
        (KeyModifiers::NONE, KeyCode::PageDown) => Some(KeyAction::PageDown),
        (KeyModifiers::NONE, KeyCode::Tab) => Some(KeyAction::Tab),
        (KeyModifiers::NONE, KeyCode::Esc) => Some(KeyAction::Escape),
        (KeyModifiers::NONE, KeyCode::Enter) => Some(KeyAction::Enter),
        _ => None,
    }
}

/// The commands that only make sense for one section: the results-list
/// bindings everywhere except `Downloads`, and the queue-management bindings
/// only in `Downloads`.
fn section_key(key: KeyEvent, section: Section) -> Option<KeyAction> {
    if section == Section::Downloads {
        return match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Char('c')) => Some(KeyAction::RemoveEntry),
            (KeyModifiers::SHIFT, KeyCode::Char('C')) => Some(KeyAction::ClearQueue),
            (KeyModifiers::NONE, KeyCode::Char('s')) => Some(KeyAction::StartDownload),
            (KeyModifiers::NONE, KeyCode::Char('p')) => Some(KeyAction::PauseDownload),
            _ => None,
        };
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Char('/')) => Some(KeyAction::EditSearch),
        (KeyModifiers::NONE, KeyCode::Char('f')) => Some(KeyAction::EditFilter),
        (KeyModifiers::NONE, KeyCode::Char('s')) => Some(KeyAction::CycleSort),
        (KeyModifiers::NONE, KeyCode::Char('z')) => Some(KeyAction::ToggleHideDead),
        (KeyModifiers::NONE, KeyCode::Char('d')) => Some(KeyAction::Download),
        (KeyModifiers::SHIFT, KeyCode::Char('D')) => Some(KeyAction::DownloadTo),
        (KeyModifiers::NONE, KeyCode::Char('y')) => Some(KeyAction::CopyMagnet),
        _ => None,
    }
}

/// One documented binding: the key as it is shown to the user, a terse
/// footer label, the help card's description, the action the key must
/// produce, and the context it produces it in.
///
/// This table is the single source of truth for both the help card and the
/// footer hints. `every_help_entry_names_a_key_that_really_does_what_it_claims`
/// checks each row against `map_key`, so a binding that moves in the mapper
/// without moving here fails the suite rather than lying on screen.
#[derive(Debug, Clone)]
pub struct Help {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub action: KeyAction,
    pub ctx: Context,
    /// Whether the footer has room to mention it. Navigation keys are left
    /// out of the footer and documented on the help card only.
    pub in_footer: bool,
}

/// Browsing a results list.
pub const LIST_CTX: Context = Context {
    mode: Mode::Normal,
    overlay: Overlay::None,
    region: Region::Content,
    section: Section::All,
};

/// Browsing the queue.
pub const QUEUE_CTX: Context = Context {
    mode: Mode::Normal,
    overlay: Overlay::None,
    region: Region::Content,
    section: Section::Downloads,
};

/// The detail view of one result.
pub const DETAIL_CTX: Context = Context {
    mode: Mode::Detail,
    overlay: Overlay::None,
    region: Region::Content,
    section: Section::All,
};

const fn nav(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    action: KeyAction,
    ctx: Context,
) -> Help {
    Help {
        key,
        label,
        description,
        action,
        ctx,
        in_footer: false,
    }
}

const fn cmd(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    action: KeyAction,
    ctx: Context,
) -> Help {
    Help {
        key,
        label,
        description,
        action,
        ctx,
        in_footer: true,
    }
}

pub const HELP: &[Help] = &[
    // Results list.
    nav("↑", "up", "Move up", KeyAction::Up, LIST_CTX),
    nav("k", "up", "Move up", KeyAction::Up, LIST_CTX),
    nav("↓", "down", "Move down", KeyAction::Down, LIST_CTX),
    nav("j", "down", "Move down", KeyAction::Down, LIST_CTX),
    nav(
        "⇥",
        "focus",
        "Switch between the sidebar and the list",
        KeyAction::Tab,
        LIST_CTX,
    ),
    cmd(
        "↵",
        "detail",
        "Open the selected result",
        KeyAction::Enter,
        LIST_CTX,
    ),
    cmd(
        "d",
        "download",
        "Queue a download",
        KeyAction::Download,
        LIST_CTX,
    ),
    cmd(
        "D",
        "download to…",
        "Queue a download to a chosen folder",
        KeyAction::DownloadTo,
        LIST_CTX,
    ),
    cmd(
        "y",
        "copy",
        "Copy the magnet link",
        KeyAction::CopyMagnet,
        LIST_CTX,
    ),
    cmd(
        "f",
        "filter",
        "Filter the results",
        KeyAction::EditFilter,
        LIST_CTX,
    ),
    cmd(
        "s",
        "sort",
        "Cycle the sort",
        KeyAction::CycleSort,
        LIST_CTX,
    ),
    cmd(
        "z",
        "dead",
        "Hide or show dead torrents",
        KeyAction::ToggleHideDead,
        LIST_CTX,
    ),
    cmd(
        "/",
        "search",
        "Start a new search",
        KeyAction::EditSearch,
        LIST_CTX,
    ),
    cmd(
        "o",
        "folder",
        "Set the default download folder",
        KeyAction::FolderPrompt,
        LIST_CTX,
    ),
    cmd("?", "help", "Show this help", KeyAction::Help, LIST_CTX),
    cmd("q", "quit", "Quit", KeyAction::Quit, LIST_CTX),
    // Queue.
    cmd(
        "c",
        "remove",
        "Remove one queue entry",
        KeyAction::RemoveEntry,
        QUEUE_CTX,
    ),
    cmd(
        "C",
        "clear",
        "Clear the whole queue",
        KeyAction::ClearQueue,
        QUEUE_CTX,
    ),
    cmd(
        "s",
        "start",
        "Start the highlighted download",
        KeyAction::StartDownload,
        QUEUE_CTX,
    ),
    cmd(
        "p",
        "pause",
        "Pause the highlighted download",
        KeyAction::PauseDownload,
        QUEUE_CTX,
    ),
    cmd(
        "o",
        "folder",
        "Set the default download folder",
        KeyAction::FolderPrompt,
        QUEUE_CTX,
    ),
    cmd("?", "help", "Show this help", KeyAction::Help, QUEUE_CTX),
    cmd("q", "quit", "Quit", KeyAction::Quit, QUEUE_CTX),
    // Detail view.
    cmd(
        "d",
        "download",
        "Queue a download",
        KeyAction::Download,
        DETAIL_CTX,
    ),
    cmd(
        "y",
        "copy",
        "Copy the magnet link",
        KeyAction::CopyMagnet,
        DETAIL_CTX,
    ),
    cmd(
        "esc",
        "back",
        "Leave the detail view",
        KeyAction::Escape,
        DETAIL_CTX,
    ),
];

/// Which documented context the app is in. The table documents three:
/// the results list, the queue, and the detail view.
pub fn help_context(mode: Mode, section: Section) -> Context {
    if section == Section::Downloads {
        QUEUE_CTX
    } else if mode == Mode::Detail {
        DETAIL_CTX
    } else {
        LIST_CTX
    }
}

/// The bindings the footer should advertise in `ctx`, in table order.
pub fn footer_hints(ctx: Context) -> impl Iterator<Item = &'static Help> {
    HELP.iter()
        .filter(move |h| h.in_footer && h.ctx.mode == ctx.mode && h.ctx.section == ctx.section)
}

/// Turn a help card key string back into the event it names, so the table
/// can be checked against the mapper it documents.
pub fn parse_help_key(key: &str) -> Option<KeyEvent> {
    let (code, modifiers) = match key {
        "↑" => (KeyCode::Up, KeyModifiers::NONE),
        "↓" => (KeyCode::Down, KeyModifiers::NONE),
        "←" => (KeyCode::Left, KeyModifiers::NONE),
        "→" => (KeyCode::Right, KeyModifiers::NONE),
        "⇥" => (KeyCode::Tab, KeyModifiers::NONE),
        "↵" => (KeyCode::Enter, KeyModifiers::NONE),
        "esc" => (KeyCode::Esc, KeyModifiers::NONE),
        "^c" => (KeyCode::Char('c'), KeyModifiers::CONTROL),
        other => {
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let modifiers = if c.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            (KeyCode::Char(c), modifiers)
        }
    };
    Some(KeyEvent::new(code, modifiers))
}

/// Resolves one key press to the action it means in the current UI context.
/// Returns `None` for anything unbound — never a catch-all default.
pub fn map_key(key: KeyEvent, ctx: Context) -> Option<KeyAction> {
    // Windows delivers a `Release` event for every `Press`; without this
    // filter every bound key fires twice there. Unix backends only ever
    // report `Press`, so this is a no-op elsewhere.
    if key.kind != KeyEventKind::Press {
        return None;
    }

    // There must be no state a user can get stuck in.
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
        return Some(KeyAction::Quit);
    }

    if is_editing(ctx) {
        return editing_key(key);
    }

    if ctx.overlay == Overlay::Help {
        return help_overlay_key(key);
    }

    browsing_key(key, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn list() -> Context {
        Context {
            mode: Mode::Normal,
            overlay: Overlay::None,
            region: Region::Content,
            section: Section::All,
        }
    }

    fn editing() -> Context {
        Context {
            mode: Mode::Search,
            ..list()
        }
    }

    #[test]
    fn every_help_entry_names_a_key_that_really_does_what_it_claims() {
        // The help card is the one surface that must not lie. Deriving it
        // from a table is not enough on its own — this test is what catches
        // a binding renamed in map_key while the table kept the old key.
        for entry in HELP {
            let event = parse_help_key(entry.key)
                .unwrap_or_else(|| panic!("help key {} is unparseable", entry.key));
            assert_eq!(
                map_key(event, entry.ctx),
                Some(entry.action.clone()),
                "help says {} = {}, but map_key disagrees",
                entry.key,
                entry.description
            );
        }
    }

    #[test]
    fn the_footer_advertises_only_keys_bound_in_that_context() {
        for ctx in [LIST_CTX, QUEUE_CTX, DETAIL_CTX] {
            let hints: Vec<&str> = footer_hints(ctx).map(|h| h.key).collect();
            assert!(!hints.is_empty(), "{ctx:?} has no footer hints");
        }
        let queue: Vec<&str> = footer_hints(QUEUE_CTX).map(|h| h.key).collect();
        assert!(queue.contains(&"c") && queue.contains(&"C"));
        assert!(
            !queue.contains(&"d"),
            "the queue footer must not advertise the download key"
        );
    }

    #[test]
    fn ctrl_c_quits_from_every_context() {
        // There must be no state the user can get stuck in.
        for ctx in [
            list(),
            editing(),
            Context {
                overlay: Overlay::Help,
                ..list()
            },
            Context {
                region: Region::Sidebar,
                ..list()
            },
            Context {
                section: Section::Downloads,
                ..list()
            },
        ] {
            assert_eq!(map_key(ctrl('c'), ctx), Some(KeyAction::Quit));
        }
    }

    #[test]
    fn letters_act_as_commands_in_a_list_and_as_text_while_editing() {
        // The bug this pins: typing "download" into the search box must not
        // fire the download, sort, and filter commands.
        assert_eq!(
            map_key(key(KeyCode::Char('d')), list()),
            Some(KeyAction::Download)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('d')), editing()),
            Some(KeyAction::Insert("d".into()))
        );
        assert_eq!(
            map_key(key(KeyCode::Char('s')), list()),
            Some(KeyAction::CycleSort)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('q')), editing()),
            Some(KeyAction::Insert("q".into()))
        );
    }

    #[test]
    fn the_results_bindings_are_exactly_the_agreed_set() {
        let expected = [
            (KeyCode::Char('/'), KeyAction::EditSearch),
            (KeyCode::Char('f'), KeyAction::EditFilter),
            (KeyCode::Char('s'), KeyAction::CycleSort),
            (KeyCode::Char('z'), KeyAction::ToggleHideDead),
            (KeyCode::Char('d'), KeyAction::Download),
            (KeyCode::Char('D'), KeyAction::DownloadTo),
            (KeyCode::Char('y'), KeyAction::CopyMagnet),
            (KeyCode::Char('?'), KeyAction::Help),
            (KeyCode::Char('o'), KeyAction::FolderPrompt),
            (KeyCode::Char('q'), KeyAction::Quit),
            (KeyCode::Char('k'), KeyAction::Up),
            (KeyCode::Char('j'), KeyAction::Down),
            (KeyCode::Char('h'), KeyAction::Left),
            (KeyCode::Char('l'), KeyAction::Right),
        ];
        for (code, action) in expected {
            let modifiers = if code == KeyCode::Char('D') {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            assert_eq!(
                map_key(KeyEvent::new(code, modifiers), list()),
                Some(action),
                "{code:?}"
            );
        }
    }

    #[test]
    fn destructive_download_keys_are_not_bound_in_the_downloads_section() {
        // `d` in Downloads would re-add an entry that is already there.
        let ctx = Context {
            section: Section::Downloads,
            ..list()
        };
        assert_eq!(map_key(key(KeyCode::Char('d')), ctx), None);
        assert_eq!(
            map_key(key(KeyCode::Char('c')), ctx),
            Some(KeyAction::RemoveEntry)
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT), ctx),
            Some(KeyAction::ClearQueue)
        );
    }

    #[test]
    fn tab_leaves_the_field_instead_of_typing_into_it() {
        // The splash hint promises "⇥ browse"; without this binding the key
        // is dead and the hint is a lie.
        assert_eq!(map_key(key(KeyCode::Tab), editing()), Some(KeyAction::Tab));
    }

    #[test]
    fn readline_editing_keys_map_while_editing() {
        assert_eq!(map_key(ctrl('a'), editing()), Some(KeyAction::Home));
        assert_eq!(map_key(ctrl('e'), editing()), Some(KeyAction::End));
        assert_eq!(map_key(ctrl('u'), editing()), Some(KeyAction::ClearField));
        assert_eq!(map_key(ctrl('k'), editing()), Some(KeyAction::KillToEnd));
        assert_eq!(
            map_key(ctrl('w'), editing()),
            Some(KeyAction::DeleteWordBefore)
        );
        assert_eq!(
            map_key(
                KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
                editing()
            ),
            Some(KeyAction::WordLeft)
        );
    }

    #[test]
    fn an_unbound_key_maps_to_nothing_rather_than_a_default_action() {
        assert_eq!(map_key(key(KeyCode::F(7)), list()), None);
        assert_eq!(map_key(ctrl('z'), list()), None);
    }

    #[test]
    fn control_characters_never_become_text() {
        // A stray control byte inserted into the search field would be sent
        // to a source as part of the query.
        assert_eq!(map_key(ctrl('z'), editing()), None);
        assert_eq!(map_key(key(KeyCode::F(1)), editing()), None);
    }

    #[test]
    fn release_events_are_ignored_so_windows_does_not_double_fire() {
        let mut release = key(KeyCode::Char('q'));
        release.kind = KeyEventKind::Release;
        assert_eq!(map_key(release, list()), None);

        let mut release_ctrl_c = ctrl('c');
        release_ctrl_c.kind = KeyEventKind::Release;
        assert_eq!(map_key(release_ctrl_c, list()), None);
    }

    #[test]
    fn overlays_that_edit_text_use_the_readline_bindings() {
        for overlay in [Overlay::FolderPrompt, Overlay::DownloadTo] {
            let ctx = Context {
                mode: Mode::Normal,
                overlay,
                ..list()
            };
            assert_eq!(
                map_key(key(KeyCode::Char('d')), ctx),
                Some(KeyAction::Insert("d".into()))
            );
            assert_eq!(map_key(key(KeyCode::Enter), ctx), Some(KeyAction::Enter));
            assert_eq!(map_key(key(KeyCode::Esc), ctx), Some(KeyAction::Escape));
        }
    }

    #[test]
    fn the_help_overlay_only_binds_dismissal_keys() {
        let ctx = Context {
            overlay: Overlay::Help,
            ..list()
        };
        assert_eq!(map_key(key(KeyCode::Esc), ctx), Some(KeyAction::Escape));
        assert_eq!(map_key(key(KeyCode::Char('?')), ctx), Some(KeyAction::Help));
        assert_eq!(map_key(key(KeyCode::Char('d')), ctx), None);
    }

    #[test]
    fn detail_mode_is_not_a_text_field() {
        let ctx = Context {
            mode: Mode::Detail,
            ..list()
        };
        assert_eq!(
            map_key(key(KeyCode::Char('d')), ctx),
            Some(KeyAction::Download),
            "a plain character in the detail view is a command, not text"
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT), ctx),
            Some(KeyAction::DownloadTo)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('y')), ctx),
            Some(KeyAction::CopyMagnet)
        );
        assert_eq!(map_key(key(KeyCode::Esc), ctx), Some(KeyAction::Escape));
        assert_eq!(
            map_key(key(KeyCode::Enter), ctx),
            None,
            "the detail view is already the opened row; the App handles no Enter there"
        );
        assert_eq!(map_key(ctrl('c'), ctx), Some(KeyAction::Quit));
    }

    #[test]
    fn start_and_pause_are_each_reachable_from_a_real_key_in_the_downloads_section() {
        // A binding the app handles but no key produces is exactly the kind
        // of dead-but-looks-alive key this suite guards against.
        let ctx = Context {
            section: Section::Downloads,
            ..list()
        };
        assert_eq!(
            map_key(key(KeyCode::Char('s')), ctx),
            Some(KeyAction::StartDownload)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('p')), ctx),
            Some(KeyAction::PauseDownload)
        );
    }

    #[test]
    fn esc_is_reachable_in_plain_browsing_not_only_in_overlays_and_detail() {
        // This binding was missing entirely once: the App handled Escape but
        // no key produced it, so the whole region walk-back was dead from the
        // keyboard while every test still passed. The app-side tests
        // construct KeyAction::Escape directly and would not catch it, so
        // this one goes through map_key deliberately.
        let ctx = list();
        assert_eq!(map_key(key(KeyCode::Esc), ctx), Some(KeyAction::Escape));

        let sidebar = Context {
            region: Region::Sidebar,
            ..ctx
        };
        assert_eq!(map_key(key(KeyCode::Esc), sidebar), Some(KeyAction::Escape));
    }
}
