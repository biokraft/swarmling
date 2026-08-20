//! The results model: accumulates search results streamed in per source,
//! deduplicates, sorts, filters, and tracks the list cursor. Pure data — it
//! draws nothing and performs no IO.

use std::cmp::Ordering;

use crate::sources::{SearchResult, SourceGroup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Default,
    Size(Dir),
    Seeders(Dir),
    Source(Dir),
}

#[derive(Debug, Clone)]
pub struct Row {
    pub result: SearchResult,
    pub reports_health: bool,
    pub groups: &'static [SourceGroup],
}

pub struct Results {
    all: Vec<Row>,
    visible: Vec<Row>,
    failures: Vec<(&'static str, String)>,
    cursor: usize,
    anchor: Option<String>,
    sort: Sort,
    hide_dead: bool,
    filter: String,
    group: Option<SourceGroup>,
}

impl Default for Results {
    fn default() -> Self {
        Self::new()
    }
}

fn title_cmp(a: &Row, b: &Row) -> Ordering {
    a.result.title.cmp(&b.result.title)
}

fn apply_dir(dir: Dir, ord: Ordering) -> Ordering {
    match dir {
        Dir::Asc => ord,
        Dir::Desc => ord.reverse(),
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.trim()
        .to_lowercase()
        .split_whitespace()
        .map(|s| s.to_owned())
        .collect()
}

/// Score a title against filter tokens. `None` if not every token matches.
fn filter_score(title: &str, toks: &[String]) -> Option<u32> {
    if toks.is_empty() {
        return Some(0);
    }
    let lower = title.to_lowercase();
    if !toks.iter().all(|t| lower.contains(t.as_str())) {
        return None;
    }
    let mut score = 10;
    let phrase = toks.join(" ");
    if lower.contains(&phrase) {
        score += 50;
    } else {
        // Tokens appear in order, not necessarily contiguous.
        let mut rest = lower.as_str();
        let mut in_order = true;
        for t in toks {
            match rest.find(t.as_str()) {
                Some(idx) => rest = &rest[idx + t.len()..],
                None => {
                    in_order = false;
                    break;
                }
            }
        }
        if in_order {
            score += 20;
        }
    }
    Some(score)
}

impl Results {
    pub fn new() -> Self {
        Self {
            all: Vec::new(),
            visible: Vec::new(),
            failures: Vec::new(),
            cursor: 0,
            anchor: None,
            sort: Sort::Default,
            hide_dead: false,
            filter: String::new(),
            group: None,
        }
    }

    pub fn clear(&mut self) {
        self.all.clear();
        self.failures.clear();
        self.anchor = None;
        self.recompute();
    }

    pub fn ingest(
        &mut self,
        source_id: &'static str,
        reports_health: bool,
        groups: &'static [SourceGroup],
        results: Vec<SearchResult>,
    ) {
        for result in results {
            if result.infohash.is_empty() {
                continue;
            }
            let row = Row {
                result,
                reports_health,
                groups,
            };
            match self
                .all
                .iter_mut()
                .find(|r| r.result.infohash == row.result.infohash)
            {
                Some(existing) => {
                    if row.result.seeders > existing.result.seeders {
                        *existing = row;
                    }
                }
                None => self.all.push(row),
            }
        }
        // source_id is accepted for symmetry with `fail` and future
        // per-source bookkeeping; dedup itself is keyed on infohash.
        let _ = source_id;
        self.recompute();
    }

    pub fn fail(&mut self, source_id: &'static str, error: String) {
        self.failures.push((source_id, error));
    }

    pub fn failures(&self) -> &[(&'static str, String)] {
        &self.failures
    }

    pub fn visible(&self) -> &[Row] {
        &self.visible
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_cursor(&mut self, delta: isize, wrap: bool) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let cur = self.cursor as isize;
        let mut next = cur + delta;
        if wrap {
            next = next.rem_euclid(len as isize);
        } else {
            next = next.clamp(0, len as isize - 1);
        }
        self.cursor = next as usize;
        self.anchor = self
            .visible
            .get(self.cursor)
            .map(|r| r.result.infohash.clone());
    }

    pub fn page(&mut self, delta: isize, page: usize) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let step = std::cmp::max(1, page.saturating_sub(1)) as isize;
        let cur = self.cursor as isize;
        let next = (cur + delta * step).clamp(0, len as isize - 1);
        self.cursor = next as usize;
        self.anchor = self
            .visible
            .get(self.cursor)
            .map(|r| r.result.infohash.clone());
    }

    pub fn selected(&self) -> Option<&Row> {
        self.visible.get(self.cursor)
    }

    pub fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            Sort::Default => Sort::Size(Dir::Asc),
            Sort::Size(Dir::Asc) => Sort::Size(Dir::Desc),
            Sort::Size(Dir::Desc) => Sort::Seeders(Dir::Asc),
            Sort::Seeders(Dir::Asc) => Sort::Seeders(Dir::Desc),
            Sort::Seeders(Dir::Desc) => Sort::Source(Dir::Asc),
            Sort::Source(Dir::Asc) => Sort::Source(Dir::Desc),
            Sort::Source(Dir::Desc) => Sort::Default,
        };
        self.recompute();
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn sort_label(&self) -> String {
        match self.sort {
            Sort::Default => "default".to_owned(),
            Sort::Size(dir) => format!("size {}", arrow(dir)),
            Sort::Seeders(dir) => format!("seeders {}", arrow(dir)),
            Sort::Source(dir) => format!("source {}", arrow(dir)),
        }
    }

    pub fn toggle_hide_dead(&mut self) {
        self.hide_dead = !self.hide_dead;
        self.recompute();
    }

    pub fn hide_dead(&self) -> bool {
        self.hide_dead
    }

    pub fn set_filter(&mut self, text: &str) {
        self.filter = text.to_owned();
        self.recompute();
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_group(&mut self, group: Option<SourceGroup>) {
        self.group = group;
        self.recompute();
    }

    fn recompute(&mut self) {
        let toks = tokens(&self.filter);

        let mut candidates: Vec<(Row, u32)> = self
            .all
            .iter()
            .filter(|row| !(self.hide_dead && row.reports_health && row.result.seeders == 0))
            .filter(|row| match self.group {
                None => true,
                Some(g) => row.groups.contains(&g),
            })
            .filter_map(|row| {
                filter_score(&row.result.title, &toks).map(|score| (row.clone(), score))
            })
            .collect();

        candidates.sort_by(|(a, sa), (b, sb)| {
            if self.sort == Sort::Default && !toks.is_empty() && sa != sb {
                return sb.cmp(sa);
            }
            self.compare(a, b)
        });

        self.visible = candidates.into_iter().map(|(row, _)| row).collect();
        self.restore_cursor();
    }

    fn compare(&self, a: &Row, b: &Row) -> Ordering {
        match self.sort {
            Sort::Default => a
                .result
                .seeders
                .cmp(&b.result.seeders)
                .reverse()
                .then_with(|| title_cmp(a, b)),
            Sort::Size(dir) => apply_dir(dir, a.result.size_bytes.cmp(&b.result.size_bytes))
                .then_with(|| b.result.seeders.cmp(&a.result.seeders))
                .then_with(|| title_cmp(a, b)),
            Sort::Seeders(dir) => apply_dir(dir, a.result.seeders.cmp(&b.result.seeders))
                .then_with(|| title_cmp(a, b)),
            Sort::Source(dir) => apply_dir(dir, a.result.source_id.cmp(b.result.source_id))
                .then_with(|| b.result.seeders.cmp(&a.result.seeders))
                .then_with(|| title_cmp(a, b)),
        }
    }

    fn restore_cursor(&mut self) {
        match &self.anchor {
            Some(hash) => {
                if let Some(idx) = self.visible.iter().position(|r| &r.result.infohash == hash) {
                    self.cursor = idx;
                } else {
                    self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
                }
            }
            None => self.cursor = 0,
        }
    }
}

fn arrow(dir: Dir) -> &'static str {
    match dir {
        Dir::Asc => "\u{25B4}",
        Dir::Desc => "\u{25BE}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SearchResult;

    fn row(title: &str, seeders: u32, size: u64, source: &'static str, hash: &str) -> SearchResult {
        SearchResult {
            title: title.to_owned(),
            magnet: format!("magnet:?xt=urn:btih:{hash}"),
            size_bytes: size,
            seeders,
            leechers: 0,
            source_id: source,
            infohash: hash.to_owned(),
        }
    }

    fn hash(n: u8) -> String {
        format!("{:040x}", n)
    }

    #[test]
    fn hide_dead_spares_sources_that_report_no_swarm_data() {
        // THE hazard this filter has: a source with no swarm data reports
        // zero seeders for everything. Filtering it on seeder count hides
        // every real result and labels them dead. Verified to fail if the
        // reports_health check is removed.
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![row("healthy dead", 0, 1, "yts", &hash(1))],
        );
        r.ingest(
            "fitgirl",
            false,
            &[SourceGroup::Games],
            vec![row("quiet source", 0, 1, "fitgirl", &hash(2))],
        );
        r.toggle_hide_dead();
        let titles: Vec<&str> = r
            .visible()
            .iter()
            .map(|x| x.result.title.as_str())
            .collect();
        assert_eq!(titles, vec!["quiet source"]);
    }

    #[test]
    fn duplicates_collapse_keeping_the_better_seeded_copy() {
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![row("film", 5, 100, "yts", &hash(1))],
        );
        r.ingest(
            "tpb-movies",
            true,
            &[SourceGroup::Movies],
            vec![row("film", 50, 100, "tpb-movies", &hash(1))],
        );
        assert_eq!(r.visible().len(), 1);
        assert_eq!(r.visible()[0].result.seeders, 50);
    }

