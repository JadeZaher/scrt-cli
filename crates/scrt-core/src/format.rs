//! Output formatting — port of v0.x `src/format.ts`.
//!
//! `json` is `serde_json::to_string_pretty(result)` (mirrors Node's
//! `JSON.stringify(result, null, 2)`). `agent-json` lives in `envelope`.
//! `llm` / `markdown` / `text` are ported here.
//!
//! Branding: the `llm` block tag is `<scrt result …>` / `</scrt result>`
//! (v0.x: `<mpg result …>`). The parity harness normalizes `mpg`↔`scrt`
//! on the non-JSON formats before diffing (COMPAT.md §Branding).
//!
//! Color: scrt's formatters emit **no ANSI color** (color is auto-off when
//! not a TTY, which is every parity/CI run). The v0.x color path is purely
//! cosmetic and not part of the byte contract; the no-color rendering —
//! including the `**hit**` match highlight in `llm` — is what we match.

pub use crate::envelope::format_agent_json;
use crate::pagination::PaginationMeta;
use crate::types::{Effort, ResultStatus, SearchResult, Strategy};

/// Serialize a result as v0.x-compatible pretty JSON (`--format json`).
pub fn format_json(result: &SearchResult) -> String {
    serde_json::to_string_pretty(result).expect("SearchResult is always serializable")
}

/// The output format selected on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Llm,
    Markdown,
    Json,
    Text,
    AgentJson,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<OutputFormat> {
        Some(match s {
            "llm" => OutputFormat::Llm,
            "markdown" => OutputFormat::Markdown,
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            "agent-json" => OutputFormat::AgentJson,
            _ => return None,
        })
    }
}

fn effort_str(e: Effort) -> &'static str {
    match e {
        Effort::Scan => "scan",
        Effort::Quick => "quick",
        Effort::Normal => "normal",
        Effort::Deep => "deep",
        Effort::Auto => "auto",
    }
}

fn strategy_str(s: Strategy) -> &'static str {
    match s {
        Strategy::Fill => "fill",
        Strategy::Deep => "deep",
    }
}

fn status_str(s: ResultStatus) -> &'static str {
    match s {
        ResultStatus::Ok => "ok",
        ResultStatus::NoMatches => "no_matches",
        ResultStatus::Truncated => "truncated",
        ResultStatus::Error => "error",
    }
}

/// Compact pagination annotation (port of `paginationAnnotation`):
/// ` page=N of M page_size=K total_items=T`, or "" when absent.
fn pagination_annotation(meta: Option<&PaginationMeta>) -> String {
    match meta {
        None => String::new(),
        Some(m) => format!(
            " page={} of {} page_size={} total_items={}",
            m.page, m.total_pages, m.page_size, m.total_items
        ),
    }
}

/// Pagination nav note (port of `paginationTextNote`).
fn pagination_text_note(meta: &PaginationMeta) -> String {
    let mut nav = Vec::new();
    if meta.has_prev {
        nav.push("<- prev".to_string());
    }
    nav.push(format!("page {} of {}", meta.page, meta.total_pages));
    if meta.has_next {
        nav.push("next ->".to_string());
    }
    format!(
        "[{} | {} items | page_size={}]",
        nav.join("  "),
        meta.total_items,
        meta.page_size
    )
}

/// UTF-16-aware slice for the `llm`/`text`/`markdown` match-span split,
/// matching JS `string.slice(s, e)` (code-unit offsets). `match_spans`
/// offsets are UTF-16 units (clip mode) or byte offsets (line mode); for
/// the ASCII corpus these coincide. We clamp to char boundaries to stay
/// panic-free on multibyte input.
fn slice_units(s: &str, start: usize, end: usize) -> (&str, &str, &str) {
    let b = |off: usize| -> usize {
        let mut o = off.min(s.len());
        while o > 0 && !s.is_char_boundary(o) {
            o -= 1;
        }
        o
    };
    let so = b(start);
    let eo = b(end).max(so);
    (&s[..so], &s[so..eo], &s[eo..])
}

/// `--format llm` (default). Port of `formatLlm` (no-color path).
pub fn format_llm(result: &SearchResult) -> String {
    let mut out: Vec<String> = Vec::new();

    let mut header_parts: Vec<String> = vec![
        format!("pattern=\"{}\"", result.pattern),
        format!("status={}", status_str(result.status)),
        format!("nodes={}", result.total_nodes),
        format!("tokens=~{}", result.total_tokens),
    ];
    if result.page_tokens != result.total_tokens {
        header_parts.push(format!("page_tokens=~{}", result.page_tokens));
    }
    header_parts.push(format!("effort={}", effort_str(result.effort)));
    header_parts.push(format!("strategy={}", strategy_str(result.strategy)));
    let ann = pagination_annotation(result.pagination.as_ref());
    let mut header = header_parts.join(" ");
    if !ann.is_empty() {
        header.push_str(&ann);
    }
    out.push(format!("<scrt result {}>", header.trim()));

    for node in &result.nodes {
        out.push(String::new());
        out.push(format!(
            "--- NODE {} of {} | {}:{} | ~{} tokens ---",
            node.id, result.total_nodes, node.source.id, node.match_line, node.tokens
        ));

        let width = node.end_line.to_string().len();
        let pad = |n: u64| format!("{:>width$}", n, width = width);

        for (i, line) in node.context_before.iter().enumerate() {
            out.push(format!("{} {} {}", pad(node.start_line + i as u64), "  ", line));
        }
        // match line, with **hit** highlight (no-color path).
        let (before, hit, after) = if let Some([s, e]) = node.match_spans.first().copied() {
            slice_units(&node.match_text, s, e)
        } else {
            ("", node.match_text.as_str(), "")
        };
        out.push(format!(
            "{} {} {}**{}**{}",
            pad(node.match_line),
            ">>",
            before,
            hit,
            after
        ));
        for (i, line) in node.context_after.iter().enumerate() {
            out.push(format!("{} {} {}", pad(node.match_line + 1 + i as u64), "  ", line));
        }
    }

    out.push(String::new());
    out.push("--- TOTAL ---".to_string());
    out.push(format!(
        "{} node{} | ~{} tokens | {} source{} | {}ms",
        result.total_nodes,
        plural(result.total_nodes),
        result.total_tokens,
        result.sources_count,
        plural(result.sources_count),
        result.duration_ms
    ));
    if result.truncated {
        out.push("(truncated: hit --max-tokens budget)".to_string());
    }
    if let Some(p) = &result.pagination {
        let nav = if p.has_next {
            " (more pages available — pass --page N)"
        } else {
            ""
        };
        out.push(format!("{}{}", pagination_text_note(p), nav));
    }
    out.push("</scrt result>".to_string());
    out.join("\n")
}

