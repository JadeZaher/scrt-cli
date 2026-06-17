//! Node construction — port of v0.x `src/nodes.ts`.
//!
//! Given a `Match` and the full content of its source, build a `Node`:
//! the matched line plus a pre/post context window sized in tokens. The
//! windows grow greedily outward from the match line so they fit the
//! budget — budgeting in *tokens*, not lines, is what distinguishes this
//! from `rg -C N`.
//!
//! Every algorithm here is a line-for-line port; the trim walk's
//! direction order (right-first, alternating) and the clip-mode char
//! slicing (UTF-16 code units, to match JS `String.slice`) are
//! load-bearing for byte-equivalence. See COMPAT.md §1.

use crate::tokens::{estimate, estimate_many};
use crate::types::{Match, Node, WindowCurve};

/// Options controlling how a node's window is built.
pub struct BuildNodeOptions {
    pub before_tokens: usize,
    pub after_tokens: usize,
    /// Sub-line clip mode. When `Some(n)`, drop line context and trim the
    /// match line to `n` chars each side of the matched span.
    pub clip_chars: Option<usize>,
}

// ── UTF-16-aware slicing, to mirror JavaScript String semantics ─────────
//
// JS `String.length` / `.slice()` operate on UTF-16 code units. rg's
// match offsets are UTF-8 **byte** offsets. v0.x clip mode mixes the two:
// `match.text.slice(match_start - N, match_end + N)` uses byte-derived
// indices as code-unit indices. For ASCII that's identical; for non-ASCII
// the two diverge, and we must reproduce v0.x's (byte-index-as-codeunit)
// behavior to stay byte-equivalent on its corpus. We therefore convert
// the line to a UTF-16 unit vector and index it the same way v0.x does,
// clamping the byte offsets into code-unit space first.

