//! agent-json envelope — port of `buildAgentEnvelope` in v0.x
//! `src/format.ts`. The structured control-loop primitive: an agent can
//! detect a bad pattern from `status` / `warning` / `next_suggestion`
//! without spending an LLM round-trip. The schema is load-bearing
//! (COMPAT.md §3); field order matches the v0.x object literal.

use serde::{Deserialize, Serialize};

use crate::types::{Node, SearchResult};

/// Status in the agent envelope. Superset of `ResultStatus` — adds
/// `partial`, which appears ONLY here, never in raw `Result.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeStatus {
    Ok,
    NoMatches,
    Truncated,
    Partial,
    Error,
}

/// What fallback the engine resorted to, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FallbackUsed {
    Fuzzy,
    Fill,
}

/// The agent-json envelope. Field order = v0.x literal: status, pattern,
/// n_literal_matches, n_fuzzy_matches, fallback_used, warning, nodes,
/// next_suggestion, errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEnvelope {
    pub status: EnvelopeStatus,
    pub pattern: String,
    pub n_literal_matches: usize,
    pub n_fuzzy_matches: usize,
    /// `null` | "fuzzy" | "fill".
    pub fallback_used: Option<FallbackUsed>,
    pub warning: Option<String>,
    pub nodes: Vec<Node>,
    pub next_suggestion: Option<String>,
    /// Always present (possibly empty). Mirrors v0.x `errors: [...] ?? []`.
    pub errors: Vec<serde_json::Value>,
}

/// Inputs the orchestrator threads into the envelope builder.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvelopeOpts {
    pub no_fill: bool,
    pub fuzzy_fired: bool,
    /// Literal match count; when `None`, defaults to `result.total_nodes`.
    pub literal_match_count: Option<usize>,
}

/// Map a raw `ResultStatus` to the envelope status namespace.
fn base_status(s: crate::types::ResultStatus) -> EnvelopeStatus {
    use crate::types::ResultStatus::*;
    match s {
        Ok => EnvelopeStatus::Ok,
        NoMatches => EnvelopeStatus::NoMatches,
        Truncated => EnvelopeStatus::Truncated,
        Error => EnvelopeStatus::Error,
    }
}

/// Build the envelope from a result. Direct port of `buildAgentEnvelope`.
pub fn build_agent_envelope(result: &SearchResult, opts: EnvelopeOpts) -> AgentEnvelope {
    let literal_matches = opts.literal_match_count.unwrap_or(result.total_nodes);

    // Status: Result status is authoritative, but force "truncated" for a
    // truncated-but-non-empty result (unless error).
    let mut status = base_status(result.status);
    if result.truncated && !result.nodes.is_empty() && status != EnvelopeStatus::Error {
        status = EnvelopeStatus::Truncated;
    }

    let fallback_used = if opts.fuzzy_fired {
        Some(FallbackUsed::Fuzzy)
    } else if !opts.no_fill && !result.nodes.is_empty() && result.total_nodes == result.max_nodes {
        Some(FallbackUsed::Fill)
    } else {
        None
    };

    // Warning + next_suggestion heuristics (exact branch order from v0.x).
    let mut warning: Option<String> = None;
    let mut next_suggestion: Option<String> = None;

    if literal_matches == 0 && opts.fuzzy_fired {
        warning = Some("literal pattern matched nothing; fell back to fuzzy".to_string());
        next_suggestion = Some("try rephrasing or use simpler keywords".to_string());
    } else if literal_matches == 0 && !opts.fuzzy_fired {
        status = EnvelopeStatus::NoMatches;
        warning = None;
        next_suggestion = Some("no matches — try simpler keywords or check the path".to_string());
    } else if literal_matches > 50 && result.truncated {
        warning = Some(format!(
            "pattern matched {literal_matches} lines; consider narrowing"
        ));
    } else if opts.no_fill && result.nodes.len() < result.max_nodes {
        warning = Some(format!(
            "strict mode: only {} usable matches found (max_nodes={})",
            result.nodes.len(),
            result.max_nodes
        ));
    }

    AgentEnvelope {
        status,
        pattern: result.pattern.clone(),
        n_literal_matches: if opts.fuzzy_fired { 0 } else { literal_matches },
        n_fuzzy_matches: if opts.fuzzy_fired {
            result.total_nodes
        } else {
            0
        },
        fallback_used,
        warning,
        nodes: result.nodes.clone(),
        next_suggestion,
        errors: Vec::new(),
    }
}

