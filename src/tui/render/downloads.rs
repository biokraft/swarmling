//! The downloads panel: the queue, matched by infohash against whatever the
//! live engine reports, plus the VPN guard's own status. An entry with no
//! matching snapshot has no progress to show and is rendered idle — never a
//! fabricated "downloading, 0%" row.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{added_label, empty, human_size, truncate};
use crate::engine::{TorrentSnapshot, TorrentState};
use crate::tui::app::{App, Region};
use crate::tui::layout::window_start;
use crate::tui::theme::{icon, source_tag, ACCENT, BAD, BRIGHT, GOOD, RULE, TEXT, WARN};
use crate::vpn::guard::GuardState;
use crate::vpn::policy::BindSupport;

const CURSOR_BG: Color = Color::Rgb(0x2a, 0x22, 0x3d);

/// Each entry takes a title row and a detail row.
const ROW_HEIGHT: usize = 2;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RULE))
        .title(Span::styled(" downloads ", Style::default().fg(ACCENT)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if empty(inner) {
        return;
    }

    let width = inner.width as usize;
    let mut lines: Vec<Line> = vec![guard_line(app.guard_state(), std::env::consts::OS)];

    if app.queue.is_empty() {
        lines.push(Line::from(Span::styled(
            truncate("the queue is empty", width),
            Style::default().fg(RULE),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let room = (inner.height.saturating_sub(1) as usize) / ROW_HEIGHT;
    let start = window_start(app.queue_cursor, app.queue.len(), room);
    let focused = app.region == Region::Content;

    for (offset, entry) in app.queue.iter().skip(start).take(room).enumerate() {
        let selected = start + offset == app.queue_cursor && focused;
        let title_style = if selected {
            Style::default()
                .fg(BRIGHT)
                .bg(CURSOR_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let marker = if selected { icon::POINTER } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(ACCENT)),
            Span::styled(truncate(&entry.title, width.saturating_sub(2)), title_style),
        ]));

        // A pasted magnet came from no source, so it has no tag to show;
        // `source_tag` gives the neutral marker for that case.
        let (tag, tag_colour) = source_tag(entry.source_id.as_deref().unwrap_or(""));
        let snapshot = app
            .snapshots()
            .iter()
            .find(|s| s.infohash == entry.infohash);
        let mut detail = vec![
            Span::raw("  "),
            Span::styled(format!("{tag} "), Style::default().fg(tag_colour)),
            Span::styled(
                format!(
                    "{} {} added {}",
                    short_hash(&entry.infohash),
                    icon::DOT,
                    added_label(entry.added_unix)
                ),
                Style::default().fg(RULE),
            ),
        ];
        detail.push(progress_span(snapshot));
        if entry.paused {
            detail.push(Span::styled(
                format!(" {} paused", icon::PAUSE),
                Style::default().fg(WARN),
            ));
        }
        lines.push(Line::from(detail));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The first eight characters of an infohash — enough to tell two queue
/// entries apart without filling the row.
fn short_hash(infohash: &str) -> String {
    infohash.chars().take(8).collect()
}

/// The state word for a snapshot's `TorrentState`.
fn state_label(state: TorrentState) -> &'static str {
    match state {
        TorrentState::Checking => "checking",
        TorrentState::Downloading => "downloading",
        TorrentState::Seeding => "seeding",
        TorrentState::Paused => "paused",
        TorrentState::Errored => "errored",
    }
}

/// Whole-number percent complete, guarding a zero denominator. `progress_bytes
/// * 100` can overflow a `u64` for a very large torrent, so the multiply runs
/// in `u128` before dividing back down; the result is always in `0..=100`,
/// which fits a `u64` without loss.
fn percent(progress_bytes: u64, total_bytes: u64) -> u64 {
    if total_bytes == 0 {
        return 0;
    }
    let capped = progress_bytes.min(total_bytes) as u128;
    ((capped * 100) / total_bytes as u128) as u64
}

/// The detail-row span describing a queue entry's live progress, or its idle
/// state when no snapshot exists for it yet.
fn progress_span(snapshot: Option<&TorrentSnapshot>) -> Span<'static> {
    match snapshot {
        None => Span::styled(
            " idle — not yet started".to_owned(),
            Style::default().fg(RULE),
        ),
        Some(s) => {
            let pct = percent(s.progress_bytes, s.total_bytes);
            Span::styled(
                format!(
                    " {} {pct}% {} {}/s",
                    state_label(s.state),
                    icon::DOWN,
                    human_size(s.download_speed)
                ),
                Style::default().fg(TEXT),
            )
        }
    }
}

/// The persistent line describing the tunnel: whether it is up, and — on a
/// platform that cannot bind traffic to an interface — that traffic is not
/// pinned to the device even while protected.
fn guard_line(guard: &GuardState, os: &str) -> Line<'static> {
    let (text, colour) = match guard {
        GuardState::Unprotected => ("VPN: not connected — nothing is protected".to_owned(), BAD),
        GuardState::Protected { device } => (format!("VPN: connected via {device}"), GOOD),
        GuardState::Lost { previous_device } => (
            format!("VPN: lost ({previous_device}) — downloads are paused"),
            BAD,
        ),
    };
    let mut spans = vec![Span::styled(text, Style::default().fg(colour))];
    if matches!(
        crate::vpn::policy::bind_support_for(os),
        BindSupport::Unsupported
    ) {
        spans.push(Span::styled(
            " — traffic is not pinned to the tunnel device on this platform".to_owned(),
            Style::default().fg(WARN),
        ));
    }
    Line::from(spans)
}
