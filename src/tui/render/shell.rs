//! The browser chrome: header, rule, sidebar, footer, and whichever panel
//! owns the content region.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{downloads, empty, results, truncate};
use crate::tui::app::{App, Mode, Region, Section};
use crate::tui::layout::{rail_width, regions};
use crate::tui::theme::{icon, ACCENT, ALT, BRIGHT, RULE, TEXT};
use crate::tui::wordmark::SPROUT;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }

    let labels = rail_labels(app);
    let rail: Vec<(&str, Option<usize>)> = labels.iter().map(|(l, b)| (*l, *b)).collect();
    let r = regions(area, rail_width(&rail));

    header(frame, r.header, app);
    if let Some(rule) = r.rule {
        horizontal_rule(frame, rule);
    }
    sidebar(frame, r.sidebar, app, &labels);
    if app.section == Section::Downloads {
        downloads::draw(frame, r.content, app);
    } else {
        results::draw(frame, r.content, app);
    }
    if let Some(footer) = r.footer {
        footer_line(frame, footer, app);
    }
}

fn rail_labels(app: &App) -> Vec<(&'static str, Option<usize>)> {
    Section::ORDER
        .iter()
        .map(|s| {
            let badge = if *s == Section::Downloads && !app.queue.is_empty() {
                Some(app.queue.len())
            } else {
                None
            };
            (s.label(), badge)
        })
        .collect()
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    // The header is one row tall, so it carries the plain name rather than
    // the three-line wordmark. The wordmark's gradient stays on the splash.
    let mut spans = vec![
        Span::styled("𐓏 ", Style::default().fg(SPROUT)),
        Span::styled(
            "swarmling",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ];

    let right = if let Some(text) = app.notice_text() {
        text.to_owned()
    } else if app.searching {
        "searching…".to_owned()
    } else {
        String::new()
    };

    if !right.is_empty() {
        let used = spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum::<usize>();
        let room = (area.width as usize).saturating_sub(used);
        let right = truncate(&right, room);
        let gap = room.saturating_sub(right.chars().count());
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(right, Style::default().fg(BRIGHT)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn horizontal_rule(frame: &mut Frame, area: Rect) {
    if empty(area) {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(RULE),
        ))),
        area,
    );
}

fn sidebar(frame: &mut Frame, area: Rect, app: &App, labels: &[(&'static str, Option<usize>)]) {
    if empty(area) {
        return;
    }
    let focused = app.region == Region::Sidebar;
    let lines: Vec<Line> = Section::ORDER
        .iter()
        .zip(labels.iter())
        .map(|(section, (label, badge))| {
            let selected = *section == app.section;
            let marker = if selected { icon::BAR } else { " " };
            let text = match badge {
                Some(n) => format!("{label} ({n})"),
                None => (*label).to_owned(),
            };
            let style = match (selected, focused) {
                (true, true) => Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
                (true, false) => Style::default().fg(ALT),
                _ => Style::default().fg(TEXT),
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(ACCENT)),
                Span::styled(
                    truncate(&text, area.width.saturating_sub(1) as usize),
                    style,
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn footer_line(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }
    let hint = match (app.section, app.mode, app.region) {
        (_, Mode::Search, _) => "↵ search · esc cancel",
        (_, Mode::Filter, _) => "↵ apply · esc cancel",
        (_, Mode::Detail, _) => "d download · y copy · esc back",
        (Section::Downloads, _, Region::Content) => "x remove · X clear · ? help",
        (_, _, Region::Content) => "↵ detail · d download · / filter · s sort · ? help",
        (_, _, Region::Sidebar) => "⇥ focus · / search · o folder · ? help · q quit",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate(hint, area.width as usize),
            Style::default().fg(RULE),
        ))),
        area,
    );
}
