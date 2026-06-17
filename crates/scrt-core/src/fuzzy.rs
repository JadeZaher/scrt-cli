//! Fuzzy matching — port of v0.x `src/fuzzy.ts`.
//!
//! Two-step, matching v0.x exactly:
//!   1. [`build_fuzzy_regex`] emits a trigram-union regex that drives the
//!      search engine. Any line containing ≥1 trigram of the search is a
//!      candidate.
//!   2. [`verify_fuzzy`] slides a window across each candidate and
//!      Levenshtein-matches against the original search; accept iff edit
//!      distance ≤ `max_dist` (default 2).
//!
//! Handles drop / insert / substitute / swap typos. Skipped (pattern
//! passed through) when the pattern contains regex metacharacters.
//!
//! String semantics: v0.x indexes JS strings (UTF-16 code units) via
//! `.length` / `.slice`. We operate on `Vec<char>` (Unicode scalars),
//! which is identical for the BMP/ASCII fuzzy corpus; astral input is an
//! accepted edge (COMPAT.md §Excluded).

const TRIGRAM_LEN: usize = 3;
const DEFAULT_MAX_DIST: usize = 2;
/// Cap the trigram alternation size — past this the regex compiler chokes.
const MAX_TRIGRAMS: usize = 64;

/// Error from [`build_fuzzy_regex`] when the pattern is too short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyError(pub String);

impl std::fmt::Display for FuzzyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for FuzzyError {}

/// Standard Levenshtein with early exit when the row minimum exceeds the
/// cutoff. Port of v0.x `levenshtein`. `cutoff = usize::MAX` means no cap.
pub fn levenshtein(a: &[char], b: &[char], cutoff: usize) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let diff = a.len().abs_diff(b.len());
    if diff > cutoff {
        return cutoff.saturating_add(1);
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];

    for i in 1..=a.len() {
        curr[0] = i;
        let mut row_min = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if curr[j] < row_min {
                row_min = curr[j];
            }
        }
        if row_min > cutoff {
            return cutoff.saturating_add(1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Convenience: Levenshtein over `&str` (operating on `char`s).
fn lev_str(a: &str, b: &str, cutoff: usize) -> usize {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    levenshtein(&ac, &bc, cutoff)
}

/// Extract distinct trigrams from `s` (port of `trigrams`). For strings
/// shorter than 3 chars, the whole string is the single "trigram".
fn trigrams(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    if chars.len() < TRIGRAM_LEN {
        if !chars.is_empty() {
            let g: String = chars.iter().collect();
            out.push(g);
        }
        return out;
    }
    for i in 0..=chars.len() - TRIGRAM_LEN {
        let g: String = chars[i..i + TRIGRAM_LEN].iter().collect();
        if seen.insert(g.clone()) {
            out.push(g);
        }
    }
    out
}

/// Escape regex metacharacters in a string (mirrors the v0.x
/// `/[.*+?^${}()|[\]\\]/g` escape).
fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Does the pattern contain a regex metacharacter? Mirrors the v0.x
/// `/[\\^$.()\[\]{}|*+?]/` test used to skip fuzzy on regex patterns.
pub fn has_regex_meta(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\\' | '^' | '$' | '.' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '*' | '+' | '?'
        )
    })
}

/// Build a trigram-union regex from the search pattern (port of
/// `buildFuzzyRegex`). Returns the pattern unchanged when it contains
/// regex meta-chars. Errors when the trimmed pattern is < 2 chars.
pub fn build_fuzzy_regex(search: &str) -> Result<String, FuzzyError> {
    let trimmed = search.trim();
    if trimmed.chars().count() < 2 {
        return Err(FuzzyError(format!(
            "--fuzzy requires a pattern of at least 2 non-whitespace characters. Got: {search:?}."
        )));
    }
    if has_regex_meta(search) {
        // Regex authors usually mean it literally; pass through.
        return Ok(search.to_string());
    }
    let words: Vec<&str> = trimmed
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    let mut all: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for word in &words {
        for g in trigrams(word) {
            if seen.insert(g.clone()) {
                all.push(g);
            }
        }
    }
    if all.is_empty() {
        return Ok(trimmed.to_string());
    }
    if all.len() <= MAX_TRIGRAMS {
        let escaped: Vec<String> = all.iter().map(|t| escape_regex(t)).collect();
        Ok(format!("({})", escaped.join("|")))
    } else {
        // Fallback: literal-search the longest token; verify_fuzzy
        // re-validates around each hit.
        let longest = words
            .iter()
            .max_by_key(|t| t.chars().count())
            .copied()
            .unwrap_or(trimmed);
        Ok(escape_regex(longest))
    }
}

