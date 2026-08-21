//! The downloads panel. It is read-only by design: this milestone starts no
//! session, so there is no progress to report. A progress bar here would be
//! an invention, and the panel says so instead.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{added_label, empty, truncate};
use crate::tui::app::{App, Region};
use crate::tui::layout::window_start;
use crate::tui::theme::{icon, source_tag, ACCENT, ALT, BRIGHT, RULE, TEXT, WARN};

const CURSOR_BG: Color = Color::Rgb(0x2a, 0x22, 0x3d);
const EXPLAINER: &str = "queued — downloads start in a later release";

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
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        truncate(EXPLAINER, width),
        Style::default().fg(ALT),
    ))];

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
