//! # scrt-evolve
//!
//! Self-evolution spike (v2, exploratory). Distills a palace's stashes into
//! a per-agent retrieval signal — a self-directed, post-deployment shaping
//! loop scoped per directory of work, on unstructured data.
//!
//! ## Ergonomics (the deliberate design)
//!
//! - **Feature-flagged ML.** The heavy candle stack is behind
//!   `--features train`. A default workspace build compiles `config` +
//!   `corpus` only — NO ML deps. You can export the training corpus from a
//!   palace without any model present.
//! - **One-thing setup.** The user provides a path to a raw local model;
//!   `EvolveConfig::scaffold` writes `.scrt/evolve.toml` pointing at it.
//!   `model_path` is read only by the `train` feature.
//!
//! ## Flow
//!
//! 1. `scrt evolve init --model <path>` → scaffolds `.scrt/evolve.toml`.
//! 2. `scrt evolve corpus` → exports `{query, positive, negatives[]}` JSONL
//!    from the palace (no ML).
//! 3. `scrt evolve train` (needs `--features train`) → trains a per-palace
//!    adapter, saves `.mpg/embeddings/<palace-id>.safetensors`.
//! 4. `scrt search … --retriever hybrid` → blends substring rg scores with
//!    the adapter's similarity scores.
//!
//! All exploratory; see EVOLVE-SPIKE.md for the result.

pub mod config;
pub mod corpus;

#[cfg(feature = "train")]
pub mod train;

pub use config::{Backend, ConfigError, EvolveConfig, EvolveSection};
pub use corpus::{build_corpus, to_jsonl, CorpusOptions, CorpusRow};

/// Whether this build can actually train (the `train` feature is on).
pub const fn train_enabled() -> bool {
    cfg!(feature = "train")
}

/// Resolve the default adapter output path for a palace id, relative to a
/// project root: `<root>/.mpg/embeddings/<palace-id>.safetensors`.
pub fn default_adapter_path(root: &std::path::Path, palace_id: &str) -> std::path::PathBuf {
    root.join(".mpg")
        .join("embeddings")
        .join(format!("{palace_id}.safetensors"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_path_shape() {
        let p = default_adapter_path(std::path::Path::new("/proj"), "default");
        assert!(
            p.ends_with("embeddings/default.safetensors")
                || p.ends_with("embeddings\\default.safetensors")
        );
    }

    #[test]
    fn train_flag_reflects_feature() {
        // In a default test build the train feature is off.
        assert_eq!(train_enabled(), cfg!(feature = "train"));
    }
}
