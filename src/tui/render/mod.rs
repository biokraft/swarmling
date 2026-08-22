//! The renderers. Every function here is a pure function of `&App`: it
//! computes no state, mutates nothing, and performs no IO.
//!
//! `layout::regions` can legitimately hand back zero-width or zero-height
//! rects on a small terminal, so every function returns early on an empty
//! area. A panic in a renderer would leave the user's terminal in raw mode.

pub mod downloads;
pub mod overlay;
pub mod results;
pub mod shell;
pub mod splash;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::tui::app::{App, Screen};

/// Draw one frame. The overlay is drawn last so it sits on top.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    match app.screen {
        Screen::Splash => splash::draw(frame, area, app),
        Screen::Browser => shell::draw(frame, area, app),
    }
    overlay::draw(frame, area, app);
}

/// True when nothing can be drawn into `area`.
pub(crate) fn empty(area: Rect) -> bool {
    area.width == 0 || area.height == 0
}

/// Truncate to `width` columns, marking the cut with an ellipsis. Counts
/// characters, never bytes: a byte slice would panic on a multi-byte title.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Pad to `width` columns so columns line up. Never truncates.
pub(crate) fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    let mut out = text.to_owned();
    for _ in len..width {
        out.push(' ');
    }
    out
}

/// A binary size, as the search sources themselves render it.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// A UTC timestamp for a unix second count, rendered without pulling in a
/// date crate. Days are converted with Howard Hinnant's civil-from-days.
pub(crate) fn added_label(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs_of_day = unix % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The focused text field, rendered into `width` columns with a block
/// cursor. Shared by the splash search box and both prompts.
pub(crate) fn field_spans(app: &App, width: u16) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    let (view, cursor) = app.field.view(width);
    let chars: Vec<char> = view.chars().collect();
    let before: String = chars.iter().take(cursor).collect();
    let at: String = chars
        .get(cursor)
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_owned());
    let after: String = chars.iter().skip(cursor + 1).collect();
    vec![
        Span::styled(before, Style::default().fg(crate::tui::theme::TEXT)),
        Span::styled(
            at,
            Style::default()
                .fg(crate::tui::theme::TEXT)
                .add_modifier(Modifier::REVERSED),
        ),
        Span::styled(after, Style::default().fg(crate::tui::theme::TEXT)),
    ]
}