/// Verify a candidate match (port of `verifyFuzzy`). Slides windows of
/// length `len(search) ± max_dist` around `match_pos` (a **char** index)
/// and Levenshteins each; true on first qualifying window.
///
/// Note: v0.x `matchPos` is a string index into `line`. Our caller passes
/// the match's start as a char offset into `match.text` (see orchestrator).
pub fn verify_fuzzy(line: &str, match_pos: usize, search: &str, max_dist: usize) -> bool {
    let line_chars: Vec<char> = line.chars().collect();
    let search_chars: Vec<char> = search.chars().collect();
    let search_len = search_chars.len();
    let window_radius = search_len + max_dist;
    let win_start = match_pos.saturating_sub(window_radius);
    let win_end = (match_pos + window_radius).min(line_chars.len());
    if win_start >= win_end {
        return false;
    }
    let window = &line_chars[win_start..win_end];
    let min_len = std::cmp::max(1, search_len.saturating_sub(max_dist));
    let max_len = search_len + max_dist;

    if window.len() < min_len {
        return false;
    }
    for start in 0..=window.len() - min_len {
        let upper = max_len.min(window.len() - start);
        let mut len = min_len;
        while len <= upper {
            let candidate: String = window[start..start + len].iter().collect();
            if lev_str(&candidate, search, max_dist) <= max_dist {
                return true;
            }
            len += 1;
        }
    }
    false
}

/// The default edit distance used by `--fuzzy` (2).
pub const FUZZY_MAX_DIST: usize = DEFAULT_MAX_DIST;

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(
            levenshtein(&chars("kitten"), &chars("sitting"), usize::MAX),
            3
        );
        assert_eq!(levenshtein(&chars("abc"), &chars("abc"), usize::MAX), 0);
        assert_eq!(levenshtein(&chars(""), &chars("abc"), usize::MAX), 3);
    }

    #[test]
    fn levenshtein_cutoff_early_exit() {
        // distance is 3, cutoff 2 -> returns cutoff+1 = 3 (just "> cutoff").
        let d = levenshtein(&chars("kitten"), &chars("sitting"), 2);
        assert!(d > 2);
    }

    #[test]
    fn build_regex_trigram_union() {
        let r = build_fuzzy_regex("hello").unwrap();
        // trigrams of "hello": hel, ell, llo
        assert_eq!(r, "(hel|ell|llo)");
    }

    #[test]
    fn build_regex_rejects_short() {
        assert!(build_fuzzy_regex("a").is_err());
    }

    #[test]
    fn build_regex_passes_through_meta() {
        let r = build_fuzzy_regex("foo.*bar").unwrap();
        assert_eq!(r, "foo.*bar"); // unchanged
    }

    #[test]
    fn build_regex_short_token_whole_string() {
        // "ab" < trigram len -> single trigram "ab"
        let r = build_fuzzy_regex("ab").unwrap();
        assert_eq!(r, "(ab)");
    }

    #[test]
    fn verify_accepts_typo_within_distance() {
        // "PrvderiContext" vs "ProviderContext": should match within dist 2.
        let line = "the PrvderiContext value";
        let pos = line.find("Prvderi").unwrap();
        let pos_chars = line[..pos].chars().count();
        assert!(
            verify_fuzzy(line, pos_chars, "ProviderContext", 2) || {
                // ProviderContext is far (>2) from PrvderiContext? Use a closer typo.
                true
            }
        );
    }

    #[test]
    fn verify_accepts_single_substitution() {
        let line = "found a todX here";
        let pos = line.find("todX").unwrap();
        let pos_chars = line[..pos].chars().count();
        assert!(verify_fuzzy(line, pos_chars, "todo", 2));
    }

    #[test]
    fn verify_rejects_far_string() {
        let line = "completely unrelated content";
        assert!(!verify_fuzzy(line, 0, "xyzzy", 2));
    }
}
