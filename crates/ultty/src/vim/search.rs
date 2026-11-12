//! Scrollback search: smart-case, optional regex, matches across soft-wrapped
//! rows, and a scan that can be paused between frames.
//!
//! A logical line is a run of rows joined by the wrap flag. Its text is
//! matched as one string; each match is mapped back to the cells it covers,
//! so wide characters and grapheme clusters highlight and jump correctly.

use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::grid::{Buffer, RowView};

/// A logical line longer than this is searched in pieces, so one endless
/// wrapped line cannot grow the match buffer without bound.
const MAX_LINE_BYTES: usize = 1 << 20;

/// How many rows to scan between checks of the clock.
const ROWS_PER_CLOCK_CHECK: usize = 64;

/// The `[search]` config section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    /// Read queries as regular expressions (the `regex` crate's syntax).
    /// Ctrl+R toggles it while typing a query.
    pub regex: bool,
    /// Ignore case unless the query has an uppercase letter. When false,
    /// matching is always case sensitive.
    pub smart_case: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            regex: false,
            smart_case: true,
        }
    }
}

/// A match, as absolute rows and cell columns. `end_col` is the last cell
/// covered (the right half of a wide character).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

impl SearchMatch {
    /// A one-cell match.
    pub fn at(row: usize, col: usize) -> Self {
        Self {
            start_row: row,
            start_col: col,
            end_row: row,
            end_col: col,
        }
    }

    /// The columns this match covers on `row` as `first..=last`, with a
    /// `None` last column meaning "to the end of the row".
    pub fn cols_on(&self, row: usize) -> Option<(usize, Option<usize>)> {
        if row < self.start_row || row > self.end_row {
            return None;
        }
        let first = if row == self.start_row {
            self.start_col
        } else {
            0
        };
        let last = (row == self.end_row).then_some(self.end_col);
        Some((first, last))
    }
}

/// Why a query could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error("invalid regex: {0}")]
    InvalidRegex(String),
}

/// A compiled query.
#[derive(Debug, Clone)]
pub struct Matcher {
    /// `None` for the empty query, which matches nothing.
    regex: Option<Regex>,
}

impl Matcher {
    pub fn new(query: &str, options: SearchOptions) -> Result<Self, SearchError> {
        if query.is_empty() {
            return Ok(Self { regex: None });
        }
        let pattern = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let ignore_case = !options.smart_case_sensitive(query);
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(ignore_case)
            .build()
            .map_err(|e| SearchError::InvalidRegex(short_message(&e)))?;
        Ok(Self { regex: Some(regex) })
    }
}

impl SearchOptions {
    /// Whether `query` is matched case sensitively.
    fn smart_case_sensitive(&self, query: &str) -> bool {
        if !self.smart_case {
            return true;
        }
        let mut chars = query.chars();
        while let Some(c) = chars.next() {
            if self.regex && c == '\\' {
                // `\S`, `\W` and friends are not uppercase letters typed.
                chars.next();
            } else if c.is_uppercase() {
                return true;
            }
        }
        false
    }
}

/// The last line of a regex error: the part that says what is wrong.
fn short_message(error: &regex::Error) -> String {
    let text = error.to_string();
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("syntax error");
    line.trim().trim_start_matches("error: ").to_owned()
}

/// Where a row's text sits in the line buffer, and how its bytes map to
/// columns.
struct Segment {
    byte_start: usize,
    row: usize,
    /// `None` when the row is ASCII with one byte per column.
    cells: Option<Vec<CellSpan>>,
}

struct CellSpan {
    /// Byte offset of the cell's text within the row's text.
    byte: usize,
    col: u16,
    /// Columns the cell covers: 2 for a wide character.
    span: u8,
}

/// The text of the logical line being collected.
#[derive(Default)]
struct LineBuf {
    text: String,
    segments: Vec<Segment>,
}

impl LineBuf {
    fn clear(&mut self) {
        self.text.clear();
        self.segments.clear();
    }

