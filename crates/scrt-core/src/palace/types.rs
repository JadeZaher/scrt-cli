//! Mind-palace data types — port of the on-disk structs in v0.x
//! `src/mind-palace.ts`. These serialize **byte-for-byte** to the v0.x
//! palace file so a file written by either implementation opens in both
//! (COMPAT.md §4). Field declaration order = the v0.x object-literal order
//! in `addStash` / `stashNodes`; do not reorder without re-capturing the
//! migration golden.

use serde::{Deserialize, Serialize};

/// On-disk format version. v0.x `PALACE_VERSION = 2`.
pub const PALACE_VERSION: u32 = 2;
pub const DEFAULT_PALACE_FILENAME: &str = "mind-palace.json";
pub const DEFAULT_PALACE_DIR: &str = ".mpg";

/// The whole palace file: `{ version, stashes }`.
///
/// `stashes` is an **insertion-ordered** map. v0.x stores it as a plain JS
/// object, whose `JSON.stringify` emits keys in insertion order. serde_json
/// without `preserve_order` would re-sort a `BTreeMap`/`Map`, so we use an
/// ordered map (`IndexMap`) to preserve the on-disk key order exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palace {
    pub version: u32,
    pub stashes: indexmap::IndexMap<String, Stash>,
}

impl Default for Palace {
    fn default() -> Self {
        Palace {
            version: PALACE_VERSION,
            stashes: indexmap::IndexMap::new(),
        }
    }
}

impl Palace {
    pub fn empty() -> Self {
        Palace::default()
    }
}

/// A stashed search result. Field order matches the v0.x `addStash`
/// object literal: name, note, tags, created_at, updated_at, expires_at,
/// search, sources, nodes, file_paths, relations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stash {
    pub name: String,
    pub note: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    /// ISO timestamp or `null`.
    pub expires_at: Option<String>,
    pub search: StashSearch,
    pub sources: Vec<String>,
    pub nodes: Vec<StashedNode>,
    pub file_paths: Vec<String>,
    pub relations: Vec<StashRelation>,
}

/// The `search` sub-object: pattern, effort, sources_count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashSearch {
    pub pattern: String,
    pub effort: String,
    pub sources_count: usize,
}

/// A directed edge between stashes: target, type, note, created_at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashRelation {
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    pub note: String,
    pub created_at: String,
}

/// A compact, self-contained snapshot of a `Node`. Field order matches the
/// v0.x `stashNodes` literal. `source_mtime_ms` / `match_line_hash` are
/// **omitted** (not null) when absent — `skip_serializing_if` reproduces
/// v0.x, where the keys are simply not assigned for non-file/legacy nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashedNode {
    pub source: String,
    /// Canonical fs path, or null if the source is not a file.
    pub file_path: Option<String>,
    pub source_type: String,
    pub match_line: u64,
    pub start_line: u64,
    pub end_line: u64,
    pub context_before: Vec<String>,
    pub match_text: String,
    pub context_after: Vec<String>,
    pub tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_mtime_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub match_line_hash: Option<String>,
}

/// Computed staleness state of a stashed node (render-time, not stored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleState {
    Fresh,
    Unknown,
    Stale(StaleReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    FileMissing,
    MtimeAdvanced,
    MtimeAdvancedContentIntact,
    ContentDrifted,
}

impl StaleReason {
    /// The string form used in v0.x output / the `stale_reason` field.
    pub fn as_str(&self) -> &'static str {
        match self {
            StaleReason::FileMissing => "file_missing",
            StaleReason::MtimeAdvanced => "mtime_advanced",
            StaleReason::MtimeAdvancedContentIntact => "mtime_advanced_content_intact",
            StaleReason::ContentDrifted => "content_drifted",
        }
    }
}
