//! `scrt evolve` subcommands — the self-evolution setup + run surface.
//!
//!   scrt evolve init --model <path>   scaffold .scrt/evolve.toml
//!   scrt evolve corpus [--out <f>]    export {query,positive,negatives} JSONL
//!   scrt evolve train                 train per-palace adapter (needs the
//!                                      CLI built with --features evolve-train)
//!
//! The `init` + `corpus` steps work in any build. `train` only runs when the
//! ML stack was compiled in; otherwise it prints how to enable it. This is
//! the ergonomics contract: setup is always available; the heavy path is opt-in.

use std::path::{Path, PathBuf};

use scrt_evolve::{build_corpus, to_jsonl, CorpusOptions, EvolveConfig};

use crate::AppError;

/// Dispatch an `evolve` subcommand. `rest` is argv after the `evolve` token.
pub fn handle(rest: &[String]) -> Result<i32, AppError> {
    let sub = rest.first().map(String::as_str).unwrap_or("");
    match sub {
        "init" => init(&rest[1..]),
        "corpus" => corpus(&rest[1..]),
        "train" => train(&rest[1..]),
        "" => Err(AppError::BadArgs(
            "evolve: subcommand required (init | corpus | train)".into(),
        )),
        other => Err(AppError::BadArgs(format!("evolve: unknown subcommand {other}"))),
    }
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1))
}

/// `scrt evolve init --model <path>` — scaffold the config.
fn init(args: &[String]) -> Result<i32, AppError> {
    let model = flag_value(args, "--model").ok_or_else(|| {
        AppError::BadArgs("evolve init: --model <path-to-local-model> is required".into())
    })?;
    let root = Path::new(".");
    let cfg_path = EvolveConfig::path_for(root);
    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::Unexpected(e.to_string()))?;
    }
    let rendered = EvolveConfig::scaffold(Path::new(model));
    std::fs::write(&cfg_path, rendered).map_err(|e| AppError::Unexpected(e.to_string()))?;
    eprintln!("scrt: wrote {}", cfg_path.display());
    eprintln!("scrt: model_path = {model}");
    if !Path::new(model).exists() {
        eprintln!("scrt: note — {model} does not exist yet; point it at a real model before `scrt evolve train`.");
    }
    eprintln!("scrt: next — `scrt evolve corpus` to export the training set, then `scrt evolve train`.");
    Ok(0)
}

/// `scrt evolve corpus [--out <file>] [--mp-path <palace>]` — export JSONL.
fn corpus(args: &[String]) -> Result<i32, AppError> {
    let palace_path = flag_value(args, "--mp-path")
        .map(PathBuf::from)
        .unwrap_or_else(scrt_core::palace::default_palace_path);
    let palace = scrt_core::palace::FilePalace::load(&palace_path, &scrt_core::palace::ops::SystemClock);

    let negatives = flag_value(args, "--negatives")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let rows = build_corpus(
        scrt_core::palace::Palace::data(&palace),
        CorpusOptions { negatives_per_row: negatives },
    );
    let jsonl = to_jsonl(&rows);

    match flag_value(args, "--out") {
        Some(out) => {
            std::fs::write(out, &jsonl).map_err(|e| AppError::Unexpected(e.to_string()))?;
            eprintln!("scrt: wrote {} corpus rows to {out}", rows.len());
        }
        None => {
            print!("{jsonl}");
            eprintln!("scrt: {} corpus rows from {}", rows.len(), palace_path.display());
        }
    }
    Ok(0)
}

/// `scrt evolve train` — train the adapter (feature-gated).
fn train(_args: &[String]) -> Result<i32, AppError> {
    if !scrt_evolve::train_enabled() {
        eprintln!(
            "scrt: `evolve train` needs the ML stack. Rebuild the CLI with:\n  \
             cargo build --release -p scrt-cli --features evolve-train\n\
             (this compiles candle; the default build stays ML-free)."
        );
        return Ok(2);
    }
    #[cfg(feature = "evolve-train")]
    {
        train_inner(_args)
    }
    #[cfg(not(feature = "evolve-train"))]
    {
        Ok(2)
    }
}

#[cfg(feature = "evolve-train")]
fn train_inner(args: &[String]) -> Result<i32, AppError> {
    let root = Path::new(".");
    let cfg_path = EvolveConfig::path_for(root);
    let cfg = EvolveConfig::load(&cfg_path).map_err(|e| AppError::BadArgs(e.to_string()))?;

    let palace_path = flag_value(args, "--mp-path")
        .map(PathBuf::from)
        .unwrap_or_else(scrt_core::palace::default_palace_path);
    let palace = scrt_core::palace::FilePalace::load(&palace_path, &scrt_core::palace::ops::SystemClock);
    let rows = build_corpus(
        scrt_core::palace::Palace::data(&palace),
        CorpusOptions { negatives_per_row: cfg.evolve.negatives_per_row },
    );

    let palace_id = flag_value(args, "--id").cloned().unwrap_or_else(|| "default".into());
    let adapter_out = cfg
        .evolve
        .adapter_out
        .clone()
        .unwrap_or_else(|| scrt_evolve::default_adapter_path(root, &palace_id));

    let report = scrt_evolve::train::train_adapter(&cfg, &rows, &adapter_out)
        .map_err(|e| AppError::Unexpected(e.to_string()))?;
    eprintln!(
        "scrt: trained adapter — {} rows, {} epochs, loss {:.4} -> {}",
        report.rows_used,
        report.epochs,
        report.final_loss,
        report.adapter_path.display()
    );
    Ok(0)
}
