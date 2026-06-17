//! Token estimation — a verbatim port of v0.x `src/tokens.ts`.
//!
//! The estimator is `chars/4` rounded up, with a `max(1, …)` floor so a
//! non-empty string never costs 0 tokens. This is **load-bearing for
//! byte-equivalence**: the trim-to-budget walk makes keep/drop decisions
//! off this estimate, so any divergence changes *which lines are kept*,
//! not merely the reported count. See COMPAT.md §1.2.
//!
//! "chars" means JavaScript `String.length`, i.e. **UTF-16 code units**,
//! not Unicode scalar values and not bytes. For the BMP (and the ASCII
//! test corpus) one `char` == one UTF-16 unit; astral characters count
//! as 2. We replicate that with `chars(s)` below so estimates match Node
//! exactly even on emoji/CJK-extension input.

const DEFAULT_CHARS_PER_TOKEN: usize = 4;

/// Count UTF-16 code units in `s` — matches JavaScript `String.length`.
#[inline]
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Estimate tokens for a single string: `max(1, ceil(len/4))`, or 0 for
/// the empty string. Mirrors `HeuristicTokenModel.estimate`.
#[inline]
pub fn estimate(text: &str) -> usize {
    let len = utf16_len(text);
    if len == 0 {
        return 0;
    }
    // ceil(len / 4) without floats.
    len.div_ceil(DEFAULT_CHARS_PER_TOKEN).max(1)
}

/// Estimate tokens across many strings (sum of per-string estimates).
/// Mirrors `estimateMany` — note this is **not** `estimate(concat)`; each
/// string gets its own `max(1, …)` floor.
#[inline]
pub fn estimate_many<S: AsRef<str>>(texts: &[S]) -> usize {
    texts.iter().map(|t| estimate(t.as_ref())).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate(""), 0);
    }

    #[test]
    fn floor_is_one() {
        assert_eq!(estimate("a"), 1); // ceil(1/4) = 1
        assert_eq!(estimate("abcd"), 1); // ceil(4/4) = 1
        assert_eq!(estimate("abcde"), 2); // ceil(5/4) = 2
    }

    #[test]
    fn matches_js_ceil() {
        // 100 chars -> ceil(100/4) = 25
        let s = "x".repeat(100);
        assert_eq!(estimate(&s), 25);
        // 101 chars -> ceil(101/4) = 26
        let s = "x".repeat(101);
        assert_eq!(estimate(&s), 26);
    }

    #[test]
    fn estimate_many_sums_with_per_string_floor() {
        // Two 1-char strings cost 1 + 1 = 2, not ceil(2/4) = 1.
        let v = vec!["a", "b"];
        assert_eq!(estimate_many(&v), 2);
    }

    #[test]
    fn astral_counts_as_two_utf16_units() {
        // "😀" is one scalar value but two UTF-16 units -> length 2.
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(estimate("😀"), 1); // ceil(2/4) = 1
    }
}