/// The wordmark as coloured lines. This is the only gradient in the app.
pub(crate) fn wordmark_lines() -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};

    crate::tui::wordmark::LINES
        .iter()
        .enumerate()
        .map(|(row, text)| {
            Line::from(
                text.chars()
                    .enumerate()
                    .map(|(col, ch)| {
                        Span::styled(
                            ch.to_string(),
                            Style::default().fg(crate::tui::wordmark::cell_color(row, col)),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::action::{Action, KeyAction};
    use crate::tui::app::Region;
    use ratatui::{backend::TestBackend, Terminal};

    fn render(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        terminal.draw(|f| draw(f, app)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn lines_of(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn text_of(buf: &ratatui::buffer::Buffer) -> String {
        // Flatten the buffer so assertions can look for a phrase without
        // caring which cell it starts in.
        lines_of(buf).join("\n")
    }

    fn app() -> App {
        App::new(std::path::PathBuf::from("/tmp/x"), Vec::new())
    }

    #[test]
    fn a_tiny_terminal_renders_without_panicking() {
        // Terminals get resized to absurd sizes mid-session. A panic here
        // leaves the user's terminal in raw mode.
        for (w, h) in [(1, 1), (2, 2), (10, 5), (20, 11), (40, 17), (80, 24)] {
            let _ = render(&app(), w, h);
        }
    }

    /// Sizes a terminal really gets resized to, degenerate ones included.
    const SIZES: [(u16, u16); 9] = [
        (1, 1),
        (2, 2),
        (3, 1),
        (10, 5),
        (20, 11),
        (40, 17),
        (80, 24),
        (120, 40),
        (200, 3),
    ];

    /// A row built to break a careless renderer: a multi-byte CJK title and
    /// the largest size a u64 can hold.
    fn hostile_result() -> crate::sources::SearchResult {
        crate::sources::SearchResult {
            title: "進撃の巨人 — 第四期 完結編 [1080p][HEVC][マルチ音声]".into(),
            magnet: "magnet:?xt=urn:btih:".to_owned() + &"cd".repeat(20),
            size_bytes: u64::MAX,
            seeders: u32::MAX,
            leechers: u32::MAX,
            source_id: "nyaa",
            infohash: "cd".repeat(20),
        }
    }

    fn hostile_entry() -> crate::download::queue::QueueEntry {
        crate::download::queue::QueueEntry {
            infohash: "cd".repeat(20),
            magnet: "magnet:?xt=urn:btih:".to_owned() + &"cd".repeat(20),
            title: "進撃の巨人 — 第四期 完結編".into(),
            added_unix: u64::MAX,
            paused: true,
            source_id: Some("nyaa".into()),
            dir: None,
        }
    }

    /// Every screen the app can be showing, each with hostile content.
    fn every_screen() -> Vec<(&'static str, App)> {
        let populated = || {
            let mut a = App::new(
                std::path::PathBuf::from("/tmp/x"),
                vec![hostile_entry(), hostile_entry()],
            );
            a.update(Action::Key(KeyAction::Enter));
            a.update(Action::SearchResults {
                source_id: "nyaa",
                reports_health: true,
                results: vec![hostile_result()],
            });
            a.update(Action::SearchFailed {
                source_id: "yts",
                error: "timed out".into(),
            });
            a.region = Region::Content;
            a
        };

        let mut screens: Vec<(&'static str, App)> = Vec::new();
        screens.push(("splash", app()));

        let mut splash_typed = app();
        splash_typed.update(Action::Key(KeyAction::Insert(
            "進撃の巨人 a very long query".into(),
        )));
        screens.push(("splash with a query", splash_typed));

        screens.push(("results", populated()));

        let mut downloads = populated();
        downloads.set_section(crate::tui::app::Section::Downloads);
        screens.push(("downloads", downloads));

        let mut empty_downloads = app();
        empty_downloads.update(Action::Key(KeyAction::Enter));
        empty_downloads.set_section(crate::tui::app::Section::Downloads);
        screens.push(("empty downloads", empty_downloads));

        let mut detail = populated();
        detail.update(Action::Key(KeyAction::Enter));
        screens.push(("detail", detail));

        let mut help = populated();
        help.update(Action::Key(KeyAction::Help));
        screens.push(("help overlay", help));

        let mut folder = populated();
        folder.update(Action::Key(KeyAction::FolderPrompt));
        screens.push(("folder prompt", folder));

        let mut download_to = populated();
        download_to.update(Action::Key(KeyAction::DownloadTo));
        screens.push(("download-to prompt", download_to));

        screens
    }

    #[test]
    fn every_screen_renders_at_every_size_without_panicking() {
        // Terminals get resized to absurd sizes mid-session, and a panic here
        // leaves the user's terminal in raw mode. Every screen is covered,
        // not only the ones that are easy to reach.
        for (_name, app) in every_screen() {
            for (w, h) in SIZES {
                let _ = render(&app, w, h);
            }
        }
    }

    #[test]
    fn every_screen_is_reachable_and_really_differs() {
        // Guards the sweep above: if a screen constructor silently failed to
        // change the app, the sweep would be testing the same screen twice.
        let screens = every_screen();
        assert_eq!(screens.len(), 9);
        let rendered: std::collections::HashSet<String> = screens
            .iter()
            .map(|(_, a)| text_of(&render(a, 100, 30)))
            .collect();
        assert_eq!(rendered.len(), 9, "two screens rendered identically");
    }

    #[test]
    fn the_splash_names_the_product_and_its_hints() {
        let buf = render(&app(), 80, 24);
        let text = text_of(&buf);
        assert!(text.contains("search"), "missing the splash hint: {text}");
    }

    #[test]
    fn a_narrow_terminal_falls_back_from_the_wordmark_to_plain_text() {
        let buf = render(&app(), 20, 24);
        let text = text_of(&buf);
        assert!(text.contains("swarmling"), "no product name at all: {text}");
    }

    #[test]
    fn the_browser_shows_every_sidebar_section() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        let text = text_of(&render(&a, 100, 30));
        for label in ["All", "Games", "Movies", "TV", "Anime", "Downloads"] {
            assert!(text.contains(label), "sidebar missing {label}: {text}");
        }
    }

    #[test]
    fn a_failed_source_is_visible_on_screen() {
        // The whole point of recording failures is that the user sees them.
        // A failure kept only in memory is the same as a silent one.
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::SearchFailed {
            source_id: "nyaa",
            error: "timed out".into(),
        });
        let text = text_of(&render(&a, 100, 30));
        assert!(
            text.contains("nyaa"),
            "the failed source is not shown: {text}"
        );
    }

    #[test]
    fn results_render_their_title_seeders_and_source_tag() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::SearchResults {
            source_id: "yts",
            reports_health: true,
            results: vec![crate::sources::SearchResult {
                title: "example release".into(),
                magnet: "magnet:?xt=urn:btih:".to_owned() + &"ab".repeat(20),
                size_bytes: 1024 * 1024 * 1024,
                seeders: 42,
                leechers: 3,
                source_id: "yts",
                infohash: "ab".repeat(20),
            }],
        });
        let text = text_of(&render(&a, 100, 30));
        assert!(text.contains("example release"));
        assert!(text.contains("42"));
        assert!(text.contains("YTS"));
    }

    #[test]
    fn the_help_overlay_lists_its_keys() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::Key(KeyAction::Help));
        let text = text_of(&render(&a, 100, 30));
        assert!(text.contains("Quit"), "help overlay is empty: {text}");
    }

    fn with_rows(count: u32) -> App {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::SearchResults {
            source_id: "yts",
            reports_health: true,
            results: (0..count)
                .map(|i| crate::sources::SearchResult {
                    title: format!("row {i}"),
                    magnet: format!("magnet:?xt=urn:btih:{:040x}", i),
                    size_bytes: 1,
                    seeders: 100 - i,
                    leechers: 0,
                    source_id: "yts",
                    infohash: format!("{:040x}", i),
                })
                .collect(),
        });
        a
    }

    #[test]
    fn the_help_card_names_the_real_filter_and_hide_dead_keys() {
        // These were documented as `/` and `h` while the mapper bound them
        // to `f` and `z`, so the card sent users to a navigation key.
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.update(Action::Key(KeyAction::Help));
        let text = text_of(&render(&a, 120, 40));
        assert!(text.contains("Filter the results"), "{text}");
        assert!(text.contains("Hide or show dead torrents"), "{text}");
        assert!(text.contains("Remove one queue entry"), "{text}");
        // The key column sits between the card's left border and the
        // description, so read it back out of the rendered line.
        let key_for = |description: &str| -> String {
            let line = text
                .lines()
                .find(|l| l.contains(description))
                .unwrap_or_else(|| panic!("{description} is not on the card:\n{text}"));
            let before = &line[..line.find(description).unwrap_or(0)];
            before.rsplit('│').next().unwrap_or("").trim().to_owned()
        };
        assert_eq!(key_for("Filter the results"), "f");
        assert_eq!(key_for("Hide or show dead torrents"), "z");
        assert_eq!(key_for("Remove one queue entry"), "c");
    }

    #[test]
    fn the_footer_names_the_real_queue_keys_in_the_downloads_section() {
        let mut a = app();
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(crate::tui::app::Section::Downloads);
        let text = text_of(&render(&a, 120, 40));
        assert!(text.contains("c remove"), "{text}");
        assert!(text.contains("C clear"), "{text}");
    }

    #[test]
    fn the_selected_row_is_styled_differently_from_its_neighbour() {
        // Without this the user cannot tell where the cursor is. Sampling a
        // fixed column would pass on the panel border alone, so this finds
        // the two rendered result rows and compares those.
        let mut a = with_rows(3);
        a.region = Region::Content;
        let buf = render(&a, 100, 30);
        let lines = lines_of(&buf);

        let find = |needle: &str| -> (u16, u16) {
            lines
                .iter()
                .enumerate()
                .find_map(|(y, line)| {
                    line.find(needle).map(|byte_idx| {
                        let col = line[..byte_idx].chars().count() as u16;
                        (col, y as u16)
                    })
                })
                .unwrap_or_else(|| panic!("{needle} is not on screen:\n{}", lines.join("\n")))
        };

        let (x0, y0) = find("row 0");
        let (x1, y1) = find("row 1");
        assert_ne!(y0, y1, "both rows landed on the same line");
        assert_ne!(
            buf[(x0, y0)].style(),
            buf[(x1, y1)].style(),
            "the selected row looks the same as its neighbour"
        );
    }

    #[test]
    fn the_detail_view_shows_the_hash_and_its_actions() {
        let mut a = with_rows(2);
        a.region = Region::Content;
        a.update(Action::Key(KeyAction::Enter));
        let text = text_of(&render(&a, 100, 30));
        assert!(text.contains("row 0"), "detail shows the wrong row: {text}");
        assert!(text.contains("copy"), "no action line: {text}");
    }

    fn queue_entry(title: &str, source_id: Option<&str>) -> crate::download::queue::QueueEntry {
        crate::download::queue::QueueEntry {
            infohash: "ab".repeat(20),
            magnet: "magnet:?xt=urn:btih:".to_owned() + &"ab".repeat(20),
            title: title.to_owned(),
            added_unix: 1_700_000_000,
            paused: false,
            source_id: source_id.map(str::to_owned),
            dir: None,
        }
    }

    fn downloads_screen(entry: crate::download::queue::QueueEntry) -> String {
        let mut a = App::new(std::path::PathBuf::from("/tmp/x"), vec![entry]);
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(crate::tui::app::Section::Downloads);
        text_of(&render(&a, 100, 30))
    }

    #[test]
    fn a_queue_entry_shows_the_source_it_was_found_through() {
        let text = downloads_screen(queue_entry("found by searching", Some("nyaa")));
        assert!(text.contains("NYAA"), "the source tag is missing: {text}");
    }

    #[test]
    fn a_pasted_magnet_shows_the_neutral_tag_rather_than_an_invented_source() {
        let text = downloads_screen(queue_entry("pasted magnet", None));
        assert!(text.contains("pasted magnet"), "{text}");
        for (tag, _) in ["nyaa", "yts", "eztv", "fitgirl"]
            .iter()
            .map(|id| crate::tui::theme::source_tag(id))
        {
            assert!(
                !text.contains(tag),
                "an unrelated source tag appeared: {text}"
            );
        }
    }

    fn snapshot(
        infohash: &str,
        progress_bytes: u64,
        total_bytes: u64,
    ) -> crate::engine::TorrentSnapshot {
        crate::engine::TorrentSnapshot {
            infohash: infohash.to_owned(),
            name: "snapshot".into(),
            state: crate::engine::TorrentState::Downloading,
            progress_bytes,
            total_bytes,
            download_speed: 4096,
            upload_speed: 0,
            error: None,
        }
    }

    fn entry(infohash: &str) -> crate::download::queue::QueueEntry {
        entry_titled(infohash, "a queued release")
    }

    fn entry_titled(infohash: &str, title: &str) -> crate::download::queue::QueueEntry {
        crate::download::queue::QueueEntry {
            infohash: infohash.to_owned(),
            magnet: "magnet:?xt=urn:btih:".to_owned() + infohash,
            title: title.to_owned(),
            added_unix: 1_700_000_000,
            paused: false,
            source_id: None,
            dir: None,
        }
    }

    /// The detail row rendered immediately below a title line, so a test can
    /// check one queue entry's progress without the assertion also being
    /// satisfied by a neighbouring row (which a positional, rather than
    /// infohash, lookup would do).
    fn detail_row_for(buf: &ratatui::buffer::Buffer, title: &str) -> String {
        let lines = lines_of(buf);
        let idx = lines
            .iter()
            .position(|l| l.contains(title))
            .unwrap_or_else(|| panic!("{title} not on screen:\n{}", lines.join("\n")));
        lines
            .get(idx + 1)
            .cloned()
            .unwrap_or_else(|| panic!("no detail row after {title}"))
    }

    fn downloads_app(queue: Vec<crate::download::queue::QueueEntry>) -> App {
        let mut a = App::new(std::path::PathBuf::from("/tmp/x"), queue);
        a.update(Action::Key(KeyAction::Enter));
        a.set_section(crate::tui::app::Section::Downloads);
        a
    }

    #[test]
    fn a_downloading_entry_shows_its_own_progress_not_a_neighbours() {
        // Two entries, only one snapshot — for "bb". A positional lookup
        // (snapshots()[0]) would put that 50% on "aa" instead; matching by
        // infohash must keep it on the right row.
        let mut a = downloads_app(vec![
            entry_titled("aa", "row aa"),
            entry_titled("bb", "row bb"),
        ]);
        a.update(Action::SnapshotsUpdated(vec![snapshot("bb", 512, 1024)]));
        let buf = render(&a, 100, 30);
        assert!(
            detail_row_for(&buf, "row bb").contains("50%"),
            "row bb should show 50%: {}",
            detail_row_for(&buf, "row bb")
        );
        assert!(
            !detail_row_for(&buf, "row aa").contains('%'),
            "row aa has no snapshot and must stay idle: {}",
            detail_row_for(&buf, "row aa")
        );
    }

    #[test]
    fn the_old_placeholder_is_gone() {
        // The panel used to promise downloads "in a later release". They are
        // in this release, and a UI that still says otherwise is lying.
        let a = downloads_app(vec![entry("aa")]);
        let text = text_of(&render(&a, 100, 30));
        assert!(
            !text.contains("later release"),
            "stale promise still rendered:\n{text}"
        );
    }

    #[test]
    fn a_queue_entry_with_no_snapshot_is_shown_idle_not_fabricated() {
        // No live session for this entry: it must not claim a transfer that
        // is not happening.
        let a = downloads_app(vec![entry("aa")]);
        let text = text_of(&render(&a, 100, 30));
        assert!(
            !text.contains('%'),
            "a progress figure was invented: {text}"
        );
        assert!(
            text.to_lowercase().contains("idle") || text.to_lowercase().contains("queued"),
            "no idle state shown: {text}"
        );
    }

    #[test]
    fn an_unprotected_tunnel_reads_differently_from_a_protected_one() {
        // "VPN: " appears in every guard state, so merely finding "vpn" would
        // pass even if the unprotected branch rendered "connected". Check the
        // state-specific text instead, in both directions.
        let mut a = downloads_app(vec![entry("aa")]);
        a.update(Action::GuardChanged(
            crate::vpn::guard::GuardState::Unprotected,
        ));
        let unprotected = text_of(&render(&a, 100, 30));
        assert!(
            unprotected.contains("not connected"),
            "the unprotected state must say so: {unprotected}"
        );

        a.update(Action::GuardChanged(
            crate::vpn::guard::GuardState::Protected {
                device: "wg0".into(),
            },
        ));
        let protected = text_of(&render(&a, 100, 30));
        assert!(
            protected.contains("connected via wg0"),
            "the protected state must name the device: {protected}"
        );
        assert!(
            !protected.contains("not connected"),
            "the protected state must not also claim to be unprotected: {protected}"
        );
    }

    #[test]
    fn the_row_budget_accounts_for_a_two_line_header() {
        // On a BindSupport::Unsupported platform, guard_lines returns TWO
        // lines (status + note). If the row budget still assumes a one-line
        // header (the old hardcoded `- 1`), it overcounts by one row's worth
        // of height and the last download row is truncated or overlaps the
        // block border. Height chosen so a two-row entry only just fits
        // above a two-line header, but not above a one-line header's
        // (wrongly) larger budget.
        //
        // inner height = 24 - 2 (borders) = 22.
        // With a 2-line header: room = (22 - 2) / 2 = 10 rows fit.
        // With the old, wrong 1-line assumption: room = (22 - 1) / 2 = 10
        // (rounds down the same here) -- so this exact height doesn't
        // distinguish them; use a height where the two disagree instead.
        let entries: Vec<_> = (0..12)
            .map(|i| entry_titled(&format!("{i:040x}"), &format!("row {i}")))
            .collect();
        let mut a = downloads_app(entries);
        a.update(Action::GuardChanged(
            crate::vpn::guard::GuardState::Protected {
                device: "wg0".into(),
            },
        ));
        // inner height = 25 - 2 = 23.
        // Two-line header: room = (23 - 2) / 2 = 10, so rows 0..10 fit,
        // "row 9" is the last full row and must be fully present.
        // One-line assumption: room = (23 - 1) / 2 = 11, one row too many is
        // attempted, overrunning the block and corrupting "row 9"'s line.
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).expect("test backend");
        // Render the downloads panel directly (outer area including its own
        // border), with the OS pinned to "windows" so the
        // BindSupport::Unsupported, two-line header path runs regardless of
        // the platform this test actually executes on.
        let panel = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 25,
        };
        terminal
            .draw(|f| crate::tui::render::downloads::draw_with_os(f, panel, &a, "windows"))
            .expect("draw");
        let text = text_of(&terminal.backend().buffer().clone());
        assert!(
            text.contains("row 9"),
            "the last row that should fit was lost or corrupted by an \
             oversized row budget:\n{text}"
        );
        assert!(
            !text.contains("row 10"),
            "a row beyond the true budget was drawn, meaning the budget did \
             not account for the two-line header:\n{text}"
        );
    }

    #[test]
    fn the_bind_unsupported_note_survives_a_narrow_panel() {
        // Render the guard lines directly at a width too small to hold the
        // status text and the note concatenated on one line — this drives
        // the actual `Unsupported` branch (via "windows"), which the app-level
        // tests on this dev machine cannot reach. Putting the note on its own
        // `Line` means a narrow `Paragraph` clips each line independently
        // instead of dropping the whole note while the reassuring status
        // text stays on screen.
        use ratatui::{backend::TestBackend, widgets::Paragraph, Terminal};

        let lines = crate::tui::render::downloads::guard_lines(
            &crate::vpn::guard::GuardState::Protected {
                device: "utun9".into(),
            },
            "windows",
        );
        assert_eq!(lines.len(), 2, "expected a status line and a note line");

        let mut terminal = Terminal::new(TestBackend::new(40, 4)).expect("test backend");
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .expect("draw");
        let text = text_of(&terminal.backend().buffer().clone());
        assert!(
            text.contains("connected via"),
            "connected status missing at narrow width: {text}"
        );
        assert!(
            text.contains("not pinned"),
            "the note must survive at narrow width instead of being clipped away: {text}"
        );
    }

    #[test]
    fn both_prompts_render_and_only_the_targeted_one_names_a_torrent() {
        let mut a = with_rows(1);
        a.region = Region::Content;
        a.update(Action::Key(KeyAction::FolderPrompt));
        let folder = text_of(&render(&a, 100, 30));
        assert!(
            folder.contains("default download folder"),
            "the folder prompt did not render: {folder}"
        );
        // The row behind the overlay is still on screen; the folder prompt
        // itself names no torrent, so the count must not grow.
        let behind = folder.matches("row 0").count();
        a.update(Action::Key(KeyAction::Escape));
        a.update(Action::Key(KeyAction::DownloadTo));
        let to = text_of(&render(&a, 100, 30));
        assert!(
            to.matches("row 0").count() > behind,
            "the download-to prompt lost its target: {to}"
        );
    }

    #[test]
    fn sizes_and_timestamps_render_without_a_date_crate() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GiB");
        // 2023-11-14T22:13:20Z
        assert_eq!(added_label(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(added_label(0), "1970-01-01 00:00");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("héllo wörld", 4), "hél…");
    }
}
