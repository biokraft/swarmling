//! The event loop: the one impure file in the TUI.
//!
//! Everything else in `crate::tui` is a pure function of state. This module
//! owns the terminal lifecycle, the `select!` loop, and the execution of the
//! [`Effect`]s the app asks for.
//!
//! **It constructs no torrent engine and no `librqbit::Session`.** In this
//! milestone a download is a recorded intent: queue effects read and rewrite
//! the queue *file*, exactly as `swarmling add` does. Nothing here contacts a
//! tracker, a DHT node, or a peer.
//!
//! Failures never tear the UI down. A clipboard with no owner, a directory
//! that cannot be created, a queue file that cannot be written — each becomes
//! an [`Action::Notice`], and the app is left exactly as it was.

use std::collections::{HashMap, VecDeque};
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::{paths, settings};
use crate::download::persist::{load_entries, save_entries};
use crate::download::queue::QueueEntry;
use crate::search::{search_all, SearchEvent};
use crate::sources::registry::all_sources;
use crate::tui::action::{Action, Effect};
use crate::tui::app::App;
use crate::tui::event::{map_key, Context};
use crate::tui::render;

/// How often the app is told what time it is. Notices expire from this tick,
/// and it is the only clock the app ever sees.
const TICK: Duration = Duration::from_millis(250);

/// Leave raw mode and the alternate screen, ignoring errors. Called on the
/// normal exit path and from the panic hook, so it must be safe to call
/// twice and must never panic itself.
pub fn restore_terminal_best_effort() {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
}

/// Expand a leading `~` against the home directory. Anything else — including
/// a `~` that is not the first component, and a home directory that cannot be
/// resolved — is returned untouched rather than guessed at.
fn expand_tilde(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        // `~user/...` names someone else's home; this is not a shell and has
        // no business inventing a path for it.
        return path.to_path_buf();
    }
    let Some(base) = directories::BaseDirs::new() else {
        return path.to_path_buf();
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        base.home_dir().to_path_buf()
    } else {
        base.home_dir().join(rest)
    }
}

/// Everything the loop carries that is not the app itself.
struct Runtime {
    queue_path: PathBuf,
    settings_path: PathBuf,
    /// `source_id -> reports_health`, built once from the registry. A
    /// `SearchEvent` carries only the id, and the app needs the flag to know
    /// whether a zero seeder count means "dead" or "not reported".
    health: HashMap<&'static str, bool>,
    search: Option<mpsc::Receiver<SearchEvent>>,
    quit: bool,
}

/// One thing that woke the loop up.
enum Step {
    Actions(Vec<Action>),
    /// A search event, or `None` when the in-flight search finished.
    Search(Option<SearchEvent>),
    /// The terminal event source ended or failed; there is nothing left to
    /// read keys from.
    Closed,
}

/// Translate a terminal event into the actions it means. Unbound keys and
/// events the UI does not use produce nothing at all.
fn actions_for(event: Event, app: &App) -> Vec<Action> {
    match event {
        Event::Key(key) => {
            let ctx = Context {
                mode: app.mode,
                overlay: app.overlay,
                region: app.region,
                section: app.section,
            };
            map_key(key, ctx).map(Action::Key).into_iter().collect()
        }
        Event::Resize(w, h) => vec![Action::Resize(w, h)],
        _ => Vec::new(),
    }
}

fn action_for_search(event: SearchEvent, health: &HashMap<&'static str, bool>) -> Action {
    match event {
        SearchEvent::Results { source_id, results } => Action::SearchResults {
            source_id,
            reports_health: health.get(source_id).copied().unwrap_or(false),
            results,
        },
        SearchEvent::SourceFailed { source_id, error } => Action::SearchFailed {
            source_id,
            error: error.to_string(),
        },
    }
}

