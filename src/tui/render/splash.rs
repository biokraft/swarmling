//! The first screen: the wordmark, what the app is, and one search box.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{empty, field_spans, wordmark_lines};
use crate::tui::app::{App, Section};
use crate::tui::layout::centered;
use crate::tui::theme::{ACCENT, ALT, RULE, TEXT};
use crate::tui::wordmark;

const TAGLINE: &str = "A quiet, terminal-native torrent finder.";
const HINT: &str = "↵ search · ⇥ browse · ^c quit";

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if empty(area) {
        return;
    }

    let wide = area.width >= wordmark::width().saturating_add(2);
    let mut lines: Vec<Line> = Vec::new();

    if wide {
        lines.extend(wordmark_lines());
    } else {
        lines.push(Line::from(Span::styled(
            "swarmling",
            Style::default().fg(ACCENT),
        )));
    }

    let groups: Vec<&str> = Section::ORDER
        .iter()
        .filter(|s| s.group().is_some())
        .map(|s| s.label())
        .collect();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(TAGLINE, Style::default().fg(TEXT))));
    lines.push(Line::from(Span::styled(
        groups.join(" · "),
        Style::default().fg(ALT),
    )));
    lines.push(Line::from(""));

    // The search box is one line: a pointer, then the field itself.
    let box_width = if wide { wordmark::width() } else { area.width };
    let field_width = box_width.saturating_sub(2);
    let mut field_line: Vec<Span> = vec![Span::styled("❯ ", Style::default().fg(ACCENT))];
    field_line.extend(field_spans(app, field_width));
    lines.push(Line::from(field_line));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(HINT, Style::default().fg(RULE))));

    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let target = centered(area, box_width, height);
    if empty(target) {
        return;
    }
    frame.render_widget(Paragraph::new(lines), target);
}
