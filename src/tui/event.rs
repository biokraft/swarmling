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
        (KeyModifiers::NONE, KeyCode::Enter) => Some(KeyAction::Enter),
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
        assert_eq!(map_key(key(KeyCode::Enter), ctx), Some(KeyAction::Enter));
        assert_eq!(map_key(ctrl('c'), ctx), Some(KeyAction::Quit));
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