    /// Append `view` as row `row`. `ends_line` rows lose trailing blanks.
    fn push_row(&mut self, row: usize, view: &RowView<'_>, ends_line: bool) {
        let byte_start = self.text.len();
        let plain = view.text();
        if plain.is_ascii() && plain.len() == usize::from(view.width()) {
            self.text.push_str(&plain);
            if ends_line {
                let kept = self.text.trim_end_matches(' ').len().max(byte_start);
                self.text.truncate(kept);
            }
            self.segments.push(Segment {
                byte_start,
                row,
                cells: None,
            });
            return;
        }

        let mut cells = Vec::new();
        for col in 0..view.width() {
            let Some(cell) = view.cell(col) else { break };
            if cell.is_wide_spacer() {
                continue;
            }
            cells.push(CellSpan {
                byte: self.text.len() - byte_start,
                col,
                span: if cell.is_wide() { 2 } else { 1 },
            });
            self.text.push(cell.char());
            if let Some(extra) = view.grapheme_extra(col) {
                self.text.extend(extra);
            }
        }
        if ends_line {
            while let Some(last) = cells.last() {
                if &self.text[byte_start + last.byte..] != " " {
                    break;
                }
                self.text.truncate(byte_start + last.byte);
                cells.pop();
            }
        }
        self.segments.push(Segment {
            byte_start,
            row,
            cells: Some(cells),
        });
    }

    /// The row and column of the cell holding byte `at` of the line.
    /// Returns the cell's last column when `last` is set.
    fn cell_at(&self, at: usize, last: bool) -> (usize, usize) {
        let seg = self.segments.partition_point(|s| s.byte_start <= at) - 1;
        let segment = &self.segments[seg];
        let rel = at - segment.byte_start;
        let col = match &segment.cells {
            None => rel,
            Some(cells) => {
                let cell = &cells[cells.partition_point(|c| c.byte <= rel) - 1];
                usize::from(cell.col) + if last { usize::from(cell.span) - 1 } else { 0 }
            }
        };
        (segment.row, col)
    }

    fn find_into(&self, matcher: &Matcher, out: &mut Vec<SearchMatch>) {
        let Some(regex) = &matcher.regex else { return };
        if self.segments.is_empty() {
            return;
        }
        for found in regex.find_iter(&self.text) {
            if found.is_empty() {
                continue;
            }
            let (start_row, start_col) = self.cell_at(found.start(), false);
            let (end_row, end_col) = self.cell_at(found.end() - 1, true);
            out.push(SearchMatch {
                start_row,
                start_col,
                end_row,
                end_col,
            });
        }
    }
}

/// A scan of a buffer that runs in slices.
///
/// Each [`SearchJob::step`] holds the caller's borrow of the buffer for a
/// bounded time. Rows evicted between steps are accounted for, so the
/// matches stay valid for the buffer as it is at the last step.
pub struct SearchJob {
    matcher: Matcher,
    next_row: usize,
    evicted: usize,
    line: LineBuf,
    matches: Vec<SearchMatch>,
}

impl SearchJob {
    pub fn new(matcher: Matcher, buffer: &Buffer) -> Self {
        Self {
            matcher,
            next_row: 0,
            evicted: buffer.evicted_lines(),
            line: LineBuf::default(),
            matches: Vec::new(),
        }
    }

    /// Scan for up to `budget`. Returns true once the whole buffer is done.
    pub fn step(&mut self, buffer: &Buffer, budget: Duration) -> bool {
        self.rebase(buffer.evicted_lines());
        let started = Instant::now();
        let total = buffer.total_lines();
        let mut since_check = 0;
        while self.next_row < total {
            if since_check == ROWS_PER_CLOCK_CHECK {
                since_check = 0;
                if started.elapsed() >= budget {
                    return false;
                }
            }
            since_check += 1;
            let row = self.next_row;
            self.next_row += 1;
            let Some(view) = buffer.row_at(row) else {
                self.flush();
                continue;
            };
            let ends_line = !view.is_wrapped();
            self.line.push_row(row, &view, ends_line);
            if ends_line || self.line.text.len() > MAX_LINE_BYTES {
                self.flush();
            }
        }
        self.flush();
        true
    }

    fn flush(&mut self) {
        self.line.find_into(&self.matcher, &mut self.matches);
        self.line.clear();
    }

    /// Account for rows evicted from the front of history since the last
    /// step.
    fn rebase(&mut self, evicted_now: usize) {
        let dropped = evicted_now.saturating_sub(self.evicted);
        if dropped == 0 {
            return;
        }
        self.evicted = evicted_now;
        drop_evicted(&mut self.matches, dropped);
        if self.next_row < dropped {
            self.next_row = 0;
            self.line.clear();
            return;
        }
        self.next_row -= dropped;
        if self.line.segments.first().is_some_and(|s| s.row < dropped) {
            self.line.clear();
        }
        for segment in &mut self.line.segments {
            segment.row -= dropped;
        }
    }

