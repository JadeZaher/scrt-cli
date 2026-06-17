//! In-process search — the replacement for v0.x `src/rg.ts`.
//!
//! v0.x shelled out to `rg --json` and parsed the stream. scrt uses the
//! BurntSushi `grep` crates (the engine *behind* ripgrep) directly, so
//! there is no subprocess and no `rg` runtime dependency. This is the
//! whole point of the port (DESIGN.md §4.1).
//!
//! Match semantics are matched to v0.x exactly:
//!   - One `Match` per **submatch**. A line with two hits yields two
//!     `Match`es with identical `(source.id, line)`; the orchestrator
//!     dedups by that pair so each line is one node.
//!   - `text` is the matched line with its trailing `\n` / `\r\n`
//!     stripped, then clipped to 16 KB (`MAX_MATCH_TEXT_CHARS`).
//!   - `match_start` / `match_end` are byte offsets within `text`.
//!   - rg flags map onto `RegexMatcherBuilder` / `SearcherBuilder`.
//!
//! Pathological-input guards mirror rg.ts: oversized lines are clipped
//! rather than buffered unbounded.

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};

use crate::types::{Match, SearchOptions, Source, SourceType};

/// Hard cap on per-`Match.text` size pushed downstream (v0.x: 16 KB).
const MAX_MATCH_TEXT_CHARS: usize = 16 * 1024;

#[derive(Debug)]
pub enum SearchError {
    /// The pattern failed to compile.
    BadPattern(String),
    /// An I/O error reading a source.
    Io(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::BadPattern(m) => write!(f, "bad pattern: {m}"),
            SearchError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Build a `RegexMatcher` from a pattern + options, mapping the rg flags
/// v0.x forwarded (`-i`, `-w`, `-F`, `-U`/`--multiline-dotall`).
pub fn build_matcher(pattern: &str, opts: &SearchOptions) -> Result<RegexMatcher, SearchError> {
    let mut b = RegexMatcherBuilder::new();
    b.case_insensitive(opts.case_insensitive)
        .word(opts.word_match)
        .fixed_strings(opts.fixed_strings);
    if opts.multiline {
        // v0.x passes `-U --multiline-dotall`: allow `.` to cross lines
        // and the pattern to span lines.
        b.multi_line(true).dot_matches_new_line(true);
    }
    b.build(pattern).map_err(|e| SearchError::BadPattern(e.to_string()))
}

/// Build a `Searcher` configured to match rg's line model. Multiline
/// searches need `multi_line(true)` on the searcher too.
fn build_searcher(opts: &SearchOptions) -> Searcher {
    let mut sb = SearcherBuilder::new();
    sb.line_number(true);
    if opts.multiline {
        sb.multi_line(true);
    }
    sb.build()
}

/// Strip a single trailing `\r\n` or `\n` (v0.x `stripTrailingNewline`).
fn strip_trailing_newline(s: &str) -> &str {
    if let Some(t) = s.strip_suffix("\r\n") {
        t
    } else if let Some(t) = s.strip_suffix('\n') {
        t
    } else {
        s
    }
}

/// Clip oversized match text (v0.x `clipMatchText`): keep the first
/// `MAX-16` chars and append `…[clipped]`. Operates on bytes-as-prefix at
/// a char boundary to avoid splitting a UTF-8 sequence.
fn clip_match_text(s: &str) -> String {
    if s.len() <= MAX_MATCH_TEXT_CHARS {
        return s.to_string();
    }
    let head = MAX_MATCH_TEXT_CHARS - 16;
    let mut end = head.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[clipped]", &s[..end])
}

/// Sink that emits one `Match` per submatch on each matched line,
/// replicating rg's `submatches` array.
struct MatchSink<'m, F: FnMut(Match)> {
    matcher: &'m RegexMatcher,
    source: Source,
    emit: F,
}

impl<'m, F: FnMut(Match)> Sink for MatchSink<'m, F> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        sink_match: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        let line_number = sink_match.line_number().unwrap_or(0);
        let raw = sink_match.bytes();
        let raw_str = String::from_utf8_lossy(raw);
        let line_text = strip_trailing_newline(&raw_str);
        let clipped = clip_match_text(line_text);