    #[test]
    fn the_default_order_is_most_seeded_first_and_is_total() {
        // A non-total order would reshuffle equal rows between renders and
        // make the cursor appear to jump on its own.
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![
                row("b", 10, 1, "yts", &hash(1)),
                row("a", 10, 1, "yts", &hash(2)),
                row("c", 99, 1, "yts", &hash(3)),
            ],
        );
        let titles: Vec<&str> = r
            .visible()
            .iter()
            .map(|x| x.result.title.as_str())
            .collect();
        assert_eq!(titles, vec!["c", "a", "b"]);
    }

    #[test]
    fn the_sort_cycle_wraps_through_every_state_and_returns_to_default() {
        let mut r = Results::new();
        let mut seen = vec![r.sort()];
        for _ in 0..6 {
            r.cycle_sort();
            seen.push(r.sort());
        }
        assert_eq!(
            seen,
            vec![
                Sort::Default,
                Sort::Size(Dir::Asc),
                Sort::Size(Dir::Desc),
                Sort::Seeders(Dir::Asc),
                Sort::Seeders(Dir::Desc),
                Sort::Source(Dir::Asc),
                Sort::Source(Dir::Desc),
            ]
        );
        r.cycle_sort();
        assert_eq!(r.sort(), Sort::Default);
    }

    #[test]
    fn every_filter_token_must_match_and_a_phrase_scores_highest() {
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![
                row("big buck bunny", 1, 1, "yts", &hash(1)),
                row("bunny big feet", 1, 1, "yts", &hash(2)),
                row("unrelated", 1, 1, "yts", &hash(3)),
            ],
        );
        r.set_filter("big bunny");
        let titles: Vec<&str> = r
            .visible()
            .iter()
            .map(|x| x.result.title.as_str())
            .collect();
        assert_eq!(titles.len(), 2, "only rows matching every token survive");
        assert_eq!(titles[0], "big buck bunny", "tokens in order rank first");
    }

    #[test]
    fn an_explicit_sort_beats_filter_relevance() {
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![
                row("aaa match", 1, 900, "yts", &hash(1)),
                row("match zzz", 1, 100, "yts", &hash(2)),
            ],
        );
        r.set_filter("match");
        r.cycle_sort(); // Size ascending
        assert_eq!(r.visible()[0].result.size_bytes, 100);
    }

    #[test]
    fn the_cursor_stays_on_the_same_torrent_when_results_are_replaced() {
        // Results stream in per source, so the list is rebuilt repeatedly
        // mid-search. If the cursor tracked an index the user's selection
        // would slide under them as new rows arrive.
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![
                row("first", 90, 1, "yts", &hash(1)),
                row("second", 80, 1, "yts", &hash(2)),
            ],
        );
        r.move_cursor(1, true);
        let picked = r.selected().map(|x| x.result.infohash.clone());
        assert_eq!(picked.as_deref(), Some(hash(2).as_str()));
        r.ingest(
            "tpb-movies",
            true,
            &[SourceGroup::Movies],
            vec![row("newcomer", 999, 1, "tpb-movies", &hash(3))],
        );
        assert_eq!(
            r.selected().map(|x| x.result.infohash.clone()).as_deref(),
            Some(hash(2).as_str()),
            "cursor followed the row instead of the index"
        );
    }

    #[test]
    fn the_cursor_clamps_when_its_row_disappears() {
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![
                row("a", 9, 1, "yts", &hash(1)),
                row("b", 8, 1, "yts", &hash(2)),
            ],
        );
        r.move_cursor(1, true);
        r.set_filter("a");
        assert!(r.cursor() < r.visible().len().max(1));
        assert!(r.selected().is_some());
    }

    #[test]
    fn navigating_an_empty_list_is_safe() {
        let mut r = Results::new();
        r.move_cursor(1, true);
        r.move_cursor(-1, true);
        r.page(1, 10);
        assert_eq!(r.cursor(), 0);
        assert!(r.selected().is_none());
    }

    #[test]
    fn paging_clamps_while_stepping_wraps() {
        let mut r = Results::new();
        let rows: Vec<SearchResult> = (1..=5)
            .map(|i| row(&format!("r{i}"), 100 - i, 1, "yts", &hash(i as u8)))
            .collect();
        r.ingest("yts", true, &[SourceGroup::Movies], rows);
        r.page(-1, 3);
        assert_eq!(r.cursor(), 0, "page up clamps at the top");
        r.page(1, 3);
        r.page(1, 3);
        assert_eq!(r.cursor(), 4, "page down clamps at the bottom");
        r.move_cursor(1, true);
        assert_eq!(r.cursor(), 0, "stepping down from the last row wraps");
    }

    #[test]
    fn the_category_tab_restricts_the_list_to_that_group() {
        // A tab that silently fails to filter is worse than no tab: the user
        // believes they are looking at one category while seeing everything.
        let mut r = Results::new();
        r.ingest(
            "yts",
            true,
            &[SourceGroup::Movies],
            vec![row("a film", 10, 1, "yts", &hash(1))],
        );
        r.ingest(
            "nyaa",
            true,
            &[SourceGroup::Anime],
            vec![row("an episode", 10, 1, "nyaa", &hash(2))],
        );
        assert_eq!(r.visible().len(), 2, "the All tab shows everything");

        r.set_group(Some(SourceGroup::Anime));
        let titles: Vec<&str> = r
            .visible()
            .iter()
            .map(|x| x.result.title.as_str())
            .collect();
        assert_eq!(titles, vec!["an episode"]);

        r.set_group(None);
        assert_eq!(r.visible().len(), 2, "clearing the tab restores everything");
    }

    #[test]
    fn a_failed_source_is_recorded_rather_than_swallowed() {
        // Silence about a failing source tells the user "nothing matched"
        // when the truth is "we did not look".
        let mut r = Results::new();
        r.fail("nyaa", "timed out".to_owned());
        assert_eq!(r.failures().len(), 1);
        assert_eq!(r.failures()[0].0, "nyaa");
    }
}
