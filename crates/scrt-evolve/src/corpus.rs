//! Training-corpus export — the no-ML half of the spike.
//!
//! Thesis (from the spike spec): a palace of stashes built up during an
//! agent's daily work is a *curated retrieval signal*. Each stash is a
//! search the agent chose to keep: its **note** is a natural query, its
//! captured **nodes** are positives (the agent judged them relevant), and
//! nodes from OTHER stashes are negatives. That's a ready-made contrastive
//! dataset — no labeling, no LLM.
//!
//! This module turns a palace into JSONL rows `{query, positive_chunk,
//! negative_chunks[]}` with zero ML dependencies, so corpus export works in
//! a default build. The `train` feature consumes these rows.

use serde::{Deserialize, Serialize};

use scrt_core::palace::types::{Palace, StashedNode};

/// One contrastive training row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorpusRow {
    /// The query signal — the stash note (what the agent was looking for).
    pub query: String,
    /// A chunk the agent kept under that query (a positive).
    pub positive_chunk: String,
    /// Chunks from other stashes — presumed-irrelevant negatives.
    pub negative_chunks: Vec<String>,
    /// Provenance: which stash this row came from (for debugging/eval).
    pub stash: String,
}

/// Options for corpus construction.
#[derive(Debug, Clone, Copy)]
pub struct CorpusOptions {
    /// Negatives sampled per positive row.
    pub negatives_per_row: usize,
}

impl Default for CorpusOptions {
    fn default() -> Self {
        CorpusOptions {
            negatives_per_row: 4,
        }
    }
}

/// Render a stashed node into a single chunk string (the text an embedding
/// model would see): context_before + match + context_after, joined.
fn node_chunk(n: &StashedNode) -> String {
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(n.context_before.iter().map(String::as_str));
    parts.push(&n.match_text);
    parts.extend(n.context_after.iter().map(String::as_str));
    parts
        .into_iter()
        .map(str::trim_end)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the contrastive corpus from a palace.
///
/// For each stash with a non-empty note and ≥1 node, emit one row per node:
/// `query = note`, `positive = that node's chunk`, `negatives = up to N
/// chunks drawn from OTHER stashes`. Stashes with an empty note are skipped
/// (no query signal). Negative sampling is **deterministic** (index-strided,
/// no RNG dependency) so the corpus is reproducible.
pub fn build_corpus(palace: &Palace, opts: CorpusOptions) -> Vec<CorpusRow> {
    // Flatten all node chunks with their owning stash, for negative sampling.
    let mut all_chunks: Vec<(usize, String)> = Vec::new(); // (stash_idx, chunk)
    let stash_list: Vec<&str> = palace.stashes.keys().map(String::as_str).collect();
    for (si, (_name, stash)) in palace.stashes.iter().enumerate() {
        for n in &stash.nodes {
            all_chunks.push((si, node_chunk(n)));
        }
    }

    let mut rows = Vec::new();
    for (si, (name, stash)) in palace.stashes.iter().enumerate() {
        if stash.note.trim().is_empty() || stash.nodes.is_empty() {
            continue;
        }
        for (ni, node) in stash.nodes.iter().enumerate() {
            let positive = node_chunk(node);
            if positive.trim().is_empty() {
                continue;
            }
            // Sample negatives: chunks NOT from this stash, strided start so
            // different positives in the same stash draw different negatives.
            let mut negatives = Vec::new();
            if all_chunks.len() > 1 {
                let start = (si * 31 + ni * 17) % all_chunks.len();
                let mut idx = start;
                let mut scanned = 0;
                while negatives.len() < opts.negatives_per_row && scanned < all_chunks.len() {
                    let (cs, chunk) = &all_chunks[idx];
                    if *cs != si && !chunk.trim().is_empty() {
                        negatives.push(chunk.clone());
                    }
                    idx = (idx + 1) % all_chunks.len();
                    scanned += 1;
                }
            }
            rows.push(CorpusRow {
                query: stash.note.clone(),
                positive_chunk: positive,
                negative_chunks: negatives,
                stash: (*stash_list.get(si).unwrap_or(&name.as_str())).to_string(),
            });
        }
    }
    rows
}

/// Serialize corpus rows to JSONL (one JSON object per line).
pub fn to_jsonl(rows: &[CorpusRow]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&serde_json::to_string(r).expect("CorpusRow serializes"));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scrt_core::palace::ops::{add_stash, StashOptions, StashSearch, SystemClock};
    use scrt_core::types::{Node, Source};

    fn node(text: &str, line: u64) -> Node {
        Node {
            id: 1,
            source: Source::file("f.txt"),
            match_line: line,
            start_line: line,
            end_line: line,
            context_before: vec![],
            match_text: text.into(),
            context_after: vec![],
            match_spans: vec![[0, 1]],
            tokens: 1,
        }
    }

    fn meta() -> StashSearch {
        StashSearch {
            pattern: "p".into(),
            effort: "normal".into(),
            sources_count: 1,
        }
    }

    #[test]
    fn corpus_rows_have_query_positive_negatives() {
        let clock = SystemClock;
        let mut p = Palace::empty();
        add_stash(
            &mut p,
            &clock,
            "auth",
            "authentication TODOs",
            &[node("// TODO auth", 1)],
            meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        add_stash(
            &mut p,
            &clock,
            "db",
            "database notes",
            &[node("// db pool", 1), node("// db cache", 2)],
            meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();

        let rows = build_corpus(
            &p,
            CorpusOptions {
                negatives_per_row: 2,
            },
        );
        // 1 (auth) + 2 (db) nodes = 3 rows, each with a note query.
        assert_eq!(rows.len(), 3);
        let auth = rows.iter().find(|r| r.stash == "auth").unwrap();
        assert_eq!(auth.query, "authentication TODOs");
        assert!(auth.positive_chunk.contains("TODO auth"));
        // Negatives come from the db stash, not auth.
        assert!(auth.negative_chunks.iter().all(|n| n.contains("db")));
    }

    #[test]
    fn empty_note_stashes_skipped() {
        let clock = SystemClock;
        let mut p = Palace::empty();
        add_stash(
            &mut p,
            &clock,
            "noted",
            "has a note",
            &[node("x", 1)],
            meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        add_stash(
            &mut p,
            &clock,
            "blank",
            "",
            &[node("y", 1)],
            meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        let rows = build_corpus(&p, CorpusOptions::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].stash, "noted");
    }

    #[test]
    fn jsonl_is_one_object_per_line() {
        let clock = SystemClock;
        let mut p = Palace::empty();
        add_stash(
            &mut p,
            &clock,
            "a",
            "note a",
            &[node("x", 1)],
            meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        let rows = build_corpus(&p, CorpusOptions::default());
        let jsonl = to_jsonl(&rows);
        assert_eq!(jsonl.lines().count(), rows.len());
        // Each line parses.
        for line in jsonl.lines() {
            let _: CorpusRow = serde_json::from_str(line).unwrap();
        }
    }
}
