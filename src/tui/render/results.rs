//! The results panel: the list of what the sources returned, and the detail
//! view that replaces it.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{empty, human_size, pad, truncate};
use crate::tui::app::{App, Mode, Region};
use crate::tui::layout::window_start;
use crate::tui::results::Row;
use crate::tui::theme::{icon, source_tag, ACCENT, ALT, BAD, BRIGHT, GOOD, RULE, TEXT, WARN};

/// The highlight behind the cursor row.
const CURSOR_BG: Color = Color::Rgb(0x2a, 0x22, 0x3d);

const IDX_W: usize = 4;
const SIZE_W: usize = 10;
const HEALTH_W: usize = 11;
const SRC_W: usize = 5;
/// Below this the columns are dropped and only the name is shown.
const MIN_COLUMNS_W: usize = IDX_W + SIZE_W + HEALTH_W + SRC_W + 12;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RULE))
        .title(Span::styled(" results ", Style::default().fg(ACCENT)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if empty(inner) {
        return;
    }

    if app.mode == Mode::Detail {
        detail(frame, inner, app);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(status_line(app, inner.width));
    for (id, error) in app.results.failures() {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon::ERROR), Style::default().fg(BAD)),
            Span::styled(
                truncate(
                    &format!("{id}: {error}"),
                    inner.width.saturating_sub(2) as usize,
                ),
                Style::default().fg(BAD),
            ),
        ]));
    }

    let width = inner.width as usize;
    let columns = width >= MIN_COLUMNS_W;
    if columns {
        lines.push(header_line(width));
    }

    let used = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let room = inner.height.saturating_sub(used) as usize;
    let rows = app.results.visible();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.searching {
                "searching…"
            } else {
                "no results"
            },
            Style::default().fg(ALT),
        )));
    } else {
        let start = window_start(app.results.cursor(), rows.len(), room);
        let focused = app.region == Region::Content;
        for (offset, row) in rows.iter().skip(start).take(room).enumerate() {
            let index = start + offset;
            lines.push(row_line(
                row,
                index,
                index == app.results.cursor() && focused,
                width,
                columns,
            ));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn status_line(app: &App, width: u16) -> Line<'static> {
    let mut parts = vec![
        if app.query.is_empty() {
            "no query".to_owned()
        } else {
            format!("\"{}\"", app.query)
        },
        format!("sort {}", app.results.sort_label()),
        format!(
            "dead {}",
            if app.results.hide_dead() {
                "hidden"
            } else {
                "shown"
            }
        ),
    ];
    if !app.results.filter().is_empty() {
        parts.push(format!("filter \"{}\"", app.results.filter()));
    }
    Line::from(Span::styled(
        truncate(&parts.join(&format!(" {} ", icon::DOT)), width as usize),
        Style::default().fg(ALT),
    ))
}

fn header_line(width: usize) -> Line<'static> {
    let name_w = width.saturating_sub(IDX_W + SIZE_W + HEALTH_W + SRC_W);
    Line::from(Span::styled(
        truncate(
            &format!(
                "{}{}{}{}{}",
                pad("#", IDX_W),
                pad("Name", name_w),
                pad("Size", SIZE_W),
                pad("Seed:Lch", HEALTH_W),
                pad("Src", SRC_W)
            ),
            width,
        ),
        Style::default().fg(RULE).add_modifier(Modifier::BOLD),
    ))
}

fn health_colour(row: &Row) -> Color {
    if !row.reports_health {
        return ALT;
    }
    match row.result.seeders {
        0 => BAD,
        1..=4 => WARN,
        _ => GOOD,
    }
}

fn row_line(row: &Row, index: usize, selected: bool, width: usize, columns: bool) -> Line<'static> {
    let (tag, tag_colour) = source_tag(row.result.source_id);
    let base = if selected {
        Style::default()
            .fg(BRIGHT)
            .bg(CURSOR_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };

    if !columns {
        return Line::from(Span::styled(truncate(&row.result.title, width), base));
    }

    let name_w = width.saturating_sub(IDX_W + SIZE_W + HEALTH_W + SRC_W);
    let health = if row.reports_health {
        format!("{}:{}", row.result.seeders, row.result.leechers)
    } else {
        "—".to_owned()
    };
    let marker = if selected { icon::POINTER } else { " " };

    Line::from(vec![
        Span::styled(
            pad(&format!("{marker}{}", index + 1), IDX_W),
            if selected { base } else { base.fg(RULE) },
        ),
        Span::styled(pad(&truncate(&row.result.title, name_w), name_w), base),
        Span::styled(
            pad(&human_size(row.result.size_bytes), SIZE_W),
            if selected { base } else { base.fg(ALT) },
        ),
        Span::styled(pad(&health, HEALTH_W), base.fg(health_colour(row))),
        Span::styled(pad(tag, SRC_W), base.fg(tag_colour)),
    ])
}

fn detail(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    let Some(row) = app.results.selected() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "nothing selected",
                Style::default().fg(ALT),
            ))),
            area,
        );
        return;
    };

    let width = area.width as usize;
    let (tag, tag_colour) = source_tag(row.result.source_id);
    let health = if row.reports_health {
        format!(
            "{} {} seeders {} {} leechers",
            icon::UP,
            row.result.seeders,
            icon::DOWN,
            row.result.leechers
        )
    } else {
        "this source reports no seeder counts".to_owned()
    };

    let field = |label: &str, value: String, colour: Color| -> Line<'static> {
        Line::from(vec![
            Span::styled(pad(label, 8), Style::default().fg(RULE)),
            Span::styled(
                truncate(&value, width.saturating_sub(8)),
                Style::default().fg(colour),
            ),
        ])
    };

    let lines = vec![
        Line::from(Span::styled(
            truncate(&row.result.title, width),
            Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        field("size", human_size(row.result.size_bytes), TEXT),
        field("health", health, health_colour(row)),
        field("source", tag.to_owned(), tag_colour),
        field("hash", row.result.infohash.clone(), ALT),
        field("magnet", row.result.magnet.clone(), ALT),
        Line::from(""),
        Line::from(Span::styled(
            truncate("d download · y copy · esc back", width),
            Style::default().fg(ACCENT),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}
