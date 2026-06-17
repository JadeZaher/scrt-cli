//! Core types — port of v0.x `src/types.ts`.
//!
//! These serialize to the exact JSON shapes in COMPAT.md. Field
//! **declaration order** is significant: serde emits fields in
//! declaration order and `--format json` is byte-diffed against Node's
//! `JSON.stringify`, which emits in object-literal order. Do not reorder
//! fields without updating COMPAT.md and the golden fixtures.

use serde::{Deserialize, Serialize};

/// Where text came from. Mirrors the v0.x `SourceType` union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    File,
    Command,
    Stdin,
    Url,
    Bulk,
}

/// `Source` — a stable identifier plus its kind. `label` is omitted when
/// absent (Node leaves it `undefined`, which `JSON.stringify` drops).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Stable id: file path, `cmd:<command>`, URL, or "stdin".
    pub id: String,
    #[serde(rename = "type")]
    pub source_type: SourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Source {
    pub fn file(id: impl Into<String>) -> Self {
        Source { id: id.into(), source_type: SourceType::File, label: None }
    }
}

/// A single line match produced by the search engine. One `Match` per
/// **submatch** (a line with two hits yields two `Match`es), matching
/// rg's `--json` submatch stream; the orchestrator dedups by
/// `(source.id, line)` so each line contributes at most one node.
///
/// `match_start` / `match_end` are **byte offsets** into `text` (rg
/// reports byte offsets; we keep that so spans line up with UTF-8 input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub source: Source,
    /// 1-indexed line number of the match.
    pub line: u64,
    /// The matched line's text, trailing newline stripped.
    pub text: String,
    /// 0-indexed start byte offset of the match within `text`.
    pub match_start: usize,
    /// 0-indexed end byte offset (exclusive) of the match within `text`.
    pub match_end: usize,
}

/// A "node": a match wrapped in a token-budgeted context window. The
/// smallest unit the LLM consumes. Serializes exactly as COMPAT.md §1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// 1-indexed position within the result set (assigned by orchestrator).
    pub id: u64,
    pub source: Source,
    /// 1-indexed line number of the match.
    pub match_line: u64,
    /// 1-indexed first context line (`start_line <= match_line`).
    pub start_line: u64,
    /// 1-indexed last context line (`end_line >= match_line`).
    pub end_line: u64,
    /// Pre-context lines (text only, no line numbers).
    pub context_before: Vec<String>,
    /// The matched line.
    pub match_text: String,
    /// Post-context lines.
    pub context_after: Vec<String>,
    /// Highlight ranges within `match_text` (offsets from its start).
    /// Each pair is `[start, end)`.
    pub match_spans: Vec<[usize; 2]>,
    /// Estimated token count for the whole node.
    pub tokens: usize,
}

/// Effort preset. `auto` is a valid value (resolves to the `normal`
/// bundle); see COMPAT.md §2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Scan,
    Quick,
    Normal,
    Deep,
    Auto,
}

/// How `--max-tokens` is spent across nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Fill,
    Deep,
}

/// Node ordering by source mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Default,
    Recent,
    Oldest,
}

/// Per-node window decay across ranks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCurve {
    Flat,
    Linear,
    Log,
}

/// Raw `Result.status`. Note: `partial` exists only in the agent-json
/// envelope (Prompt 3), never here. See COMPAT.md §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Ok,
    NoMatches,
    Truncated,
    Error,
}

/// Top-level search result. Serializes exactly as COMPAT.md §2.
///
/// **Field order is the v0.x object-literal order** (index.ts ~line 468),
/// NOT the `types.ts` interface order — `JSON.stringify` follows
/// insertion order. serde emits in declaration order, so these fields are
/// declared in the literal order to make `--format json` byte-match. Do
/// not reorder without re-capturing the golden fixtures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub pattern: String,
    pub effort: Effort,
    pub strategy: Strategy,
    pub status: ResultStatus,
    /// Pre-pagination total node count.
    pub total_nodes: usize,
    /// Pre-pagination total token count.
    pub total_tokens: usize,
    /// Tokens of the returned (paginated) nodes.
    pub page_tokens: usize,
    pub sources_count: usize,
    pub truncated: bool,
    pub nodes: Vec<Node>,
    /// Wall-clock duration in ms. EXCLUDED from parity diff (COMPAT.md §Excluded).
    pub duration_ms: u64,
    pub before_tokens: usize,
    pub after_tokens: usize,
    /// Present only when the wide-record auto-tune fired (emitted between
    /// after_tokens and max_nodes in v0.x).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_tune_applied: Option<bool>,
    pub max_nodes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// Present only when pagination is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::pagination::PaginationMeta>,
}