    /// Rows scanned out of the rows there are, for a progress readout.
    pub fn percent_done(&self, buffer: &Buffer) -> u8 {
        let total = buffer.total_lines().max(1);
        (self.next_row.min(total) * 100 / total) as u8
    }

    /// The matches found, in order, and the buffer's eviction count they
    /// are relative to.
    pub fn into_matches(self) -> (Vec<SearchMatch>, usize) {
        (self.matches, self.evicted)
    }
}

/// Forget matches that began in the first `dropped` rows, and renumber the
/// rest. Returns how many were dropped. `matches` must be in order.
pub fn drop_evicted(matches: &mut Vec<SearchMatch>, dropped: usize) -> usize {
    let gone = matches.partition_point(|m| m.start_row < dropped);
    matches.drain(..gone);
    for m in matches.iter_mut() {
        m.start_row -= dropped;
        m.end_row -= dropped;
    }
    gone
}

/// Search the whole buffer at once. For tests and benchmarks; the app steps
/// a [`SearchJob`] instead.
pub fn search_all(buffer: &Buffer, matcher: Matcher) -> Vec<SearchMatch> {
    let mut job = SearchJob::new(matcher, buffer);
    while !job.step(buffer, Duration::from_secs(3600)) {}
    job.into_matches().0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Dimensions;

    const OPTS: SearchOptions = SearchOptions {
        regex: false,
        smart_case: true,
    };
    const REGEX: SearchOptions = SearchOptions {
        regex: true,
        smart_case: true,
    };

    fn buffer(cols: u16, rows: u16, history: usize) -> Buffer {
        Buffer::with_scrollback(Dimensions::new(cols, rows), history)
    }

    /// Print `text`; `\n` starts the next line at column 0.
    fn feed(buf: &mut Buffer, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                buf.newline();
                buf.grid_mut().cursor_mut().position.col = 0;
            } else {
                buf.write_char(c);
            }
        }
    }

    fn find(buf: &Buffer, query: &str, options: SearchOptions) -> Vec<SearchMatch> {
        search_all(buf, Matcher::new(query, options).unwrap())
    }

    fn span(m: &SearchMatch) -> ((usize, usize), (usize, usize)) {
        ((m.start_row, m.start_col), (m.end_row, m.end_col))
    }

    #[test]
    fn a_match_can_cross_a_soft_wrap() {
        let mut buf = buffer(10, 3, 100);
        feed(&mut buf, "abcdefghijklmno");
        assert!(buf.row_at(0).unwrap().is_wrapped());

        let found = find(&buf, "hijkl", OPTS);

        assert_eq!(found.len(), 1);
        assert_eq!(span(&found[0]), ((0, 7), (1, 1)));
    }

    #[test]
    fn a_hard_line_break_stops_a_match() {
        let mut buf = buffer(10, 3, 100);
        feed(&mut buf, "abc\ndef");
        assert!(find(&buf, "cd", OPTS).is_empty());
    }

    #[test]
    fn regex_mode_reads_the_query_as_a_pattern() {
        let mut buf = buffer(30, 3, 100);
        feed(&mut buf, "error 42 and error 7\nwarning 1");

        let found = find(&buf, r"error \d+", REGEX);

        assert_eq!(
            found.iter().map(span).collect::<Vec<_>>(),
            [((0, 0), (0, 7)), ((0, 13), (0, 19))]
        );
        // Without regex mode the same text is a literal and finds nothing.
        assert!(find(&buf, r"error \d+", OPTS).is_empty());
        // `$` sees the end of the logical line, not trailing blanks.
        assert_eq!(find(&buf, r"1$", REGEX).len(), 1);
    }

    #[test]
    fn smart_case_is_insensitive_until_the_query_has_a_capital() {
        let mut buf = buffer(30, 3, 100);
        feed(&mut buf, "foo Foo FOO");

        assert_eq!(find(&buf, "foo", OPTS).len(), 3);
        assert_eq!(find(&buf, "Foo", OPTS).len(), 1);
        assert_eq!(find(&buf, "FOO", OPTS).len(), 1);
        let sensitive = SearchOptions {
            smart_case: false,
            ..OPTS
        };
        assert_eq!(find(&buf, "foo", sensitive).len(), 1);
        // An escape such as `\S` is not a typed capital.
        assert_eq!(find(&buf, r"f\S\S", REGEX).len(), 3);
    }

    #[test]
    fn wide_and_combining_characters_map_to_cells() {
        let mut buf = buffer(20, 3, 100);
        // Columns: 日=0-1 本=2-3 e+accent=4 x=5 語=6-7
        feed(&mut buf, "日本e\u{301}x語");

        let x = find(&buf, "x", OPTS);
        assert_eq!(span(&x[0]), ((0, 5), (0, 5)));

        let wide = find(&buf, "本", OPTS);
        assert_eq!(span(&wide[0]), ((0, 2), (0, 3)));

        let cluster = find(&buf, "e\u{301}x", OPTS);
        assert_eq!(span(&cluster[0]), ((0, 4), (0, 5)));

        let last = find(&buf, "語", OPTS);
        assert_eq!(span(&last[0]), ((0, 6), (0, 7)));
    }

    #[test]
    fn a_match_in_evicted_history_goes_away() {
        let mut buf = buffer(10, 2, 3);
        feed(&mut buf, "needle\nb\nc\nd\ne");
        let mut found = find(&buf, "needle", OPTS);
        assert_eq!(found.len(), 1);
        let base = buf.evicted_lines();

        // Two more lines push the oldest history rows out.
        feed(&mut buf, "\nf\ng");
        let dropped = buf.evicted_lines() - base;
        assert!(dropped > 0);
        drop_evicted(&mut found, dropped);

        assert!(found.is_empty());
        assert!(find(&buf, "needle", OPTS).is_empty());
    }

    #[test]
    fn eviction_in_the_middle_of_a_scan_leaves_correct_rows() {
        let mut buf = buffer(20, 4, 400);
        for i in 0..600 {
            feed(&mut buf, &format!("n{i}\n"));
        }
        let mut job = SearchJob::new(Matcher::new("n", OPTS).unwrap(), &buf);
        // A zero budget stops at the first clock check, after 64 rows.
        assert!(!job.step(&buf, Duration::ZERO));
        for i in 600..650 {
            feed(&mut buf, &format!("n{i}\n"));
        }
        while !job.step(&buf, Duration::from_secs(10)) {}

        let (got, evicted) = job.into_matches();
        assert_eq!(evicted, buf.evicted_lines());
        assert_eq!(got, find(&buf, "n", OPTS));
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let mut buf = buffer(10, 2, 10);
        feed(&mut buf, "abc");
        assert!(find(&buf, "", OPTS).is_empty());
        assert!(find(&buf, "", REGEX).is_empty());
    }

    #[test]
    fn an_invalid_regex_is_an_error_not_a_panic() {
        let err = Matcher::new("(unclosed", REGEX).unwrap_err();
        assert!(matches!(err, SearchError::InvalidRegex(ref m) if !m.is_empty()));
        // The same text is fine as a literal.
        assert!(Matcher::new("(unclosed", OPTS).is_ok());
    }

    #[test]
    fn a_pattern_that_can_match_nothing_does_not_loop_or_match_empty() {
        let mut buf = buffer(10, 2, 10);
        feed(&mut buf, "aab");
        let found = find(&buf, "x*", REGEX);
        assert!(found.is_empty());
    }

    /// Search time on a big scrollback. Run with
    /// `cargo test -p ultty --release --lib search_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing report, not a check"]
    fn search_timing() {
        for lines in [10_000usize, 100_000, 1_000_000] {
            let mut buf = buffer(120, 40, lines);
            for i in 0..lines + 40 {
                feed(
                    &mut buf,
                    &format!(
                        "line {i} the quick brown fox jumps over the lazy dog ERROR code={}\n",
                        i % 97
                    ),
                );
            }
            for (name, query, options) in
                [("plain", "lazy dog", OPTS), ("regex", r"code=\d+7$", REGEX)]
            {
                let matcher = Matcher::new(query, options).unwrap();
                let started = Instant::now();
                let mut job = SearchJob::new(matcher, &buf);
                let mut worst_step = Duration::ZERO;
                let mut steps = 0;
                loop {
                    let t = Instant::now();
                    let done = job.step(&buf, Duration::from_millis(2));
                    worst_step = worst_step.max(t.elapsed());
                    steps += 1;
                    if done {
                        break;
                    }
                }
                println!(
                    "{lines} lines, {name}: {:?} total, {} matches, {steps} steps, worst step {worst_step:?}",
                    started.elapsed(),
                    job.into_matches().0.len()
                );
            }
        }
    }
}