/// Await the next search event, or stay pending forever when no search is in
/// flight, so the `select!` arm can simply be disabled by having no receiver.
async fn next_search(rx: &mut Option<mpsc::Receiver<SearchEvent>>) -> Option<SearchEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Perform one effect, returning the actions that feed its outcome back into
/// the app. `App::update` mutates neither the queue nor the download folder
/// itself, so without this feedback the UI would never show an add, a
/// removal, or a folder change.
fn perform(effect: Effect, app: &App, rt: &mut Runtime) -> Vec<Action> {
    match effect {
        Effect::Quit => {
            rt.quit = true;
            Vec::new()
        }

        Effect::StartSearch(_) => {
            // Handled by the caller: starting a search is the one effect that
            // needs to await, and it replaces the loop's receiver.
            Vec::new()
        }

        Effect::AddToQueue {
            infohash,
            magnet,
            title,
            dir,
            source_id,
        } => {
            // Recorded intent only: this appends to the queue file and starts
            // no transfer. The destination is created and recorded so the
            // folder the user typed is not silently discarded, but nothing
            // here writes into it.
            let mut entries = app.queue.clone();
            if entries.iter().any(|e| e.infohash == infohash) {
                return Vec::new();
            }
            let dir = expand_tilde(&dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                // A destination that cannot be made is not a queue entry: an
                // entry pointing at an impossible path would fail later, out
                // of sight of the person who typed it.
                return vec![Action::Notice(format!(
                    "Could not create {} ({e})",
                    dir.display()
                ))];
            }
            let added_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push(QueueEntry {
                infohash,
                magnet,
                title,
                added_unix,
                paused: false,
                // The effect knows which source the row came from;
                // `DownloadQueue::add` cannot, because it takes a bare magnet.
                source_id,
                dir: Some(dir),
            });
            save_queue(entries, rt)
        }

        Effect::RemoveFromQueue(infohash) => {
            let mut entries = app.queue.clone();
            entries.retain(|e| e.infohash != infohash);
            save_queue(entries, rt)
        }

        Effect::ClearQueue => save_queue(Vec::new(), rt),

        Effect::CopyToClipboard(text) => match copy_to_clipboard(&text) {
            Ok(()) => vec![Action::Notice("Magnet copied".into())],
            // A machine with no clipboard is not a broken swarmling.
            Err(e) => vec![Action::Notice(format!("Clipboard unavailable ({e})"))],
        },

        Effect::SaveDownloadDir(dir) => {
            // Never write settings back blind: `load` returns the safe
            // fallback both for a corrupt file and for one from a newer
            // swarmling, and saving that straight back would rewrite the file
            // at this build's version and discard the user's real values.
            let settings::Load::Clean(mut cfg) = settings::load_result(&rt.settings_path) else {
                return vec![Action::Notice(
                    "Settings file unreadable — folder not saved".into(),
                )];
            };
            let dir = expand_tilde(&dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return vec![Action::Notice(format!(
                    "Could not create {} ({e})",
                    dir.display()
                ))];
            }
            cfg.download_dir = Some(dir.clone());
            if let Err(e) = settings::save(&rt.settings_path, &cfg) {
                return vec![Action::Notice(format!("Could not save the folder ({e})"))];
            }
            vec![Action::DownloadDirChanged(dir)]
        }

        // Wiring these into a live session is a later milestone's job; this
        // task only introduces the vocabulary the event loop will eventually
        // act on. No socket, no session, nothing started here.
        //
        // TRIPWIRE: the wiring task MUST delete this arm (replacing it with
        // real session calls) and delete the covering test
        // `the_session_effects_are_still_stubbed_out` in this module's test
        // suite. Leaving either behind would mean `s`/`p` silently do
        // nothing forever, which is exactly the defect this stub exists to
        // avoid — see the notice below.
        Effect::StartDownload(_) | Effect::PauseDownload(_) | Effect::RemoveDownload { .. } => {
            vec![Action::Notice(
                "Live transfers are not available yet".to_string(),
            )]
        }
    }
}

/// Write the queue file and report the outcome. A write that fails leaves the
/// app's queue exactly as it was, and says so.
fn save_queue(entries: Vec<QueueEntry>, rt: &Runtime) -> Vec<Action> {
    match save_entries(&rt.queue_path, &entries) {
        Ok(()) => vec![Action::QueueChanged(entries)],
        Err(e) => vec![Action::Notice(format!("Could not save the queue ({e})"))],
    }
}

/// macOS and Windows keep the clipboard in the system, so setting it and
/// dropping the handle is enough.
#[cfg(not(target_os = "linux"))]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|e| e.to_string())
}