/// Options forwarded to the search engine (the v0.x `RgOptions`).
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub case_insensitive: bool,
    pub word_match: bool,
    pub fixed_strings: bool,
    pub multiline: bool,
    pub hidden: bool,
    pub no_ignore: bool,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub type_filter: Option<String>,
    pub glob_case_insensitive: bool,
    /// Max columns a line may have before it's reported as a preview only.
    /// Default 1_000_000 (matches v0.x `DEFAULT_MAX_COLUMNS`).
    pub max_columns: Option<usize>,
}

/// The v0.x `Result` object-literal key order, kept as a guard: a unit
/// test serializes a populated `SearchResult` and asserts the emitted key
/// sequence equals this slice, so an accidental field reorder fails CI
/// rather than silently breaking the byte diff.
#[cfg(test)]
pub(crate) const V0X_RESULT_KEY_ORDER: &[&str] = &[
    "pattern",
    "effort",
    "strategy",
    "status",
    "total_nodes",
    "total_tokens",
    "page_tokens",
    "sources_count",
    "truncated",
    "nodes",
    "duration_ms",
    "before_tokens",
    "after_tokens",
    "auto_tune_applied",
    "max_nodes",
    "max_tokens",
    "pagination",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node() -> Node {
        Node {
            id: 1,
            source: Source::file("a.txt"),
            match_line: 2,
            start_line: 1,
            end_line: 3,
            context_before: vec!["before".into()],
            match_text: "match".into(),
            context_after: vec!["after".into()],
            match_spans: vec![[0, 5]],
            tokens: 4,
        }
    }

    /// Extract top-level JSON object keys in their serialized (string)
    /// order. We parse the *pretty string* (which preserves declaration
    /// order) rather than `to_value` (whose Map alphabetizes without the
    /// `preserve_order` feature). Byte-diffing uses the string, so this is
    /// the order that actually matters.
    fn top_level_keys(pretty: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let mut depth: i32 = 0;
        let mut in_str = false;
        let mut esc = false;
        let mut cur = String::new();
        let mut chars = pretty.chars().peekable();
        while let Some(c) = chars.next() {
            if in_str {
                if esc {
                    cur.push(c);
                    esc = false;
                } else if c == '\\' {
                    cur.push(c);
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                    // Capture only depth-1 keys followed by a ':'.
                    if depth == 1 {
                        // peek past whitespace for ':'
                        let mut look = chars.clone();
                        while matches!(look.peek(), Some(c) if c.is_whitespace()) {
                            look.next();
                        }
                        if look.peek() == Some(&':') {
                            keys.push(cur.clone());
                        }
                    }
                    cur.clear();
                } else {
                    cur.push(c);
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        keys
    }

    #[test]
    fn result_key_order_matches_v0x_literal() {
        let r = SearchResult {
            pattern: "x".into(),
            effort: Effort::Normal,
            strategy: Strategy::Fill,
            status: ResultStatus::Ok,
            total_nodes: 1,
            total_tokens: 4,
            page_tokens: 4,
            sources_count: 1,
            truncated: false,
            nodes: vec![sample_node()],
            duration_ms: 0,
            before_tokens: 500,
            after_tokens: 500,
            auto_tune_applied: Some(true),
            max_nodes: 30,
            max_tokens: Some(8000),
            pagination: None, // None -> skipped
        };
        let pretty = serde_json::to_string_pretty(&r).unwrap();
        let keys = top_level_keys(&pretty);
        let expected: Vec<String> = V0X_RESULT_KEY_ORDER
            .iter()
            .copied()
            .filter(|k| *k != "pagination")
            .map(String::from)
            .collect();
        assert_eq!(keys, expected, "\npretty:\n{pretty}");
    }

    #[test]
    fn source_label_omitted_when_none() {
        let s = Source::file("a.txt");
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("label"), "label must be omitted when None: {json}");
    }

    #[test]
    fn node_serializes_in_declaration_order() {
        let pretty = serde_json::to_string_pretty(&sample_node()).unwrap();
        let keys = top_level_keys(&pretty);
        assert_eq!(
            keys,
            vec![
                "id", "source", "match_line", "start_line", "end_line",
                "context_before", "match_text", "context_after", "match_spans", "tokens",
            ]
        );
    }
}
