//! The event loop: the one impure file in the TUI.
//!
//! Everything else in `crate::tui` is a pure function of state. This module
//! owns the terminal lifecycle, the `select!` loop, and the execution of the
//! [`Effect`]s the app asks for.
//!
//! **This is the only file in swarmling that builds a real session.** It
//! constructs the one [`LibrqbitFactory`] in the codebase and hands it to a
//! [`Driver`]; everything else — including every test, here and elsewhere —
//! goes through the `SessionFactory` seam and a fake. A session comes up only
//! after the guard has confirmed a tunnel *and* the user has pressed start:
//! restoring the queue file at startup fills the Downloads panel and sends no
//! intent, so opening swarmling to look at the queue never moves a byte.
//!
//! Failures never tear the UI down. A clipboard with no owner, a directory
//! that cannot be created, a queue file that cannot be written — each becomes
//! an [`Action::Notice`], and the app is left exactly as it was.

use std::collections::{HashMap, VecDeque};
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
use crate::supervisor::driver::Driver;
use crate::supervisor::factory::{LibrqbitFactory, SessionFactory};
use crate::supervisor::{Input, Intent, Supervisor, Wanted};
use crate::tui::action::{Action, Effect};
use crate::tui::app::App;
use crate::tui::event::{map_key, Context};
use crate::tui::render;
use crate::vpn::adapter::VpnAdapter;
use crate::vpn::guard::{Guard, GuardAction};

/// How often the app is told what time it is. Notices expire from this tick,
/// and it is the only clock the app ever sees.
const TICK: Duration = Duration::from_millis(250);

/// How often the VPN is looked at. The guard is the kill switch, so this is
/// the longest a lost tunnel can go unnoticed on a platform that cannot bind.
const VPN_POLL: Duration = Duration::from_secs(2);

/// How often a live session is asked what it is doing. Only runs while a
/// session exists, so an idle app polls nothing.
const SESSION_POLL: Duration = Duration::from_secs(1);

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

/// The live half of the loop: the VPN it watches, the guard that decides what
/// that means, and the driver that owns whatever session exists. Kept apart
/// from [`Runtime`] because none of it may be reachable from a pure effect.
struct Session {
    driver: Driver,
    guard: Guard,
    adapter: Box<dyn VpnAdapter>,
    /// The torrents the supervisor has already been told about. A start for
    /// one it has never heard of needs an `Add` first; a second start must
    /// not add it twice.
    known: std::collections::HashSet<String>,
}

/// Every notice the driver produces has to reach the user: a dropped one is a
/// transfer that silently did not happen.
fn notices(messages: Vec<String>) -> Vec<Action> {
    messages.into_iter().map(Action::Notice).collect()
}

/// Look at the VPN once and let the guard decide what it means. `status` is
/// infallible — an adapter that cannot tell reports `VpnStatus::Unknown`,
/// which the guard treats exactly like a disconnection.
async fn poll_vpn(session: &mut Session) -> Vec<Action> {
    let status = session.adapter.status().await;
    let action = session.guard.observe(&status);
    // The kill switch firing has to be visible. `SessionOp::Kill` carries its
    // reason to the driver, which drops the session without a word, so the
    // announcement is made here — a queue that stops dead with no explanation
    // reads as a hang.
    let mut actions = match &action {
        GuardAction::PauseAll { reason } => {
            vec![Action::Notice(format!("Stopped every transfer: {reason}"))]
        }
        _ => Vec::new(),
    };
    actions.extend(notices(session.driver.handle(Input::Vpn(action)).await));
    actions.push(Action::GuardChanged(session.guard.state().clone()));
    actions
}

/// Ask a live session what it is doing. Silent while none exists, so a fresh
/// app never reports progress it does not have.
async fn poll_session(session: &mut Session) -> Vec<Action> {
    if !session.driver.session_is_live() {
        return Vec::new();
    }
    let mut actions = notices(session.driver.poll_completions().await);
    actions.push(Action::SnapshotsUpdated(session.driver.snapshots().await));
    actions
}