/// Encode `s` as a vector of UTF-16 code units.
fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Decode a UTF-16 unit slice back to a `String` (lossy on lone
/// surrogates, which JS would keep — acceptable; such input is not in the
/// parity corpus and is documented in COMPAT.md §Excluded as a unicode edge).
fn from_utf16(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// Map a UTF-8 byte offset within `s` to a UTF-16 code-unit offset.
/// rg gives byte offsets; the JS code treats `match.text.length` (a
/// UTF-16 count) as the bound, so we translate to keep clamping identical.
fn byte_to_utf16_offset(s: &str, byte_off: usize) -> usize {
    let clamped = byte_off.min(s.len());
    // Count UTF-16 units in the prefix [0, clamped). If clamped lands
    // inside a multibyte char, floor to the char boundary (rg never emits
    // mid-char offsets, so this only guards against bad input).
    let mut prefix_end = clamped;
    while prefix_end > 0 && !s.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    s[..prefix_end].chars().map(char::len_utf16).sum()
}

/// Build a single context node from a match. Direct port of `buildNode`.
pub fn build_node(m: &Match, content: &str, opts: &BuildNodeOptions) -> Node {
    if let Some(n) = opts.clip_chars {
        let units = to_utf16(&m.text);
        let line_len = units.len(); // UTF-16 length == JS String.length
        let start_u = byte_to_utf16_offset(&m.text, m.match_start);
        let end_u = byte_to_utf16_offset(&m.text, m.match_end);

        let start_char = start_u.saturating_sub(n);
        let end_char = (end_u + n).min(line_len);

        let head = if start_char > 0 { "…" } else { "" };
        let tail = if end_char < line_len { "…" } else { "" };
        let mid = from_utf16(&units[start_char..end_char]);
        let clipped = format!("{head}{mid}{tail}");

        // Re-anchor the span to the clipped string. `head.length` in JS is
        // the UTF-16 length of the ellipsis ("…" is 1 unit). We measure in
        // UTF-16 units to match, but `match_spans` offsets are consumed as
        // string offsets downstream; we emit them as UTF-16 unit offsets
        // exactly as v0.x does.
        let head_len = utf16_units(head);
        let new_start = head_len + (start_u - start_char);
        let new_end = new_start + (end_u - start_u);

        return Node {
            id: 0,
            source: m.source.clone(),
            match_line: m.line,
            start_line: m.line,
            end_line: m.line,
            context_before: Vec::new(),
            match_text: clipped.clone(),
            context_after: Vec::new(),
            match_spans: vec![[new_start, new_end]],
            tokens: estimate(&clipped),
        };
    }

    // ── Line-context mode ───────────────────────────────────────────────
    // v0.x: content.split("\n"). We replicate exactly — split on '\n'
    // only (NOT '\r\n'); a trailing '\r' stays attached to the line, as
    // it does in JS. This matters for byte-equivalence on CRLF files.
    let all_lines: Vec<&str> = content.split('\n').collect();

    let match_index = if all_lines.is_empty() {
        0
    } else {
        ((m.line as i64) - 1).clamp(0, all_lines.len() as i64 - 1) as usize
    };

    // v0.x passes `beforeLines` to `trimAnchoredAtStart` WITHOUT reversing
    // (despite the misleading comment in nodes.ts). The anchor is therefore
    // index 0 = the FIRST line of the file slice, and the walk grows toward
    // the match. We replicate the *code*, not the comment, to stay byte-
    // equivalent. The kept result is already in original order.
    let before_lines = &all_lines[..match_index];
    let before_trim = trim_anchored_at_start(before_lines, opts.before_tokens);

    let after_lines = if match_index < all_lines.len() {
        &all_lines[(match_index + 1).min(all_lines.len())..]
    } else {
        &[][..]
    };
    let after_trim = trim_anchored_at_start(after_lines, opts.after_tokens);

    // start_line = matchIndex - keptBefore + 1 (1-indexed), floored at 1.
    let start_line = (match_index as i64) - (before_trim.kept.len() as i64) + 1;
    let end_line = (match_index + after_trim.kept.len() + 1) as u64;

    let tokens = before_trim.spent + estimate(&m.text) + after_trim.spent;

    Node {
        id: 0,
        source: m.source.clone(),
        match_line: m.line,
        start_line: start_line.max(1) as u64,
        end_line,
        context_before: before_trim.kept,
        match_text: m.text.clone(),
        context_after: after_trim.kept,
        match_spans: vec![[m.match_start, m.match_end]],
        tokens,
    }
}

fn utf16_units(s: &str) -> usize {
    s.encode_utf16().count()
}

struct Trim {
    kept: Vec<String>,
    spent: usize,
}

/// Port of `trimAnchoredAtStart`: anchor at index 0 (the line nearest the
/// match), grow outward, **alternate starting with `right`** (further from
/// anchor), stop expanding a side once a line would overflow the budget.
/// Returns kept lines in original order.
fn trim_anchored_at_start(lines: &[&str], budget: usize) -> Trim {
    if budget == 0 || lines.is_empty() {
        return Trim {
            kept: Vec::new(),
            spent: 0,
        };
    }
    let n = lines.len();
    let mut kept: Vec<Option<&str>> = vec![None; n];
    kept[0] = Some(lines[0]);
    let mut spent = estimate(lines[0]);

    // left/right as signed cursors; -1 / n mean "side exhausted".
    let mut left: i64 = 0;
    let mut right: i64 = 1;
    let mut try_right_first = true;

    while left >= 0 || right < n as i64 {
        let can_l = left >= 0;
        let can_r = right < n as i64;
        if !can_l && !can_r {
            break;
        }
        let take_right = if !can_l {
            true
        } else if !can_r {
            false
        } else {
            try_right_first
        };

        if take_right {
            let cost = estimate(lines[right as usize]);
            if spent + cost > budget {
                right = n as i64; // stop expanding this side
            } else {
                kept[right as usize] = Some(lines[right as usize]);
                spent += cost;
                right += 1;
            }
        } else {
            let cost = estimate(lines[left as usize]);
            if spent + cost > budget {
                left = -1; // stop expanding this side
            } else {
                kept[left as usize] = Some(lines[left as usize]);
                spent += cost;
                left -= 1;
            }
        }
        try_right_first = !try_right_first;
    }

    let result: Vec<String> = kept.into_iter().flatten().map(str::to_owned).collect();
    Trim {
        kept: result,
        spent,
    }
}

// ── Window-decay curve ──────────────────────────────────────────────────

/// Port of `applyWindowCurve` — shrink later nodes' windows in place.
pub fn apply_window_curve(
    nodes: &mut [Node],
    mode: WindowCurve,
    base_before: usize,
    base_after: usize,
) {
    if matches!(mode, WindowCurve::Flat) || nodes.is_empty() {
        return;
    }
    let total = nodes.len();
    for (i, node) in nodes.iter_mut().enumerate() {
        let ratio = curve_ratio(i, total, mode);
        let target_before = (base_before as f64 * ratio).floor().max(0.0) as usize;
        let target_after = (base_after as f64 * ratio).floor().max(0.0) as usize;
        trim_node_context(node, target_before, target_after);
    }
}

/// Port of `curveRatio`. `linear`: `max(0.1, 1 - rank/(total-1)*0.9)`.
/// `log`: `1 / log2(rank + 2)`.
fn curve_ratio(rank: usize, total: usize, mode: WindowCurve) -> f64 {
    match mode {
        WindowCurve::Linear => {
            if total <= 1 {
                1.0
            } else {
                (1.0 - (rank as f64 / (total as f64 - 1.0)) * 0.9).max(0.1)
            }
        }
        WindowCurve::Log => 1.0 / ((rank as f64 + 2.0).log2()),
        WindowCurve::Flat => 1.0,
    }
}

/// Port of `trimNodeContext`: drop outer lines until each side is under
/// its target, then recompute tokens.
fn trim_node_context(node: &mut Node, target_before: usize, target_after: usize) {
    while !node.context_before.is_empty() && estimate_many(&node.context_before) > target_before {
        node.context_before.remove(0);
        node.start_line = node.start_line.saturating_add(1).min(node.match_line);
    }
    while !node.context_after.is_empty() && estimate_many(&node.context_after) > target_after {
        node.context_after.pop();
        node.end_line = node.end_line.saturating_sub(1).max(node.match_line);
    }
    node.tokens = estimate_many(&node.context_before)
        + estimate(&node.match_text)
        + estimate_many(&node.context_after);
}

// ── Total token budget ──────────────────────────────────────────────────

/// Port of `applyTotalBudget`. Greedy keep-until-exhausted; both `fill`
/// and `deep` use the same trim in v1 (v0.x `void strategy`).
pub fn apply_total_budget(nodes: Vec<Node>, max_tokens: Option<usize>) -> (Vec<Node>, bool) {
    let max_tokens = match max_tokens {
        Some(m) if m > 0 => m,
        _ => return (nodes, false),
    };
    let total: usize = nodes.iter().map(|n| n.tokens).sum();
    if total <= max_tokens {
        return (nodes, false);
    }
    let original_len = nodes.len();
    let mut kept = Vec::new();
    let mut spent = 0usize;
    for n in nodes {
        if spent + n.tokens > max_tokens {
            break;
        }
        spent += n.tokens;
        kept.push(n);
    }
    let truncated = kept.len() < original_len;
    (kept, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Source;

    fn m(text: &str, line: u64, start: usize, end: usize) -> Match {
        Match {
            source: Source::file("t.txt"),
            line,
            text: text.into(),
            match_start: start,
            match_end: end,
        }
    }

    #[test]
    fn clip_mode_basic() {
        // line "abcdefghij", match "ef" at bytes 4..6, clip 2 chars each side.
        let mat = m("abcdefghij", 1, 4, 6);
        let n = build_node(
            &mat,
            "abcdefghij",
            &BuildNodeOptions {
                before_tokens: 500,
                after_tokens: 500,
                clip_chars: Some(2),
            },
        );
        // start_char = 4-2 = 2 ('c'), end_char = 6+2 = 8 ('h'), both interior
        // => head "…" + "cdefgh" + "…" tail.
        assert_eq!(n.match_text, "…cdefgh…");
        assert!(n.context_before.is_empty());
        assert!(n.context_after.is_empty());
        assert_eq!(n.start_line, 1);
        assert_eq!(n.end_line, 1);
        // span: head_len(1) + (start_u - start_char) = 1 + (4-2) = 3; len 2.
        assert_eq!(n.match_spans, vec![[3, 5]]);
    }

    #[test]
    fn clip_mode_no_ellipsis_at_edges() {
        // match at very start, clip wide enough to reach both ends.
        let mat = m("abc", 1, 0, 1);
        let n = build_node(
            &mat,
            "abc",
            &BuildNodeOptions {
                before_tokens: 0,
                after_tokens: 0,
                clip_chars: Some(10),
            },
        );
        assert_eq!(n.match_text, "abc"); // no ellipsis either side
        assert_eq!(n.match_spans, vec![[0, 1]]);
    }

    #[test]
    fn line_context_window() {
        let content = "l1\nl2\nl3\nMATCH\nl5\nl6";
        let mat = m("MATCH", 4, 0, 5);
        let n = build_node(
            &mat,
            content,
            &BuildNodeOptions {
                before_tokens: 500,
                after_tokens: 500,
                clip_chars: None,
            },
        );
        assert_eq!(n.match_line, 4);
        assert_eq!(n.match_text, "MATCH");
        // Generous budget keeps all surrounding lines.
        assert_eq!(n.context_before, vec!["l1", "l2", "l3"]);
        assert_eq!(n.context_after, vec!["l5", "l6"]);
        assert_eq!(n.start_line, 1);
        assert_eq!(n.end_line, 6);
    }

    #[test]
    fn budget_truncates_from_end() {
        let mk = |tokens: usize, id: u64| Node {
            id,
            source: Source::file("t"),
            match_line: 1,
            start_line: 1,
            end_line: 1,
            context_before: vec![],
            match_text: "x".into(),
            context_after: vec![],
            match_spans: vec![[0, 1]],
            tokens,
        };
        let nodes = vec![mk(10, 1), mk(10, 2), mk(10, 3)];
        let (kept, truncated) = apply_total_budget(nodes, Some(25));
        assert_eq!(kept.len(), 2); // 10+10 <= 25, third would hit 30
        assert!(truncated);
    }

    #[test]
    fn budget_none_keeps_all() {
        let nodes: Vec<Node> = vec![];
        let (kept, truncated) = apply_total_budget(nodes, None);
        assert_eq!(kept.len(), 0);
        assert!(!truncated);
    }
}