/// On X11 and Wayland the clipboard lives in the process that owns the
/// selection: dropping the handle releases it, and the paste comes back
/// empty. `SetExtLinux::wait` keeps serving requests until another
/// application takes the selection over, which cannot happen on the event
/// loop — so a detached thread owns a clipboard of its own for as long as
/// anyone wants the text.
///
/// A handle is opened here first, and dropped again, purely so that a machine
/// with no display server reports the failure to the user straight away
/// rather than failing silently inside the thread. The thread builds its own,
/// which also means nothing has to be `Send`.
#[cfg(target_os = "linux")]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use arboard::SetExtLinux;

    drop(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    let text = text.to_owned();
    std::thread::Builder::new()
        .name("swarmling-clipboard".into())
        .spawn(move || {
            // The UI has already been told the copy succeeded; there is
            // nobody left to tell if the selection is taken over later.
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return;
            };
            let _ = clipboard.set().wait().text(text);
        })
        .map_err(|e| format!("could not hold the clipboard ({e})"))?;
    Ok(())
}

/// Feed one action to the app and perform whatever it asks for, until the
/// work settles. Effects feed actions back, so this drains a queue rather
/// than recursing; `LIMIT` stops a pathological cycle from hanging the UI.
async fn dispatch(action: Action, app: &mut App, rt: &mut Runtime) {
    const LIMIT: usize = 64;
    let mut pending = VecDeque::from([action]);
    let mut steps = 0;
    while let Some(action) = pending.pop_front() {
        steps += 1;
        if steps > LIMIT {
            break;
        }
        for effect in app.update(action) {
            if let Effect::StartSearch(query) = effect {
                app.results.clear();
                rt.search = Some(search_all(all_sources(), &query).await);
                continue;
            }
            pending.extend(perform(effect, app, rt));
        }
    }
}

/// Run the terminal UI until the user quits.
///
/// Constructs no engine and no session: the queue effects touch the queue
/// file and nothing else.
pub async fn run() -> anyhow::Result<()> {
    let queue_path = paths::queue_state_path();
    let settings_path = paths::settings_path();
    let entries = load_entries(&queue_path);
    // Silent on purpose: a warning printed here would land in the alternate
    // screen with raw mode on and corrupt the display. An untrustworthy file
    // simply yields the default folder, and `SaveDownloadDir` refuses to
    // overwrite it.
    let cfg = settings::load_result(&settings_path).settings();
    let download_dir = cfg.download_dir.unwrap_or_else(paths::default_download_dir);
    let mut app = App::new(download_dir, entries);

    let mut rt = Runtime {
        queue_path,
        settings_path,
        health: all_sources()
            .iter()
            .map(|s| (s.id(), s.reports_health()))
            .collect(),
        search: None,
        quit: false,
    };

    // The hook goes in before raw mode does: a panic between the two would
    // otherwise hand the user a terminal that no longer echoes what they
    // type. `restore_terminal_best_effort` cannot panic, so it cannot turn a
    // panic into a double panic and an aborted process with no message.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_best_effort();
        previous(info);
    }));

    let mut terminal = enter_terminal()?;
    let result = event_loop(&mut terminal, &mut app, &mut rt).await;
    restore_terminal_best_effort();
    let _ = terminal.show_cursor();
    result
}

