//! scrt — command-line entry point. Full surface port of v0.x `cli.ts` +
//! the search/palace orchestration in `index.ts`.
//!
//! Exit codes (v0.x contract, minus code 3): 0 match / 1 no-match /
//! 2 bad-args / 4 palace-error / 99 unexpected. Code 3 ("ripgrep not
//! installed") is GONE — scrt owns the regex engine (MIGRATION.md).
//!
//! Defaults match v0.x: effort `quick`, strategy `fill`, format `llm`.
//! Branding: user-facing output is `scrt`-branded; `MPG_MIND_PALACE` /
//! `MPG_PATTERN` env vars are kept for migration.

use std::process::exit;

mod args;
mod evolve_cli;
mod palace_cli;

use args::{ParseError, RawArgs};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&argv) {
        Ok(c) => c,
        Err(AppError::BadArgs(msg)) => {
            eprintln!("scrt: {msg}");
            eprintln!("Run 'scrt --help' for usage.");
            2
        }
        Err(AppError::Palace(msg)) => {
            eprintln!("scrt: {msg}");
            4
        }
        Err(AppError::Unexpected(msg)) => {
            eprintln!("scrt: unexpected error: {msg}");
            99
        }
    };
    exit(code);
}

/// Internal error carrying its intended exit code.
pub enum AppError {
    BadArgs(String),
    Palace(String),
    Unexpected(String),
}

impl From<ParseError> for AppError {
    fn from(e: ParseError) -> Self {
        AppError::BadArgs(e.0)
    }
}

fn run(argv: &[String]) -> Result<i32, AppError> {
    // `evolve` is a subcommand with its own argv grammar (init/corpus/train);
    // handle it before the search arg parser.
    if argv.first().map(String::as_str) == Some("evolve") {
        return evolve_cli::handle(&argv[1..]);
    }

    let raw = args::parse(argv)?;

    // ── Meta / mode switches (short-circuit before search validation) ────
    if raw.help {
        print!("{}", args::HELP);
        return Ok(0);
    }
    if raw.version {
        println!("scrt {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if raw.print_entry {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "scrt".into());
        println!("{exe}");
        return Ok(0);
    }
    if raw.tool_spec {
        let fmt = raw.tool_spec_format.as_deref().unwrap_or("openai");
        let spec = scrt_core::tool_spec::build_tool_spec(fmt).map_err(AppError::BadArgs)?;
        println!("{}", serde_json::to_string_pretty(&spec).unwrap());
        return Ok(0);
    }
    if raw.serve {
        return serve(&raw);
    }

    // ── --ls / --tree: list searchable files (via the ignore walker) ─────
    if raw.ls {
        return list_files(&raw);
    }

    // ── Palace-only operations (no search) ───────────────────────────────
    if let Some(code) = palace_cli::handle_palace_only(&raw)? {
        return Ok(code);
    }

    // ── Search path ──────────────────────────────────────────────────────
    search(&raw)
}

/// Start the long-running server (`--serve` / `--serve-http`).
fn serve(raw: &RawArgs) -> Result<i32, AppError> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| AppError::Unexpected(e.to_string()))?;
    let host = raw.serve_host.clone().unwrap_or_else(|| "127.0.0.1".into());
    let port = raw.serve_port.unwrap_or(17317);
    let http = raw.serve_http;
    rt.block_on(async move {
        if http {
            scrt_server::http::serve_http(&host, port)
                .await
                .map_err(|e| AppError::Unexpected(e.to_string()))?;
        } else {
            scrt_server::stdio::serve_stdio()
                .await
                .map_err(|e| AppError::Unexpected(e.to_string()))?;
        }
        Ok(0)
    })
}

/// `--ls` / `--tree`: print every searchable file. Replaces v0.x's
/// `rg --files` subprocess with the `ignore` walker (MIGRATION.md §rg).
fn list_files(raw: &RawArgs) -> Result<i32, AppError> {
    let roots: Vec<String> = if raw.in_paths.is_empty() {
        vec![".".to_string()]
    } else {
        raw.in_paths.clone()
    };
    let mut count = 0;
    for root in roots {
        let mut wb = ignore::WalkBuilder::new(&root);
        wb.hidden(!raw.hidden).git_ignore(!raw.no_ignore).ignore(!raw.no_ignore);
        for entry in wb.build().flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                println!("{}", entry.path().display());
                count += 1;
            }
        }
    }
    Ok(if count == 0 { 1 } else { 0 })
}