/// Serialize the envelope as pretty JSON (`--format agent-json`):
/// `JSON.stringify(envelope, null, 2)`.
pub fn format_agent_json(envelope: &AgentEnvelope) -> String {
    serde_json::to_string_pretty(envelope).expect("AgentEnvelope is always serializable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Effort, ResultStatus, Strategy};

    fn result_with(
        total_nodes: usize,
        max_nodes: usize,
        truncated: bool,
        status: ResultStatus,
    ) -> SearchResult {
        SearchResult {
            pattern: "p".into(),
            effort: Effort::Normal,
            strategy: Strategy::Fill,
            status,
            total_nodes,
            total_tokens: 0,
            page_tokens: 0,
            sources_count: 0,
            truncated,
            nodes: Vec::new(),
            duration_ms: 0,
            before_tokens: 500,
            after_tokens: 500,
            auto_tune_applied: None,
            max_nodes,
            max_tokens: None,
            pagination: None,
        }
    }

    #[test]
    fn no_matches_sets_status_and_suggestion() {
        let r = result_with(0, 30, false, ResultStatus::NoMatches);
        let e = build_agent_envelope(&r, EnvelopeOpts::default());
        assert_eq!(e.status, EnvelopeStatus::NoMatches);
        assert_eq!(e.n_literal_matches, 0);
        assert!(e.warning.is_none());
        assert_eq!(
            e.next_suggestion.as_deref(),
            Some("no matches — try simpler keywords or check the path")
        );
        assert!(e.fallback_used.is_none());
    }

    #[test]
    fn fuzzy_fired_flips_counts_and_fallback() {
        let mut r = result_with(3, 30, false, ResultStatus::Ok);
        r.total_nodes = 3;
        let e = build_agent_envelope(
            &r,
            EnvelopeOpts {
                fuzzy_fired: true,
                literal_match_count: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(e.fallback_used, Some(FallbackUsed::Fuzzy));
        assert_eq!(e.n_literal_matches, 0);
        assert_eq!(e.n_fuzzy_matches, 3);
        assert_eq!(
            e.warning.as_deref(),
            Some("literal pattern matched nothing; fell back to fuzzy")
        );
    }

    #[test]
    fn fill_flagged_when_at_max_nodes() {
        let mut r = result_with(30, 30, false, ResultStatus::Ok);
        r.nodes = vec![sample_node()];
        let e = build_agent_envelope(&r, EnvelopeOpts::default());
        assert_eq!(e.fallback_used, Some(FallbackUsed::Fill));
    }

    #[test]
    fn no_fill_strict_warning() {
        let mut r = result_with(5, 30, false, ResultStatus::Ok);
        r.nodes = vec![sample_node()];
        let e = build_agent_envelope(
            &r,
            EnvelopeOpts {
                no_fill: true,
                ..Default::default()
            },
        );
        assert!(e
            .warning
            .as_deref()
            .unwrap()
            .starts_with("strict mode: only"));
        assert!(e.fallback_used.is_none()); // fill suppressed by no_fill
    }

    #[test]
    fn envelope_key_order() {
        let r = result_with(0, 30, false, ResultStatus::NoMatches);
        let e = build_agent_envelope(&r, EnvelopeOpts::default());
        let pretty = format_agent_json(&e);
        // The first keys in declaration order.
        let want = [
            "status",
            "pattern",
            "n_literal_matches",
            "n_fuzzy_matches",
            "fallback_used",
            "warning",
            "nodes",
            "next_suggestion",
            "errors",
        ];
        let mut idx = 0;
        for key in want {
            let needle = format!("\"{key}\"");
            let pos = pretty.find(&needle).expect("key present");
            assert!(pos >= idx, "key {key} out of order");
            idx = pos;
        }
    }

    fn sample_node() -> Node {
        Node {
            id: 1,
            source: crate::types::Source::file("x"),
            match_line: 1,
            start_line: 1,
            end_line: 1,
            context_before: vec![],
            match_text: "x".into(),
            context_after: vec![],
            match_spans: vec![[0, 1]],
            tokens: 1,
        }
    }
}
