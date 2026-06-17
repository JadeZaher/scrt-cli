//! # scrt-core
//!
//! The scrt engine: token-budgeted **context-node** search plus an
//! instantiable **mind palace**. In-process Rust port of
//! `mind-palace-graph` (v0.3.2, the canonical reference).
//!
//! ## Thesis
//!
//! Any directory is a database. scrt is a processing engine for grounded
//! generative context — the agent is the processor, scrt is the layer in
//! between. Same lazy-interpretation bet as the Node v0.x line: no
//! write-time LLM compression, perfect fidelity, the repo *is* the memory.
//! Interpretation is deferred to read time.
//!
//! ## What this crate owns (planned)
//!
//! - **Search** (Prompt 2): source resolution + the BurntSushi `grep`
//!   family in-process (no subprocess `rg`) + node construction with
//!   token-budget trimming. Serializes to the v0.x JSON node shape.
//! - **Budgeting / presets / fuzzy / agent-json** (Prompt 3): effort
//!   presets, fill/deep strategy, clip, sort, window-curve, fuzzy driver,
//!   and the `agent-json` envelope.
//! - **Mind palace** (Prompt 4): the `Palace` trait with `FilePalace`
//!   (on-disk, byte-compatible with v0.x) and `MemoryPalace` (in-process)
//!   backends, plus multi-tenant routing.
//!
//! The compat boundary — exact JSON shapes that must round-trip — is
//! documented in `COMPAT.md` at the workspace root. Branding note: scrt
//! emits `scrt`-prefixed names where v0.x emitted `mpg`; the parity
//! harness normalizes the two before diffing. See `COMPAT.md §Branding`.
//!
//! No engine code lives here yet (Prompt 1 is foundation only).

// ── Engine modules ──────────────────────────────────────────────────────
// Prompt 2 (search + node construction) is implemented below. Later
// phases add: fuzzy + agent-json envelope (Prompt 3), palace (Prompt 4).

pub mod envelope; // agent-json envelope (Prompt 3)
pub mod format; // json output; agent-json (Prompt 3); llm/markdown/text (Prompt 6)
pub mod fuzzy; // trigram regex + Levenshtein post-filter (Prompt 3)
pub mod nodes; // match -> token-budgeted context node
pub mod orchestrator; // the search() pipeline (port of index.ts main path)
pub mod palace; // mind palace: FilePalace + MemoryPalace + multi-tenant registry
pub mod pagination; // page-through utility
pub mod search; // in-process grep-searcher driver (replaces rg subprocess)
pub mod sources; // file/glob/dir/cmd/url/stdin resolution
pub mod tokens; // chars/4 estimator
pub mod tool_spec; // provider tool descriptors (openai/anthropic/gemini)
pub mod types; // Node / Source / Match / SearchResult

// Convenience re-exports for the common entry points.
pub use envelope::{build_agent_envelope, AgentEnvelope, EnvelopeOpts};
pub use orchestrator::{search, search_with_meta, SearchConfig, SearchMeta};
pub use sources::SourceInput;
pub use types::{Effort, SearchResult, SortMode, Strategy, WindowCurve};

/// Version of the Node reference implementation this port targets.
/// Used by COMPAT round-trip tests to label golden fixtures.
pub const REFERENCE_VERSION: &str = "0.3.2";

/// On-disk palace format version (matches v0.x `PALACE_VERSION`).
pub const PALACE_VERSION: u32 = 2;