/// `--format text`. Port of `formatText` (no-color path).
pub fn format_text(result: &SearchResult) -> String {
    let mut out: Vec<String> = Vec::new();
    for node in &result.nodes {
        out.push(String::new());
        out.push(format!("{}:{}", node.source.id, node.match_line));
        for (i, line) in node.context_before.iter().enumerate() {
            out.push(format!("{}  {}", node.start_line + i as u64, line));
        }
        out.push(format!("{}  {}", node.match_line, node.match_text));
        for (i, line) in node.context_after.iter().enumerate() {
            out.push(format!("{}  {}", node.match_line + 1 + i as u64, line));
        }
        out.push(format!("~{} tokens", node.tokens));
    }
    if result.truncated {
        out.push(String::new());
        out.push("[truncated: total token budget reached]".to_string());
    }
    out.join("\n")
}

/// `--format markdown`. Port of `formatMarkdown` (no-color path uses
/// `_N tokens_`).
pub fn format_markdown(result: &SearchResult) -> String {
    let mut out: Vec<String> = Vec::new();
    for node in &result.nodes {
        out.push(format!("### `{}` line {}", node.source.id, node.match_line));
        out.push(String::new());
        let lang = infer_lang(&node.source.id);
        let mut lines: Vec<String> = Vec::new();
        for (i, line) in node.context_before.iter().enumerate() {
            lines.push(format!("{}  {}", node.start_line + i as u64, line));
        }
        lines.push(format!("**{}**  {}", node.match_line, node.match_text));
        for (i, line) in node.context_after.iter().enumerate() {
            lines.push(format!("{}  {}", node.match_line + 1 + i as u64, line));
        }
        out.push(format!("```{lang}"));
        out.push(lines.join("\n"));
        out.push("```".to_string());
        out.push(String::new());
        out.push(format!("_{} tokens_", node.tokens));
        out.push(String::new());
    }
    if result.truncated {
        out.push("> ⚠ Truncated to fit token budget.".to_string());
    }
    out.join("\n")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Port of `inferLang` — file extension → markdown code fence language.
fn infer_lang(path: &str) -> &'static str {
    let ends = |suf: &str| path.ends_with(suf);
    if ends(".ts") || ends(".tsx") {
        "typescript"
    } else if ends(".js") || ends(".jsx") {
        "javascript"
    } else if ends(".py") {
        "python"
    } else if ends(".rs") {
        "rust"
    } else if ends(".go") {
        "go"
    } else if ends(".md") {
        "markdown"
    } else if ends(".json") {
        "json"
    } else if ends(".yaml") || ends(".yml") {
        "yaml"
    } else if ends(".sh") || ends(".bash") {
        "bash"
    } else if ends(".sql") {
        "sql"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Node, Source};

    fn result() -> SearchResult {
        SearchResult {
            pattern: "TODO".into(),
            effort: Effort::Normal,
            strategy: Strategy::Fill,
            status: ResultStatus::Ok,
            total_nodes: 1,
            total_tokens: 5,
            page_tokens: 5,
            sources_count: 1,
            truncated: false,
            nodes: vec![Node {
                id: 1,
                source: Source::file("a.ts"),
                match_line: 2,
                start_line: 1,
                end_line: 3,
                context_before: vec!["before".into()],
                match_text: "x TODO y".into(),
                context_after: vec!["after".into()],
                match_spans: vec![[2, 6]],
                tokens: 5,
            }],
            duration_ms: 0,
            before_tokens: 500,
            after_tokens: 500,
            auto_tune_applied: None,
            max_nodes: 30,
            max_tokens: None,
            pagination: None,
        }
    }

    #[test]
    fn llm_block_is_scrt_branded() {
        let s = format_llm(&result());
        assert!(s.starts_with("<scrt result "));
        assert!(s.ends_with("</scrt result>"));
        assert!(s.contains("status=ok"));
        assert!(s.contains(">> x **TODO** y"));
        assert!(s.contains("--- TOTAL ---"));
        assert!(s.contains("1 node | ~5 tokens | 1 source | 0ms"));
    }

    #[test]
    fn text_format_shape() {
        let s = format_text(&result());
        assert!(s.contains("a.ts:2"));
        assert!(s.contains("2  x TODO y"));
        assert!(s.contains("~5 tokens"));
    }

    #[test]
    fn markdown_infers_lang_and_no_color_tokens() {
        let s = format_markdown(&result());
        assert!(s.contains("### `a.ts` line 2"));
        assert!(s.contains("```typescript"));
        assert!(s.contains("**2**  x TODO y"));
        assert!(s.contains("_5 tokens_"));
    }
}
