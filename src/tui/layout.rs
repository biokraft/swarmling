//! Pure geometry for the TUI: computes screen regions from the terminal
//! size and list-window scrolling maths. Draws nothing.

use ratatui::layout::Rect;

/// The screen regions for a single frame. `rule` and `footer` are `None`
/// when the terminal is too small to show them.
pub struct Regions {
    pub header: Rect,
    pub rule: Option<Rect>,
    pub sidebar: Rect,
    pub content: Rect,
    pub footer: Option<Rect>,
}

const FOOTER_HEIGHT: u16 = 1;

/// Clamps `r` so it lies entirely inside `area`, even when `r`'s origin is
/// outside `area`. `Rect::intersection` can leave the origin of a
/// zero-sized rect outside the target area when there is no overlap, which
/// is exactly the degenerate case this layout must never produce.
fn clamp(r: Rect, area: Rect) -> Rect {
    let x = r.x.clamp(area.x, area.right());
    let y = r.y.clamp(area.y, area.bottom());
    let width = r.width.min(area.right().saturating_sub(x));
    let height = r.height.min(area.bottom().saturating_sub(y));
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Splits `area` into header / rule / sidebar / content / footer regions.
///
/// Every returned rect is intersected with `area` before being returned,
/// so none of them can ever run past the buffer, even at degenerate sizes
/// (0x0, 1x1, etc). When a region has no room, its rect is zero-width or
/// zero-height rather than clamped to a positive size — callers must skip
/// empty rects.
pub fn regions(area: Rect, rail_width: u16) -> Regions {
    let compact = area.height < 18;
    let has_footer = area.height >= 12;

    let header = clamp(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        area,
    );

    let mut body_top = area.y.saturating_add(1);

    let rule = if compact {
        None
    } else {
        let r = clamp(
            Rect {
                x: area.x,
                y: body_top,
                width: area.width,
                height: 1,
            },
            area,
        );
        body_top = body_top.saturating_add(1);
        Some(r)
    };

    let footer_y = area.bottom().saturating_sub(FOOTER_HEIGHT);
    let footer = if has_footer {
        Some(clamp(
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: FOOTER_HEIGHT,
            },
            area,
        ))
    } else {
        None
    };

    let body_bottom = if has_footer { footer_y } else { area.bottom() };
    let body_height = body_bottom.saturating_sub(body_top);

    let sidebar = clamp(
        Rect {
            x: area.x,
            y: body_top,
            width: rail_width,
            height: body_height,
        },
        area,
    );

    let content_x = area.x.saturating_add(rail_width).saturating_add(1);
    let content_width = area.width.saturating_sub(rail_width.saturating_add(3));
    let content = clamp(
        Rect {
            x: content_x,
            y: body_top,
            width: content_width,
            height: body_height,
        },
        area,
    );

    Regions {
        header,
        rule,
        sidebar,
        content,
        footer,
    }
}

/// The sidebar width needed to show every `(label, badge_count)` pair
/// without truncation.
pub fn rail_width(labels: &[(&str, Option<usize>)]) -> u16 {
    let mut max_width: usize = 0;
    for (label, badge) in labels {
        let mut width = label.chars().count();
        if let Some(count) = badge {
            // " (" + digits + ")"
            width += 1 + 2 + count.to_string().len();
        }
        max_width = max_width.max(width);
    }
    u16::try_from(max_width)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
}

/// The first visible index of a scrolling list of `total` items in a
/// window of `height` rows, centred on `cursor` and clamped so the
/// window never starts before 0 nor extends past the end of the list.
///
/// Returns 0 when the list fits entirely (`total <= height`) or when
/// `height` is 0.
pub fn window_start(cursor: usize, total: usize, height: usize) -> usize {
    if height == 0 || total <= height {
        return 0;
    }
    let max_start = total - height;
    cursor.saturating_sub(height / 2).min(max_start)
}

/// A `width` x `height` box centred inside `area`, clamped so it never
/// overflows `area` even when the requested size is larger than `area`.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x.saturating_add(area.width.saturating_sub(w) / 2);
    let y = area.y.saturating_add(area.height.saturating_sub(h) / 2);
    clamp(
        Rect {
            x,
            y,
            width: w,
            height: h,
        },
        area,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn a_normal_terminal_gets_every_region() {
        let r = regions(area(100, 40), 14);
        assert!(r.rule.is_some());
        assert!(r.footer.is_some());
        assert_eq!(r.sidebar.width, 14);
        assert!(r.content.width > 0);
    }

    #[test]
    fn a_short_terminal_drops_the_rule_then_the_footer() {
        // compact below 18 rows, footer hidden below 12.
        let compact = regions(area(100, 15), 14);
        assert!(compact.rule.is_none());
        assert!(compact.footer.is_some());
        let tiny = regions(area(100, 10), 14);
        assert!(tiny.rule.is_none());
        assert!(tiny.footer.is_none());
    }

    #[test]
    fn every_region_stays_inside_the_terminal_at_any_size() {
        // A region that runs past the buffer panics inside ratatui when it
        // is drawn, so this is the guard against a resize crash.
        for w in [0u16, 1, 5, 20, 80, 200] {
            for h in [0u16, 1, 5, 11, 17, 40] {
                let a = area(w, h);
                let r = regions(a, 14);
                for rect in [r.header, r.sidebar, r.content]
                    .into_iter()
                    .chain(r.rule)
                    .chain(r.footer)
                {
                    assert!(rect.right() <= a.right(), "{rect:?} overflows {a:?}");
                    assert!(rect.bottom() <= a.bottom(), "{rect:?} overflows {a:?}");
                }
            }
        }
    }

    #[test]
    fn the_rail_is_as_wide_as_its_longest_label_plus_a_badge() {
        let w = rail_width(&[("All", None), ("Downloads", Some(12))]);
        assert!(w >= "Downloads".len() as u16 + 5);
    }

    #[test]
    fn the_window_centres_on_the_cursor_and_clamps_at_both_ends() {
        assert_eq!(window_start(0, 100, 10), 0);
        assert_eq!(window_start(50, 100, 10), 45);
        assert_eq!(window_start(99, 100, 10), 90);
        assert_eq!(
            window_start(3, 5, 10),
            0,
            "a list that fits does not scroll"
        );
        assert_eq!(window_start(0, 0, 10), 0);
    }

    #[test]
    fn centring_a_box_larger_than_its_area_does_not_overflow() {
        let a = area(10, 5);
        let c = centered(a, 100, 100);
        assert!(c.right() <= a.right());
        assert!(c.bottom() <= a.bottom());
    }
}