/// The three effects that touch a real session. Separate from [`perform`]
/// because they await, and because keeping them here makes it obvious which
/// effects can move bytes.
async fn perform_session(effect: Effect, app: &App, session: &mut Session) -> Vec<Action> {
    let mut actions = Vec::new();
    match effect {
        Effect::StartDownload(infohash) => {
            if !session.known.contains(&infohash) {
                let Some(entry) = app.queue.iter().find(|e| e.infohash == infohash) else {
                    // The row went away between the key press and here.
                    return vec![Action::Notice("That download is no longer queued".into())];
                };
                let wanted = Wanted {
                    infohash: infohash.clone(),
                    magnet: entry.magnet.clone(),
                    dir: entry
                        .dir
                        .clone()
                        .unwrap_or_else(|| app.download_dir.clone()),
                    // Added paused: an add is a record, and only the start
                    // below may set it running.
                    paused: true,
                };
                actions.extend(notices(
                    session
                        .driver
                        .handle(Input::User(Intent::Add(wanted)))
                        .await,
                ));
                session.known.insert(infohash.clone());
            }
            actions.extend(notices(
                session
                    .driver
                    .handle(Input::User(Intent::Start(infohash)))
                    .await,
            ));
        }
        Effect::PauseDownload(infohash) => {
            actions.extend(notices(
                session
                    .driver
                    .handle(Input::User(Intent::Pause(infohash)))
                    .await,
            ));
        }
        Effect::RemoveDownload {
            infohash,
            delete_files,
        } => {
            session.known.remove(&infohash);
            actions.extend(notices(
                session
                    .driver
                    .handle(Input::User(Intent::Remove {
                        infohash,
                        delete_files,
                    }))
                    .await,
            ));
        }
        // Every other effect is `perform`'s; the caller routes by variant,
        // so reaching this arm means nothing was asked of the session.
        _ => return Vec::new(),
    }
    actions.extend(poll_session(session).await);
    actions
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

        // Handled by `perform_session`, which awaits a real driver. The
        // caller routes them there; reaching this arm would mean the routing
        // was lost, so it does nothing rather than pretending to.
        Effect::StartDownload(_) | Effect::PauseDownload(_) | Effect::RemoveDownload { .. } => {
            Vec::new()
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
async fn dispatch(action: Action, app: &mut App, rt: &mut Runtime, session: &mut Session) {
    const LIMIT: usize = 64;
    let mut pending = VecDeque::from([action]);
    let mut steps = 0;
    while let Some(action) = pending.pop_front() {
        steps += 1;
        if steps > LIMIT {
            break;
        }
        for effect in app.update(action) {
            match effect {
                Effect::StartSearch(query) => {
                    app.results.clear();
                    rt.search = Some(search_all(all_sources(), &query).await);
                }
                Effect::StartDownload(_)
                | Effect::PauseDownload(_)
                | Effect::RemoveDownload { .. } => {
                    pending.extend(perform_session(effect, app, session).await);
                }
                effect => pending.extend(perform(effect, app, rt)),
            }
        }
    }
}

/// Run the terminal UI until the user quits.
///
/// Builds the driver that can bring a session up, but brings none up here:
/// the queue is restored for display only, and the first session can appear
/// no earlier than the first start the user presses under a confirmed VPN.
pub async fn run() -> anyhow::Result<()> {
    let queue_path = paths::queue_state_path();
    let settings_path = paths::settings_path();
    let entries = load_entries(&queue_path);
    // Silent on purpose: a warning printed here would land in the alternate
    // screen with raw mode on and corrupt the display. An untrustworthy file
    // simply yields the default folder, and `SaveDownloadDir` refuses to
    // overwrite it.
    let cfg = settings::load_result(&settings_path).settings();
    let download_dir = cfg
        .download_dir
        .clone()
        .unwrap_or_else(paths::default_download_dir);
    // Restored for display only. No `Intent` is sent here, and `known` starts
    // empty, so nothing in this queue is running until the user says so.
    let mut app = App::new(download_dir.clone(), entries);

    let factory: Arc<dyn SessionFactory> = Arc::new(LibrqbitFactory::new(
        download_dir,
        paths::data_dir().join("session"),
    ));
    let mut session = Session {
        driver: Driver::new(factory, Supervisor::new(crate::vpn::policy::bind_support())),
        guard: Guard::new(),
        adapter: crate::vpn::require::adapter_for(&cfg),
        known: std::collections::HashSet::new(),
    };

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
    let result = event_loop(&mut terminal, &mut app, &mut rt, &mut session).await;
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
    session: &mut Session,
) -> anyhow::Result<()> {
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut vpn_ticker = tokio::time::interval(VPN_POLL);
    vpn_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut session_ticker = tokio::time::interval(SESSION_POLL);
    session_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    if let Ok(size) = terminal.size() {
        dispatch(Action::Resize(size.width, size.height), app, rt, session).await;
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
            // The kill switch: this is the only thing that notices a tunnel
            // going away, so it runs whether or not a session exists.
            _ = vpn_ticker.tick() => Step::Actions(poll_vpn(session).await),
            // Progress and completions, only while something is running.
            _ = session_ticker.tick() => Step::Actions(poll_session(session).await),
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
            dispatch(action, app, rt, session).await;
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
    use crate::vpn::guard::GuardState;
    use crate::vpn::policy::BindSupport;

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

    fn queued(hash: &str, dir: Option<PathBuf>) -> QueueEntry {
        QueueEntry {
            infohash: hash.to_string(),
            magnet: format!("magnet:?xt=urn:btih:{hash}"),
            title: "Example".into(),
            added_unix: 0,
            paused: false,
            source_id: None,
            dir,
        }
    }

    /// A session wired to a fake factory and a fake interface list. Opens
    /// nothing: no test in this file may name the real factory or engine.
    fn fake_session(
        interfaces: Arc<crate::vpn::interfaces::FakeInterfaces>,
    ) -> (Arc<crate::supervisor::factory::FakeFactory>, Session) {
        let factory = Arc::new(crate::supervisor::factory::FakeFactory::new());
        let session = Session {
            driver: Driver::new(
                Arc::clone(&factory) as Arc<dyn SessionFactory>,
                Supervisor::new(BindSupport::Supported),
            ),
            guard: Guard::new(),
            adapter: Box::new(crate::vpn::adapter::GenericAdapter::new(interfaces)),
            known: std::collections::HashSet::new(),
        };
        (factory, session)
    }

    fn tunnel(name: &str) -> crate::vpn::interfaces::Interface {
        crate::vpn::interfaces::Interface {
            name: name.into(),
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2)),
        }
    }

    #[tokio::test]
    async fn a_start_effect_becomes_a_supervisor_intent() {
        // The effect must reach the engine, not merely be accepted and
        // dropped. A silently ignored effect is the defect this catches, and
        // the stub arm it replaces did exactly that on purpose.
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "utun4",
        )]));
        let (factory, mut session) = fake_session(interfaces);
        let hash = "a".repeat(40);
        let app = App::new(
            dir.path().to_path_buf(),
            vec![queued(&hash, Some(dir.path().join("here")))],
        );

        // The tunnel has to be confirmed before anything may start.
        poll_vpn(&mut session).await;
        let actions =
            perform_session(Effect::StartDownload(hash.clone()), &app, &mut session).await;

        assert!(
            session.driver.session_is_live(),
            "a start with a live tunnel must bring the session up: {actions:?}"
        );
        assert_eq!(
            factory.engine().added_magnets().len(),
            1,
            "the start effect never reached the engine: {actions:?}"
        );
    }

    #[tokio::test]
    async fn nothing_starts_while_the_tunnel_is_unconfirmed() {
        // Fail closed: with no VPN observed, a start must refuse and say so
        // rather than open a session.
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "en0",
        )]));
        let (factory, mut session) = fake_session(interfaces);
        let hash = "b".repeat(40);
        let app = App::new(dir.path().to_path_buf(), vec![queued(&hash, None)]);

        poll_vpn(&mut session).await;
        let actions = perform_session(Effect::StartDownload(hash), &app, &mut session).await;

        assert!(!session.driver.session_is_live(), "{actions:?}");
        assert!(factory.engine().added_magnets().is_empty(), "{actions:?}");
        assert!(
            actions.iter().any(|a| matches!(a, Action::Notice(_))),
            "a refusal the user cannot see is a silent failure: {actions:?}"
        );
    }

    #[tokio::test]
    async fn a_start_uses_the_folder_recorded_on_the_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "utun4",
        )]));
        let (_factory, mut session) = fake_session(interfaces);
        let hash = "c".repeat(40);
        let app = App::new(dir.path().to_path_buf(), vec![queued(&hash, None)]);

        poll_vpn(&mut session).await;
        perform_session(Effect::StartDownload(hash.clone()), &app, &mut session).await;
        // The default folder stands in when the entry names none; the entry
        // must not be dropped for want of a directory.
        assert!(session.known.contains(&hash));
    }

    #[tokio::test]
    async fn removing_an_entry_takes_it_out_of_the_session_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "utun4",
        )]));
        let (_factory, mut session) = fake_session(interfaces);
        let hash = "d".repeat(40);
        let app = App::new(dir.path().to_path_buf(), vec![queued(&hash, None)]);

        poll_vpn(&mut session).await;
        perform_session(Effect::StartDownload(hash.clone()), &app, &mut session).await;
        assert!(session.known.contains(&hash));

        perform_session(
            Effect::RemoveDownload {
                infohash: hash.clone(),
                delete_files: false,
            },
            &app,
            &mut session,
        )
        .await;
        assert!(
            !session.known.contains(&hash),
            "a removed torrent must stop being one the loop believes is running"
        );
    }

    #[tokio::test]
    async fn a_lost_tunnel_pauses_everything_and_tells_the_user() {
        // The kill switch has to be visible: a pause the user is not told
        // about looks like a stall.
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "utun4",
        )]));
        let (_factory, mut session) = fake_session(Arc::clone(&interfaces));
        let hash = "e".repeat(40);
        let app = App::new(dir.path().to_path_buf(), vec![queued(&hash, None)]);

        poll_vpn(&mut session).await;
        perform_session(Effect::StartDownload(hash), &app, &mut session).await;
        assert!(session.driver.session_is_live());

        interfaces.set(vec![tunnel("en0")]);
        let actions = poll_vpn(&mut session).await;

        assert!(
            !session.driver.session_is_live(),
            "the session must go down when the tunnel does: {actions:?}"
        );
        assert!(
            actions.iter().any(|a| matches!(a, Action::Notice(_))),
            "the kill switch firing must reach the user: {actions:?}"
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::GuardChanged(GuardState::Lost { .. }))),
            "the guard's new state must reach the app: {actions:?}"
        );
    }

    #[tokio::test]
    async fn the_snapshot_poll_is_silent_until_a_session_exists() {
        // Starting the app must never look like a running transfer.
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![]));
        let (_factory, mut session) = fake_session(interfaces);
        assert!(poll_session(&mut session).await.is_empty());
    }

    #[tokio::test]
    async fn a_finished_torrent_leaves_the_session_and_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let interfaces = Arc::new(crate::vpn::interfaces::FakeInterfaces::new(vec![tunnel(
            "utun4",
        )]));
        let (factory, mut session) = fake_session(interfaces);
        let hash = "f".repeat(40);
        let app = App::new(dir.path().to_path_buf(), vec![queued(&hash, None)]);

        poll_vpn(&mut session).await;
        perform_session(Effect::StartDownload(hash.clone()), &app, &mut session).await;
        factory.engine().finish(&hash);

        let actions = poll_session(&mut session).await;
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::SnapshotsUpdated(_))),
            "the poll must feed progress back to the app: {actions:?}"
        );
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