        // Recover submatch spans within the (untrimmed) line bytes, then
        // clamp offsets into the clipped text. We search the raw matched
        // block so offsets are byte offsets relative to the line start.
        let line_bytes = line_text.as_bytes();
        let clipped_len = clipped.len();
        let mut at = 0usize;
        let mut emit_one = |start: usize, end: usize| {
            // Clamp to the clipped string length so spans stay valid when
            // the line was truncated.
            let s = start.min(clipped_len);
            let e = end.min(clipped_len);
            (self.emit)(Match {
                source: self.source.clone(),
                line: line_number,
                text: clipped.clone(),
                match_start: s,
                match_end: e,
            });
        };

        loop {
            match self.matcher.find_at(line_bytes, at) {
                Ok(Some(mat)) => {
                    emit_one(mat.start(), mat.end());
                    // Advance; guard against zero-width matches looping.
                    at = if mat.end() > at { mat.end() } else { at + 1 };
                    if at > line_bytes.len() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        Ok(true)
    }
}

/// Search a single in-memory content blob for `pattern`, invoking `on_match`
/// for each submatch. Used for command / url / stdin sources and for the
/// round-trip tests. `source` is attached to every emitted match.
pub fn search_content<F: FnMut(Match)>(
    matcher: &RegexMatcher,
    opts: &SearchOptions,
    source: Source,
    content: &str,
    on_match: F,
) -> Result<(), SearchError> {
    let mut searcher = build_searcher(opts);
    let mut sink = MatchSink { matcher, source, emit: on_match };
    searcher
        .search_slice(matcher, content.as_bytes(), &mut sink)
        .map_err(|e| SearchError::Io(e.to_string()))
}

/// Search a real file on disk for `pattern`. The emitted matches carry a
/// `file`-typed source with the file's path as id (the orchestrator
/// resolves to absolute before calling).
pub fn search_file<F: FnMut(Match)>(
    matcher: &RegexMatcher,
    opts: &SearchOptions,
    path: &str,
    on_match: F,
) -> Result<(), SearchError> {
    let source = Source { id: path.to_string(), source_type: SourceType::File, label: None };
    let mut searcher = build_searcher(opts);
    let mut sink = MatchSink { matcher, source, emit: on_match };
    searcher
        .search_path(matcher, path, &mut sink)
        .map_err(|e| SearchError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(pattern: &str, content: &str, opts: &SearchOptions) -> Vec<Match> {
        let m = build_matcher(pattern, opts).unwrap();
        let mut out = Vec::new();
        search_content(&m, opts, Source::file("t.txt"), content, |mm| out.push(mm)).unwrap();
        out
    }

    #[test]
    fn single_match_basic() {
        let ms = collect("TODO", "line1\nx TODO y\nline3", &SearchOptions::default());
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].line, 2);
        assert_eq!(ms[0].text, "x TODO y");
        assert_eq!(ms[0].match_start, 2);
        assert_eq!(ms[0].match_end, 6);
    }

    #[test]
    fn two_submatches_on_one_line() {
        // rg emits one record per submatch; we emit two Matches same line.
        let ms = collect("ab", "ab xx ab", &SearchOptions::default());
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].match_start, 0);
        assert_eq!(ms[1].match_start, 6);
        assert!(ms.iter().all(|m| m.line == 1 && m.text == "ab xx ab"));
    }

    #[test]
    fn case_insensitive_flag() {
        let opts = SearchOptions { case_insensitive: true, ..Default::default() };
        let ms = collect("todo", "Found a TODO here", &opts);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].match_start, 8);
    }

    #[test]
    fn fixed_strings_treats_pattern_literally() {
        let opts = SearchOptions { fixed_strings: true, ..Default::default() };
        // "a.b" as a literal should NOT match "axb".
        let ms = collect("a.b", "axb and a.b", &opts);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].match_start, 8);
    }

    #[test]
    fn word_match_flag() {
        let opts = SearchOptions { word_match: true, ..Default::default() };
        let ms = collect("cat", "category cat scatter", &opts);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].match_start, 9);
    }

    #[test]
    fn no_match_yields_empty() {
        let ms = collect("zzz", "nothing here", &SearchOptions::default());
        assert!(ms.is_empty());
    }

    #[test]
    fn crlf_line_strips_terminator() {
        let ms = collect("hit", "a\r\nhit here\r\nb", &SearchOptions::default());
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].text, "hit here"); // no trailing \r
    }
}