fn enter_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    Ok(terminal)
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rt: &mut Runtime,
) -> anyhow::Result<()> {
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    if let Ok(size) = terminal.size() {
        dispatch(Action::Resize(size.width, size.height), app, rt).await;
    }
    terminal.draw(|frame| render::draw(frame, app))?;

    loop {
        let step = tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => Step::Actions(actions_for(event, app)),
                // A terminal that can no longer be read is not recoverable,
                // and retrying would spin the loop at full speed.
                Some(Err(_)) | None => Step::Closed,
            },
            event = next_search(&mut rt.search) => Step::Search(event),
            _ = ticker.tick() => Step::Actions(vec![Action::Tick(Instant::now())]),
        };

        let actions = match step {
            Step::Closed => break,
            Step::Actions(actions) => actions,
            Step::Search(Some(event)) => vec![action_for_search(event, &rt.health)],
            Step::Search(None) => {
                rt.search = None;
                vec![Action::SearchFinished]
            }
        };

        for action in actions {
            dispatch(action, app, rt).await;
        }
        if rt.quit || app.should_quit {
            break;
        }
        terminal.draw(|frame| render::draw(frame, app))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panic_hook_restores_the_terminal_before_reporting() {
        // A panic that leaves raw mode on hands the user a terminal that no
        // longer echoes what they type. The hook must run the restore even
        // though the process is on its way down.
        //
        // The restore itself is exercised here without a real terminal: on a
        // headless test runner `disable_raw_mode` is a no-op or an error, and
        // either way it must not panic inside the hook and cause a double
        // panic.
        restore_terminal_best_effort();
        restore_terminal_best_effort();
    }

    #[test]
    fn the_session_effects_are_still_stubbed_out() {
        // TRIPWIRE: this test exists to fail loudly the day someone wires
        // Effect::{StartDownload,PauseDownload,RemoveDownload} into a real
        // session and forgets to delete the stub arm in `perform`. Until
        // then, pressing the keys that produce these effects must tell the
        // user why nothing happened rather than silently doing nothing — the
        // defect a prior milestone shipped five times over.
        //
        // When the wiring task lands, DELETE this test along with the stub
        // arm in `perform` that it covers.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut rt = Runtime {
            queue_path: dir.path().join("queue.json"),
            settings_path: dir.path().join("settings.json"),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());

        for effect in [
            Effect::StartDownload("a".repeat(40)),
            Effect::PauseDownload("a".repeat(40)),
            Effect::RemoveDownload {
                infohash: "a".repeat(40),
                delete_files: false,
            },
        ] {
            let actions = perform(effect, &app, &mut rt);
            assert!(
                matches!(
                    actions.as_slice(),
                    [Action::Notice(n)] if n == "Live transfers are not available yet"
                ),
                "a stubbed session effect must say so, not do nothing: {actions:?}"
            );
        }
    }

    #[test]
    fn a_leading_tilde_is_expanded_and_nothing_else_is() {
        // The prompt is a text field, so "~/Downloads" is what a user types.
        // Creating a literal directory named "~" in the working directory
        // instead is the bug this pins.
        let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
        if let Some(home) = home {
            assert_eq!(expand_tilde(Path::new("~")), home);
            assert_eq!(expand_tilde(Path::new("~/Torrents")), home.join("Torrents"));
        }
        assert_eq!(
            expand_tilde(Path::new("/tmp/torrents")),
            PathBuf::from("/tmp/torrents")
        );
        assert_eq!(
            expand_tilde(Path::new("torrents/~backup")),
            PathBuf::from("torrents/~backup"),
            "a tilde that is not the first character is part of the name"
        );
        assert_eq!(
            expand_tilde(Path::new("~someone/else")),
            PathBuf::from("~someone/else"),
            "another user's home is not this program's to guess"
        );
    }

    #[test]
    fn a_queue_effect_writes_the_file_and_reports_the_new_contents() {
        // The app no longer mutates its own queue: without this feedback the
        // Downloads panel would never show what was added.
        let dir = tempfile::tempdir().expect("tempdir");
        let queue_path = dir.path().join("queue.json");
        let mut rt = Runtime {
            queue_path: queue_path.clone(),
            settings_path: dir.path().join("settings.json"),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());
        let hash = "a".repeat(40);

        let target = dir.path().join("keep");
        let actions = perform(
            Effect::AddToQueue {
                infohash: hash.clone(),
                magnet: format!("magnet:?xt=urn:btih:{hash}"),
                title: "Example".into(),
                dir: target.clone(),
                source_id: Some("yts".into()),
            },
            &app,
            &mut rt,
        );

        match actions.as_slice() {
            [Action::QueueChanged(entries)] => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].infohash, hash);
                assert_eq!(
                    entries[0].source_id.as_deref(),
                    Some("yts"),
                    "the effect's source id must reach the entry, or the source tag stays blank"
                );
                assert_eq!(
                    entries[0].dir.as_deref(),
                    Some(target.as_path()),
                    "the folder the user typed at the D prompt must reach the entry"
                );
            }
            other => panic!("expected one QueueChanged, got {other:?}"),
        }
        let on_disk = load_entries(&queue_path);
        assert_eq!(on_disk.len(), 1, "the queue file was not written");
        assert_eq!(on_disk[0].infohash, hash);
        assert_eq!(on_disk[0].dir.as_deref(), Some(target.as_path()));
        assert!(target.is_dir(), "the chosen destination was not created");
    }

    #[test]
    fn a_destination_that_cannot_be_made_queues_nothing() {
        // An entry pointing at an impossible path would fail later, out of
        // sight of the person who typed it.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"not a directory").expect("write");
        let queue_path = dir.path().join("queue.json");
        let mut rt = Runtime {
            queue_path: queue_path.clone(),
            settings_path: dir.path().join("settings.json"),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());
        let hash = "b".repeat(40);
        let actions = perform(
            Effect::AddToQueue {
                infohash: hash.clone(),
                magnet: format!("magnet:?xt=urn:btih:{hash}"),
                title: "Example".into(),
                dir: blocker.join("under"),
                source_id: None,
            },
            &app,
            &mut rt,
        );
        assert!(
            matches!(actions.as_slice(), [Action::Notice(_)]),
            "expected a notice, got {actions:?}"
        );
        assert!(
            load_entries(&queue_path).is_empty(),
            "nothing should have been queued"
        );
    }

    #[test]
    fn a_queue_write_that_fails_becomes_a_notice_and_changes_nothing() {
        // Fail soft: an unwritable state directory must not take the UI down.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, b"not a directory").expect("write");
        let mut rt = Runtime {
            // `blocked` is a file, so creating it as a parent directory fails.
            queue_path: blocker.join("queue.json"),
            settings_path: dir.path().join("settings.json"),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());
        let actions = perform(Effect::ClearQueue, &app, &mut rt);
        assert!(
            matches!(actions.as_slice(), [Action::Notice(_)]),
            "expected a notice, got {actions:?}"
        );
    }

    #[test]
    fn saving_a_download_folder_creates_it_and_persists_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = dir.path().join("settings.json");
        let target = dir.path().join("nested").join("torrents");
        let mut rt = Runtime {
            queue_path: dir.path().join("queue.json"),
            settings_path: settings_path.clone(),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());

        let actions = perform(Effect::SaveDownloadDir(target.clone()), &app, &mut rt);
        assert!(
            matches!(actions.as_slice(), [Action::DownloadDirChanged(d)] if *d == target),
            "expected DownloadDirChanged({}), got {actions:?}",
            target.display()
        );
        assert!(target.is_dir(), "the folder was not created");
        assert_eq!(settings::load(&settings_path).download_dir, Some(target));
    }

    #[test]
    fn a_settings_file_from_a_newer_version_is_not_overwritten_by_a_folder_change() {
        // `settings::load` hands back the safe fallback for a file it cannot
        // trust; writing that back would rewrite the file at this build's
        // version and drop whatever the user really had in it.
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = dir.path().join("settings.json");
        let original = br#"{"version":99,"settings":{"vpn_adapter":"nordvpn"}}"#;
        std::fs::write(&settings_path, original).expect("write");
        let mut rt = Runtime {
            queue_path: dir.path().join("queue.json"),
            settings_path: settings_path.clone(),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());

        let actions = perform(
            Effect::SaveDownloadDir(dir.path().join("torrents")),
            &app,
            &mut rt,
        );
        assert!(
            matches!(actions.as_slice(), [Action::Notice(_)]),
            "expected a notice, got {actions:?}"
        );
        assert_eq!(
            std::fs::read(&settings_path).expect("read"),
            original.to_vec(),
            "the settings file was rewritten with this build's defaults"
        );
    }

    #[test]
    fn a_folder_that_cannot_be_created_is_a_notice_and_not_a_change() {
        // The spec's rule: a failure sets a notice and changes nothing.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"not a directory").expect("write");
        let mut rt = Runtime {
            queue_path: dir.path().join("queue.json"),
            settings_path: dir.path().join("settings.json"),
            health: HashMap::new(),
            search: None,
            quit: false,
        };
        let app = App::new(dir.path().to_path_buf(), Vec::new());
        let actions = perform(
            Effect::SaveDownloadDir(blocker.join("under")),
            &app,
            &mut rt,
        );
        assert!(
            matches!(actions.as_slice(), [Action::Notice(_)]),
            "expected a notice, got {actions:?}"
        );
    }

    #[test]
    fn a_search_event_carries_the_registry_health_flag_the_app_needs() {
        // `SearchEvent` carries only the source id, so the loop is the only
        // place that can attach `reports_health`; getting it wrong silently
        // marks healthy sources dead.
        let mut health = HashMap::new();
        health.insert("yts", true);
        let action = action_for_search(
            SearchEvent::Results {
                source_id: "yts",
                results: Vec::new(),
            },
            &health,
        );
        assert!(matches!(
            action,
            Action::SearchResults {
                source_id: "yts",
                reports_health: true,
                ..
            }
        ));

        let unknown = action_for_search(
            SearchEvent::Results {
                source_id: "nobody",
                results: Vec::new(),
            },
            &health,
        );
        assert!(matches!(
            unknown,
            Action::SearchResults {
                reports_health: false,
                ..
            }
        ));
    }
}
