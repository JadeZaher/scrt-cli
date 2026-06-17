//! Evolve configuration — the setup surface the user fills in to point scrt
//! at a local model. Lives at `.scrt/evolve.toml` (next to the palace's
//! `.mpg/`). Designed so the *config + corpus-export* path works with no ML
//! deps; only the `train` feature (candle) consumes `model_path`.
//!
//! Ergonomics goal: a user who wants to try self-evolution provides ONE
//! thing — a path to a raw local model — and runs `scrt evolve init` to
//! scaffold this file, then `scrt evolve train`. Nothing in the default
//! build references the model.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default config location relative to a project root.
pub const DEFAULT_CONFIG_DIR: &str = ".scrt";
pub const DEFAULT_CONFIG_FILE: &str = "evolve.toml";

/// The on-disk evolve config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolveConfig {
    pub evolve: EvolveSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolveSection {
    /// Path to the **raw local model** the user provides — a directory of
    /// safetensors + tokenizer, or a single `.safetensors` file. This is
    /// the one thing the user must supply. Consumed only under `--features
    /// train`; the corpus-export path ignores it.
    pub model_path: PathBuf,

    /// Which backend interprets `model_path`. `candle` loads raw weights
    /// in-process; `endpoint` (future) would POST to a local OpenAI-shaped
    /// server. v1 spike supports `candle`.
    #[serde(default = "default_backend")]
    pub backend: Backend,

    /// Training epochs over the distilled corpus (spike default: 1).
    #[serde(default = "default_epochs")]
    pub epochs: usize,

    /// Output path for the trained per-palace adapter weights. Defaults to
    /// `.mpg/embeddings/<palace-id>.safetensors` resolved at train time.
    #[serde(default)]
    pub adapter_out: Option<PathBuf>,

    /// Learning rate for the adapter loop.
    #[serde(default = "default_lr")]
    pub learning_rate: f64,

    /// Number of negative chunks sampled per (query, positive) row.
    #[serde(default = "default_negatives")]
    pub negatives_per_row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Load raw safetensors weights in-process via candle.
    Candle,
    /// POST to a local OpenAI-shaped embedding endpoint (reserved; not in
    /// the v1 spike).
    Endpoint,
}

fn default_backend() -> Backend {
    Backend::Candle
}
fn default_epochs() -> usize {
    1
}
fn default_lr() -> f64 {
    2e-5
}
fn default_negatives() -> usize {
    4
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("evolve config not found at {0}. Run `scrt evolve init --model <path>` first.")]
    NotFound(PathBuf),
    #[error("evolve config at {0} is invalid: {1}")]
    Invalid(PathBuf, String),
    #[error("io error on {0}: {1}")]
    Io(PathBuf, String),
}

impl EvolveConfig {
    /// Resolve the config path under a project root.
    pub fn path_for(root: &Path) -> PathBuf {
        root.join(DEFAULT_CONFIG_DIR).join(DEFAULT_CONFIG_FILE)
    }

    /// Load and parse the config from `path`.
    pub fn load(path: &Path) -> Result<EvolveConfig, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(path.to_path_buf(), e.to_string()))?;
        let cfg: EvolveConfig = toml::from_str(&text)
            .map_err(|e| ConfigError::Invalid(path.to_path_buf(), e.to_string()))?;
        Ok(cfg)
    }

    /// Scaffold a fresh config pointing at `model_path` (the `init` step).
    /// Returns the rendered TOML; the caller writes it to disk.
    pub fn scaffold(model_path: &Path) -> String {
        // Hand-rendered (not `toml::to_string`) so we can include guiding
        // comments — this file is the user's setup surface.
        format!(
            "# scrt self-evolution config (EXPERIMENTAL, v2 spike).\n\
             # Distills this project's mind-palace stashes into a per-agent\n\
             # retrieval adapter. Requires a build with `--features train`\n\
             # (the candle ML stack) to actually train; corpus export works\n\
             # without it.\n\
             \n\
             [evolve]\n\
             # The raw local model you provide — a dir of safetensors +\n\
             # tokenizer.json, or a single .safetensors file.\n\
             model_path = {model:?}\n\
             \n\
             # Backend that interprets model_path: \"candle\" (load weights\n\
             # in-process). \"endpoint\" is reserved for a local embedding server.\n\
             backend = \"candle\"\n\
             \n\
             # Training over the distilled stash corpus (spike default: 1 epoch).\n\
             epochs = 1\n\
             learning_rate = 2e-5\n\
             negatives_per_row = 4\n\
             \n\
             # Where to write the trained adapter. Defaults to\n\
             # .mpg/embeddings/<palace-id>.safetensors when omitted.\n\
             # adapter_out = \".mpg/embeddings/default.safetensors\"\n",
            model = model_path
        )
    }

    /// Validate that the model path the user gave actually exists, with a
    /// helpful message. Called at train time (not load time) so the config
    /// can be scaffolded before the model is in place.
    pub fn validate_model(&self) -> Result<(), ConfigError> {
        let p = &self.evolve.model_path;
        if !p.exists() {
            return Err(ConfigError::Invalid(
                EvolveConfig::path_for(Path::new(".")),
                format!(
                    "model_path {p:?} does not exist. Point it at a local model \
                     (safetensors dir or file)."
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_then_parse_roundtrips() {
        let rendered = EvolveConfig::scaffold(Path::new("/models/nomic-embed-text"));
        // The scaffold must parse back as valid config.
        let cfg: EvolveConfig = toml::from_str(&rendered).unwrap();
        assert_eq!(cfg.evolve.backend, Backend::Candle);
        assert_eq!(cfg.evolve.epochs, 1);
        assert_eq!(cfg.evolve.negatives_per_row, 4);
        assert!(cfg.evolve.model_path.to_string_lossy().contains("nomic"));
    }

    #[test]
    fn missing_config_is_not_found() {
        let p = PathBuf::from("/nonexistent/.scrt/evolve.toml");
        assert!(matches!(
            EvolveConfig::load(&p),
            Err(ConfigError::NotFound(_))
        ));
    }

    #[test]
    fn defaults_fill_in() {
        let toml_src = "[evolve]\nmodel_path = \"/m\"\n";
        let cfg: EvolveConfig = toml::from_str(toml_src).unwrap();
        assert_eq!(cfg.evolve.backend, Backend::Candle); // default
        assert_eq!(cfg.evolve.learning_rate, 2e-5); // default
    }
}
