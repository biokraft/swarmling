//! The overlays: the help card and the two destination prompts. Each is
//! centred over whatever screen is already drawn.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{empty, field_spans, human_size, pad, truncate};
use crate::tui::app::{App, Overlay};
use crate::tui::event;
use crate::tui::layout::centered;
use crate::tui::theme::{ACCENT, ALT, BRIGHT, RULE, TEXT};

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    match app.overlay {
        Overlay::None => {}
        Overlay::Help => help(frame, area),
        Overlay::FolderPrompt => prompt(frame, area, app, " default download folder ", None),
        Overlay::DownloadTo => {
            // The spec asks for the name *and* the size: the destination is
            // being chosen, and how much will land there is part of that.
            let title = app.pending.as_ref().map(|p| {
                if p.size_bytes == 0 {
                    p.title.clone()
                } else {
                    format!("{} · {}", p.title, human_size(p.size_bytes))
                }
            });
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

/// The help card is derived from `event::HELP` so it can never document a
/// key the mapper does not bind. Keys that mean the same thing share a row.
fn help_rows() -> Vec<(String, &'static str)> {
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    for entry in event::HELP {
        match rows.iter_mut().find(|(_, d)| *d == entry.description) {
            Some((keys, _)) => {
                if !keys.split(' ').any(|k| k == entry.key) {
                    keys.push(' ');
                    keys.push_str(entry.key);
                }
            }
            None => rows.push((entry.key.to_owned(), entry.description)),
        }
    }
    rows
}

fn help(frame: &mut Frame, area: Rect) {
    let rows = help_rows();
    let key_w = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line<'static>> = rows
        .iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(pad(key, key_w + 2), Style::default().fg(ACCENT)),
                Span::styled(*description, Style::default().fg(TEXT)),
            ])
        })
        .collect();
    let width = rows
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
