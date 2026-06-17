//! Search orchestrator — port of the main search path in v0.x
//! `src/index.ts` (steps 5–10), minus mind-palace wiring (Prompt 4) and
//! fuzzy (Prompt 3, layered on later).
//!
//! Sequence, matching v0.x exactly:
//!   5.  resolve inputs to sources
//!   6.  run search per source; dedup by (source.id, line); cap at max_nodes
//!   6b. optional sort by source mtime
//!   6c. window-curve decay
//!   7.  total token budget
//!   8.  assign 1-indexed ids
//!   9.  pagination (slice AFTER budgeting; re-id within page)
//!   10. assemble Result with status
//!
//! Effort presets are resolved here from `EFFORT_PRESETS` (COMPAT.md §2.1).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::nodes::{apply_total_budget, apply_window_curve, build_node, BuildNodeOptions};
use crate::pagination::{paginate, PaginationOptions};
use crate::search::{build_matcher, search_content, search_file, SearchError};
use crate::sources::{
    capture_command, classify_path_specs, command_source, ResolvedSource, SourceInput,
};
use crate::types::{
    Effort, Match, Node, ResultStatus, SearchOptions, SearchResult, SortMode, Source, SourceType,
    Strategy, WindowCurve,
};

/// `EFFORT_PRESETS` from v0.x cli.ts — (before, after, max_nodes).
pub fn effort_preset(effort: Effort) -> (usize, usize, usize) {
    match effort {
        Effort::Scan => (20, 20, 100_000),
        Effort::Quick => (200, 200, 10),
        Effort::Normal => (500, 500, 30),
        Effort::Deep => (2000, 2000, 100),
        Effort::Auto => (500, 500, 30), // aliases normal
    }
}

/// Fully resolved search configuration, mirroring the search-relevant
/// fields of v0.x `ResolvedConfig`.
pub struct SearchConfig {
    pub pattern: String,
    pub inputs: Vec<SourceInput>,
    pub effort: Effort,
    pub strategy: Strategy,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub max_nodes: usize,
    pub max_tokens: Option<usize>,
    pub clip_chars: Option<usize>,
    pub sort: SortMode,
    pub window_curve: WindowCurve,
    pub rg_options: SearchOptions,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub all: bool,
    /// --fuzzy: typo-tolerant search (trigram regex + Levenshtein filter).
    pub fuzzy: bool,
    /// Pre-read stdin content, threaded so @- and stdin source share one read.
    pub stdin_content: Option<String>,
}

/// Side-channel metadata the agent-json envelope needs but the raw
/// `SearchResult` doesn't carry. Returned by [`search_with_meta`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchMeta {
    /// True iff `--fuzzy` was on AND actually transformed the pattern
    /// (i.e. it didn't pass through unchanged due to regex metachars).
    pub fuzzy_fired: bool,
    /// Count of literal (pre-fuzzy) matches — for the envelope's
    /// `n_literal_matches` / warning heuristics. Equals `total_nodes`
    /// when fuzzy didn't fire.
    pub literal_match_count: usize,
}

impl SearchConfig {
    /// Build a config from a pattern + inputs at a given effort, filling
    /// before/after/max_nodes from the preset. Convenience for the debug
    /// binary and tests; the real CLI parser lands in Prompt 6.
    pub fn from_effort(pattern: impl Into<String>, inputs: Vec<SourceInput>, effort: Effort) -> Self {
        let (b, a, n) = effort_preset(effort);
        SearchConfig {
            pattern: pattern.into(),
            inputs,
            effort,
            strategy: Strategy::Fill,
            before_tokens: b,
            after_tokens: a,
            max_nodes: n,
            max_tokens: None,
            clip_chars: None,
            sort: SortMode::Default,
            window_curve: WindowCurve::Flat,
            rg_options: SearchOptions::default(),
            page: None,
            page_size: None,
            all: false,
            fuzzy: false,
            stdin_content: None,
        }
    }
}