fn search(raw: &RawArgs) -> Result<i32, AppError> {
    use scrt_core::format::OutputFormat;
    use scrt_core::types::{Effort, SearchOptions};
    use scrt_core::{
        build_agent_envelope, search_with_meta, EnvelopeOpts, SearchConfig, SortMode, SourceInput,
        Strategy, WindowCurve,
    };

    let pattern = resolve_pattern(raw)?;

    let mut inputs: Vec<SourceInput> = Vec::new();
    palace_cli::prepend_palace_sources(raw, &mut inputs)?;
    for p in &raw.in_paths {
        inputs.push(SourceInput::Path(p.clone()));
    }
    if let Some(cmd) = &raw.cmd {
        inputs.push(SourceInput::Command(cmd.clone()));
    }
    if let Some(url) = &raw.url {
        inputs.push(SourceInput::Url(url.clone()));
    }
    let mut stdin_content = None;
    if raw.stdin {
        stdin_content = Some(scrt_core::sources::read_stdin().map_err(AppError::Unexpected)?);
        inputs.push(SourceInput::Stdin);
    }

    let Some(pattern) = pattern else {
        return Err(AppError::BadArgs(
            "No pattern provided. Pass it as the first positional argument, e.g. `scrt \"TODO\"`."
                .into(),
        ));
    };

    if inputs.is_empty() {
        return Err(AppError::BadArgs(
            "No source provided. Use --in <path>, --cmd <command>, --url <url>, --mp-from, --mp-compose, or pipe via stdin.".into(),
        ));
    }

    let effort = raw.effort.unwrap_or(Effort::Quick);
    let (pb, pa, pn) = scrt_core::orchestrator::effort_preset(effort);

    let opts = SearchOptions {
        case_insensitive: raw.ignore_case,
        word_match: raw.word,
        fixed_strings: raw.fixed_strings,
        multiline: raw.multiline,
        hidden: raw.hidden,
        no_ignore: raw.no_ignore,
        include_globs: raw.include_globs.clone(),
        exclude_globs: raw.exclude_globs.clone(),
        type_filter: raw.type_filter.clone(),
        glob_case_insensitive: false,
        max_columns: None,
    };

    let config = SearchConfig {
        pattern: pattern.clone(),
        inputs,
        effort,
        strategy: raw.strategy.unwrap_or(Strategy::Fill),
        before_tokens: raw.before.unwrap_or(pb),
        after_tokens: raw.after.unwrap_or(pa),
        max_nodes: raw.max_nodes.unwrap_or(pn),
        max_tokens: raw.max_tokens,
        clip_chars: raw.clip_chars,
        sort: raw.sort.unwrap_or(SortMode::Default),
        window_curve: raw.window_curve.unwrap_or(WindowCurve::Flat),
        rg_options: opts,
        page: raw.page,
        page_size: raw.page_size,
        all: raw.all,
        fuzzy: raw.fuzzy,
        stdin_content,
    };

    let (result, meta) = search_with_meta(&config).map_err(|e| AppError::Unexpected(e.to_string()))?;

    palace_cli::maybe_stash(raw, &result)?;

    let fmt = raw.format.unwrap_or(OutputFormat::Llm);
    let out = match fmt {
        OutputFormat::Json => scrt_core::format::format_json(&result),
        OutputFormat::Llm => scrt_core::format::format_llm(&result),
        OutputFormat::Text => scrt_core::format::format_text(&result),
        OutputFormat::Markdown => scrt_core::format::format_markdown(&result),
        OutputFormat::AgentJson => {
            // PARITY: the CLI builds the envelope with NO opts (format.ts:101).
            let _ = &meta;
            scrt_core::format::format_agent_json(&build_agent_envelope(
                &result,
                EnvelopeOpts::default(),
            ))
        }
    };
    println!("{out}");

    Ok(if result.total_nodes == 0 { 1 } else { 0 })
}

/// Precedence: `--pattern-file` (exclusive with positional) > positional > `MPG_PATTERN` env.
fn resolve_pattern(raw: &RawArgs) -> Result<Option<String>, AppError> {
    if let Some(pf) = &raw.pattern_file {
        if raw.pattern.is_some() {
            return Err(AppError::BadArgs(
                "--pattern-file is mutually exclusive with a positional pattern.".into(),
            ));
        }
        let content = std::fs::read_to_string(pf)
            .map_err(|e| AppError::BadArgs(format!("--pattern-file: cannot read {pf}: {e}")))?;
        // Strip a single trailing newline (v0.x `.replace(/\r?\n$/, "")`).
        let trimmed = content
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(&content);
        if trimmed.is_empty() {
            return Err(AppError::BadArgs(format!("--pattern-file: {pf} is empty.")));
        }
        return Ok(Some(trimmed.to_string()));
    }
    if let Some(p) = &raw.pattern {
        return Ok(Some(p.clone()));
    }
    Ok(std::env::var("MPG_PATTERN").ok().filter(|s| !s.is_empty()))
}
