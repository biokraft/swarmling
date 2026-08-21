//! The overlays: the help card and the two destination prompts. Each is
//! centred over whatever screen is already drawn.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{empty, field_spans, pad, truncate};
use crate::tui::app::{App, Overlay};
use crate::tui::layout::centered;
use crate::tui::theme::{ACCENT, ALT, BRIGHT, RULE, TEXT};

const KEYS: &[(&str, &str)] = &[
    ("↑ ↓ j k", "Move"),
    ("⇥", "Switch between the sidebar and the list"),
    ("↵", "Open the selected result"),
    ("/", "Filter the results"),
    ("s", "Cycle the sort"),
    ("h", "Hide or show dead torrents"),
    ("d", "Queue a download"),
    ("D", "Queue a download to a chosen folder"),
    ("y", "Copy the magnet link"),
    ("o", "Set the default download folder"),
    ("x  X", "Remove one queue entry, or clear the queue"),
    ("?", "Show this help"),
    ("q  ^c", "Quit"),
];

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    match app.overlay {
        Overlay::None => {}
        Overlay::Help => help(frame, area),
        Overlay::FolderPrompt => prompt(frame, area, app, " default download folder ", None),
        Overlay::DownloadTo => {
            let title = app.pending.as_ref().map(|p| p.title.clone());
            prompt(frame, area, app, " download to ", title)
        }
    }
}

fn card(frame: &mut Frame, area: Rect, title: &'static str, lines: Vec<Line<'static>>, width: u16) {
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let target = centered(area, width, height);
    if empty(target) {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(target);
    frame.render_widget(Clear, target);
    frame.render_widget(block, target);
    if empty(inner) {
        return;
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help(frame: &mut Frame, area: Rect) {
    let key_w = KEYS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line<'static>> = KEYS
        .iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(pad(key, key_w + 2), Style::default().fg(ACCENT)),
                Span::styled(*description, Style::default().fg(TEXT)),
            ])
        })
        .collect();
    let width = KEYS
        .iter()
        .map(|(k, d)| k.chars().count() + d.chars().count() + 6)
        .max()
        .unwrap_or(20);
    card(
        frame,
        area,
        " keys ",
        lines,
        u16::try_from(width).unwrap_or(u16::MAX),
    );
}

fn prompt(frame: &mut Frame, area: Rect, app: &App, title: &'static str, target: Option<String>) {
    let width = area.width.saturating_sub(8).clamp(1, 72);
    let field_width = width.saturating_sub(4);

    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(name) = target {
        lines.push(Line::from(Span::styled(
            truncate(&name, field_width as usize),
            Style::default().fg(BRIGHT),
        )));
        lines.push(Line::from(""));
    }
    let mut field = vec![Span::styled("❯ ", Style::default().fg(ACCENT))];
    field.extend(field_spans(app, field_width));
    lines.push(Line::from(field));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        truncate("↵ confirm · esc cancel", field_width as usize),
        Style::default().fg(RULE),
    )));
    lines.push(Line::from(Span::styled(
        truncate(
            "nothing is transferred — the queue records intent",
            field_width as usize,
        ),
        Style::default().fg(ALT),
    )));

    card(frame, area, title, lines, width);
}