/// Resolve `inputs` to concrete sources, capturing command/url/stdin content.
fn resolve_inputs(
    inputs: &[SourceInput],
    opts: &SearchOptions,
    stdin_content: Option<&str>,
) -> Result<Vec<ResolvedSource>, String> {
    let mut out = Vec::new();

    let path_specs: Vec<String> = inputs
        .iter()
        .filter_map(|i| match i {
            SourceInput::Path(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    if !path_specs.is_empty() {
        let files = classify_path_specs(&path_specs, opts, stdin_content)?;
        for f in files {
            out.push(ResolvedSource {
                source: Source { id: f, source_type: SourceType::File, label: None },
                content: None,
            });
        }
    }

    for input in inputs {
        match input {
            SourceInput::Command(cmd) => {
                let content = capture_command(cmd)?;
                out.push(ResolvedSource { source: command_source(cmd), content: Some(content) });
            }
            SourceInput::Stdin => {
                out.push(ResolvedSource {
                    source: Source { id: "stdin".into(), source_type: SourceType::Stdin, label: None },
                    content: Some(stdin_content.unwrap_or("").to_string()),
                });
            }
            #[cfg(feature = "url-source")]
            SourceInput::Url(url) => {
                let content = crate::sources::capture_url(url)?;
                out.push(ResolvedSource {
                    source: Source { id: url.clone(), source_type: SourceType::Url, label: None },
                    content: Some(content),
                });
            }
            #[cfg(not(feature = "url-source"))]
            SourceInput::Url(_) => {
                return Err("URL sources require the `url-source` feature".into());
            }
            SourceInput::Path(_) => {} // handled above
        }
    }
    Ok(out)
}

/// Run a search to completion. Thin wrapper over [`search_with_meta`]
/// that drops the side-channel metadata (used by callers that only emit
/// `--format json`).
pub fn search(config: &SearchConfig) -> Result<SearchResult, SearchError> {
    Ok(search_with_meta(config)?.0)
}

/// Run a search and also return the agent-json side-channel metadata
/// (`fuzzy_fired`, `literal_match_count`). Port of the v0.x index.ts main
/// search path including the `--fuzzy` transform + post-filter.
pub fn search_with_meta(config: &SearchConfig) -> Result<(SearchResult, SearchMeta), SearchError> {
    let t0 = Instant::now();

    let resolved = resolve_inputs(&config.inputs, &config.rg_options, config.stdin_content.as_deref())
        .map_err(SearchError::Io)?;

    // Fuzzy: transform the pattern into a trigram-union regex. It "fires"
    // only if the transform actually changed the pattern (regex-meta
    // patterns pass through unchanged and are treated as a normal search).
    let (effective_pattern, fuzzy_fired) = if config.fuzzy {
        match crate::fuzzy::build_fuzzy_regex(&config.pattern) {
            Ok(p) => {
                let fired = p != config.pattern;
                (p, fired)
            }
            Err(e) => return Err(SearchError::BadPattern(e.0)),
        }
    } else {
        (config.pattern.clone(), false)
    };

    let matcher = build_matcher(&effective_pattern, &config.rg_options)?;

    // ── Phase 1: collect matches per source ─────────────────────────────
    // The expensive work (read + regex over potentially thousands of files)
    // runs in PARALLEL across file sources via rayon — this is where rg's
    // speed comes from. Each source produces its own `Vec<Match>`; results
    // are kept in the ORIGINAL source order (par_iter().map() preserves
    // index order), so output is deterministic run-to-run. Content sources
    // (cmd/url/stdin) are cheap and searched inline in the same pass.
    use rayon::prelude::*;
    let per_source: Vec<Vec<Match>> = resolved
        .par_iter()
        .map(|rs| {
            let mut matches: Vec<Match> = Vec::new();
            {
                let collect = |m: Match| matches.push(m);
                let res = match (&rs.source.source_type, &rs.content) {
                    (_, Some(content)) => {
                        search_content(&matcher, &config.rg_options, rs.source.clone(), content, collect)
                    }
                    (SourceType::File, None) => {
                        search_file(&matcher, &config.rg_options, &rs.source.id, collect)
                    }
                    (_, None) => Ok(()),
                };
                // A per-source search error (bad file etc.) drops that source
                // (v0.x "continue"); a bad pattern can't occur here since the
                // matcher already compiled.
                if res.is_err() {
                    return Vec::new();
                }
            }
            matches
        })
        .collect();

    // ── Phase 2: dedup + node construction (serial, deterministic) ──────
    // Applies the same (source.id, line) dedup, the fuzzy post-filter, the
    // per-source content cache, and the `max_nodes` early break as before —
    // in source order, so semantics are byte-identical to the serial path.
    let mut all_nodes: Vec<Node> = Vec::new();
    let mut seen_lines: HashSet<String> = HashSet::new();
    let mut content_cache: HashMap<String, String> = HashMap::new();

    'outer: for (rs, matches) in resolved.iter().zip(per_source.into_iter()) {
        for m in matches {
            if all_nodes.len() >= config.max_nodes {
                break 'outer;
            }
            if fuzzy_fired {
                let char_pos = m.text[..m.match_start.min(m.text.len())].chars().count();
                if !crate::fuzzy::verify_fuzzy(
                    &m.text,
                    char_pos,
                    &config.pattern,
                    crate::fuzzy::FUZZY_MAX_DIST,
                ) {
                    continue;
                }
            }
            let line_key = format!("{}:{}", m.source.id, m.line);
            if seen_lines.contains(&line_key) {
                continue;
            }
            seen_lines.insert(line_key);

            let content: String = match &rs.content {
                Some(c) => c.clone(),
                None => content_cache
                    .entry(m.source.id.clone())
                    .or_insert_with(|| std::fs::read_to_string(&m.source.id).unwrap_or_default())
                    .clone(),
            };

            let node = build_node(
                &m,
                &content,
                &BuildNodeOptions {
                    before_tokens: config.before_tokens,
                    after_tokens: config.after_tokens,
                    clip_chars: config.clip_chars,
                },
            );
            all_nodes.push(node);
            if all_nodes.len() >= config.max_nodes {
                break 'outer;
            }
        }
    }

    // 6b. Sort by source mtime.
    if matches!(config.sort, SortMode::Recent | SortMode::Oldest) {
        sort_by_mtime(&mut all_nodes, config.sort);
    }

    // 6c. Window-curve decay.
    if !matches!(config.window_curve, WindowCurve::Flat) {
        apply_window_curve(
            &mut all_nodes,
            config.window_curve,
            config.before_tokens,
            config.after_tokens,
        );
    }

    // 7. Total token budget.
    let (mut budgeted, truncated) = apply_total_budget(all_nodes, config.max_tokens);

    // 8. Assign 1-indexed ids over the full budgeted set.
    for (i, n) in budgeted.iter_mut().enumerate() {
        n.id = (i + 1) as u64;
    }

    // Pre-pagination totals (status & metadata reflect the full set).
    let total_nodes = budgeted.len();
    let total_tokens: usize = budgeted.iter().map(|n| n.tokens).sum();
    let result_sources: HashSet<&str> = budgeted.iter().map(|n| n.source.id.as_str()).collect();
    let sources_count = result_sources.len();

    // 9. Pagination — slice AFTER budgeting, re-id within page.
    let (mut paged, pagination) = paginate(
        budgeted,
        PaginationOptions { page: config.page, page_size: config.page_size, all: config.all },
    );
    for (i, n) in paged.iter_mut().enumerate() {
        n.id = (i + 1) as u64;
    }
    let page_tokens: usize = paged.iter().map(|n| n.tokens).sum();

    // 10. Status.
    let status = if total_nodes == 0 {
        ResultStatus::NoMatches
    } else if truncated {
        ResultStatus::Truncated
    } else {
        ResultStatus::Ok
    };

    let result = SearchResult {
        pattern: config.pattern.clone(),
        effort: config.effort,
        strategy: config.strategy,
        status,
        total_nodes,
        total_tokens,
        page_tokens,
        sources_count,
        truncated,
        nodes: paged,
        duration_ms: t0.elapsed().as_millis() as u64,
        before_tokens: config.before_tokens,
        after_tokens: config.after_tokens,
        auto_tune_applied: None, // wide-record auto-tune: deferred (DESIGN §7)
        max_nodes: config.max_nodes,
        max_tokens: config.max_tokens,
        pagination,
    };

    // `literal_match_count` mirrors v0.x's envelope default of
    // `result.total_nodes`; when fuzzy fired the envelope zeroes it itself.
    let meta = SearchMeta { fuzzy_fired, literal_match_count: total_nodes };
    Ok((result, meta))
}

/// Port of v0.x step 6b. Sort nodes by source file mtime; non-file
/// sources sort to the extreme end; ties break by ascending match_line.
fn sort_by_mtime(nodes: &mut [Node], sort: SortMode) {
    use std::time::UNIX_EPOCH;
    let mut mtimes: HashMap<String, f64> = HashMap::new();
    for n in nodes.iter() {
        mtimes.entry(n.source.id.clone()).or_insert_with(|| {
            if n.source.source_type == SourceType::File {
                std::fs::metadata(&n.source.id)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0)
            } else if matches!(sort, SortMode::Recent) {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        });
    }
    let dir = if matches!(sort, SortMode::Recent) { -1.0 } else { 1.0 };
    nodes.sort_by(|a, b| {
        let ma = *mtimes.get(&a.source.id).unwrap_or(&0.0);
        let mb = *mtimes.get(&b.source.id).unwrap_or(&0.0);
        if ma != mb {
            (dir * (ma - mb)).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.match_line.cmp(&b.match_line)
        }
    });
}
