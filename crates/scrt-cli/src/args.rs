//! CLI argument parser — port of `parseArgs` in v0.x `cli.ts`.
//!
//! Hand-rolled (like v0.x) to match its exact flag set, greedy `--in` /
//! `--mp-compose` / `--mp-intersect` / `--mp-except` semantics, comma
//! splitting, and `--mp-link` / `--mp-graph` positional consumption.

use scrt_core::format::OutputFormat;
use scrt_core::types::{Effort, SortMode, Strategy, WindowCurve};

/// A parse error → exit code 2.
pub struct ParseError(pub String);

/// Raw parsed args before resolution. Mirrors v0.x `RawArgs`.
#[derive(Default)]
pub struct RawArgs {
    pub pattern: Option<String>,
    pub pattern_file: Option<String>,
    pub in_paths: Vec<String>,
    pub cmd: Option<String>,
    pub url: Option<String>,
    pub stdin: bool,

    pub before: Option<usize>,
    pub after: Option<usize>,
    pub max_nodes: Option<usize>,
    pub max_tokens: Option<usize>,
    pub strategy: Option<Strategy>,
    pub effort: Option<Effort>,
    pub format: Option<OutputFormat>,
    pub sort: Option<SortMode>,
    pub window_curve: Option<WindowCurve>,
    pub clip_chars: Option<usize>,
    pub fuzzy: bool,

    pub ignore_case: bool,
    pub word: bool,
    pub fixed_strings: bool,
    pub multiline: bool,
    pub hidden: bool,
    pub no_ignore: bool,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub type_filter: Option<String>,

    // Mind palace.
    pub mp_stash_name: Option<String>,
    pub mp_stash_note: Option<String>,
    pub mp_stash_tags: Vec<String>,
    pub mp_stash_replace: bool,
    pub mp_stash_locations: bool,
    pub mp_list: bool,
    pub mp_list_tags: Vec<String>,
    pub mp_get: Option<String>,
    pub mp_get_with_nodes: bool,
    pub mp_drop: Option<String>,
    pub mp_from: Option<String>,
    pub mp_compose: Vec<String>,
    pub mp_except: Option<String>,
    pub mp_except_names: Vec<String>,
    pub mp_intersect: Vec<String>,
    pub mp_path: Option<String>,
    pub mp_ttl: Option<String>,

    // Pruning.
    pub mp_prune_older_than: Option<String>,
    pub mp_prune_keep: Option<usize>,
    pub mp_prune_tag: Option<String>,
    pub mp_prune_all: bool,
    pub mp_prune_expired: bool,
    pub mp_prune_confirm: bool,
    pub mp_prune_dry_run: bool,

    // Relationships.
    pub mp_link: Option<(String, String, String, Option<String>)>, // from,to,type,note
    pub mp_unlink: Option<(String, String)>,
    pub mp_related: Option<String>,
    pub mp_graph: Option<(String, usize)>, // name, depth (default 3)

    // Similarity (SimHash over stash content — see DESIGN.md §2.4).
    pub mp_similar: Option<String>, // a stash name to find neighbors of
    pub mp_similar_term: Option<String>, // ...or a raw term to match against
    pub mp_similar_score: Option<u8>, // 1..=10 falloff steepness (default 5)
    pub mp_similar_match_full: bool, // compare note+body axis (default: note)
    pub mp_similar_match_vector: bool, // compare via random-projection cosine
    pub mp_similar_top: Option<usize>, // truncate ranked list

    // Stash-time link suggestions.
    pub mp_no_suggest_links: bool, // suppress the "~ related" suggestions on --mp-stash
    pub mp_link_threshold: Option<u8>, // 0..=100 min relevance to suggest (default 55)

    // Pagination.
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub all: bool,

    pub no_fill: bool,
    pub ls: bool,

    // Meta / modes.
    pub help: bool,
    pub version: bool,
    pub print_entry: bool,
    pub serve: bool,
    pub serve_http: bool,
    pub serve_port: Option<u16>,
    pub serve_host: Option<String>,
    pub tool_spec: bool,
    pub tool_spec_format: Option<String>,
}

fn require<'a>(flag: &str, argv: &'a [String], i: usize) -> Result<&'a String, ParseError> {
    argv.get(i).ok_or_else(|| ParseError(format!("Missing value for {flag}")))
}

fn parse_int(flag: &str, s: &str) -> Result<usize, ParseError> {
    s.parse::<usize>()
        .map_err(|_| ParseError(format!("{flag} expects a non-negative integer, got: {s}")))
}

/// Split a comma list, dropping empties (v0.x `.split(",").filter(Boolean)`).
fn comma_split(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(',').filter(|p| !p.is_empty()).map(String::from)
}

pub fn parse(argv: &[String]) -> Result<RawArgs, ParseError> {
    let mut a = RawArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        match arg {
            "--no-color" | "--color" => {} // color is cosmetic; accepted, ignored
            "-h" | "--help" => a.help = true,
            "-v" | "--version" => a.version = true,
            "--print-entry" => a.print_entry = true,
            "--pattern-file" => {
                i += 1;
                a.pattern_file = Some(require("--pattern-file", argv, i)?.clone());
            }
            "-i" | "--in" => {
                i += 1;
                while i < argv.len() && !argv[i].starts_with('-') {
                    a.in_paths.extend(comma_split(&argv[i]));
                    i += 1;
                }
                continue;
            }
            "--cmd" => {
                i += 1;
                a.cmd = Some(require("--cmd", argv, i)?.clone());
            }
            "--stdin" => a.stdin = true,
            "-u" | "--url" => {
                i += 1;
                a.url = Some(require("--url", argv, i)?.clone());
            }
            "-b" | "--before" => {
                i += 1;
                a.before = Some(parse_int("--before", require("--before", argv, i)?)?);
            }
            "-a" | "--after" => {
                i += 1;
                a.after = Some(parse_int("--after", require("--after", argv, i)?)?);
            }
            "-n" | "--max-nodes" => {
                i += 1;
                a.max_nodes = Some(parse_int("--max-nodes", require("--max-nodes", argv, i)?)?);
            }
            "--max-tokens" => {
                i += 1;
                a.max_tokens = Some(parse_int("--max-tokens", require("--max-tokens", argv, i)?)?);
            }
            "--strategy" => {
                i += 1;
                let v = require("--strategy", argv, i)?;
                a.strategy = Some(match v.as_str() {
                    "fill" => Strategy::Fill,
                    "deep" => Strategy::Deep,
                    _ => return Err(ParseError(format!("--strategy must be fill or deep, got: {v}"))),
                });
            }
            "-e" | "--effort" => {
                i += 1;
                let v = require("--effort", argv, i)?;
                a.effort = Some(match v.as_str() {
                    "scan" => Effort::Scan,
                    "quick" => Effort::Quick,
                    "normal" => Effort::Normal,
                    "deep" => Effort::Deep,
                    "auto" => Effort::Auto,
                    _ => {
                        return Err(ParseError(format!(
                            "--effort must be scan|quick|normal|deep|auto, got: {v}"
                        )))
                    }
                });
            }
            "-f" | "--format" => {
                i += 1;
                let v = require("--format", argv, i)?;
                if a.tool_spec {
                    if !matches!(v.as_str(), "openai" | "anthropic" | "gemini") {
                        return Err(ParseError(format!(
                            "tool-spec --format must be openai|anthropic|gemini, got: {v}"
                        )));
                    }
                    a.tool_spec_format = Some(v.clone());
                } else {
                    a.format = Some(OutputFormat::parse(v).ok_or_else(|| {
                        ParseError(format!(
                            "--format must be llm|markdown|json|text|agent-json, got: {v}"
                        ))
                    })?);
                }
            }
            "--json" => a.format = Some(OutputFormat::Json),
            "--agent-json" => a.format = Some(OutputFormat::AgentJson),
            "-I" | "--ignore-case" => a.ignore_case = true,
            "-w" | "--word" => a.word = true,
            "-F" | "--fixed-strings" => a.fixed_strings = true,
            "-U" | "--multiline" => a.multiline = true,
            "--hidden" => a.hidden = true,
            "--no-ignore" => a.no_ignore = true,
            "--include" => {
                i += 1;
                a.include_globs.push(require("--include", argv, i)?.clone());
            }
            "--exclude" => {
                i += 1;
                a.exclude_globs.push(require("--exclude", argv, i)?.clone());
            }
            "--type" => {
                i += 1;
                a.type_filter = Some(require("--type", argv, i)?.clone());
            }

            // Mind palace.
            "--mp-stash" => {
                i += 1;
                a.mp_stash_name = Some(require("--mp-stash", argv, i)?.clone());
                i += 1;
                a.mp_stash_note = Some(require("--mp-stash <note>", argv, i)?.clone());
            }
            "--mp-stash-note" => {
                i += 1;
                a.mp_stash_note = Some(require("--mp-stash-note", argv, i)?.clone());
            }
            "--mp-stash-tag" | "--mp-tag" => {
                i += 1;
                a.mp_stash_tags.push(require("--mp-tag", argv, i)?.clone());
            }
            "--mp-replace" => a.mp_stash_replace = true,
            "--mp-stash-locations" => a.mp_stash_locations = true,
            "--mp-list" => a.mp_list = true,
            "--mp-list-tag" => {
                i += 1;
                a.mp_list_tags.push(require("--mp-list-tag", argv, i)?.clone());
            }
            "--mp-get" => {
                i += 1;
                a.mp_get = Some(require("--mp-get", argv, i)?.clone());
            }
            "--with-nodes" | "--full" => a.mp_get_with_nodes = true,
            "--mp-drop" => {
                i += 1;
                a.mp_drop = Some(require("--mp-drop", argv, i)?.clone());
            }
            "--mp-from" => {
                i += 1;
                a.mp_from = Some(require("--mp-from", argv, i)?.clone());
            }
            "--mp-compose" => {
                i += 1;
                while i < argv.len() && !argv[i].starts_with('-') {
                    a.mp_compose.extend(comma_split(&argv[i]));
                    i += 1;
                }
                continue;
            }
            "--mp-except" => {
                i += 1;
                a.mp_except = Some(require("--mp-except", argv, i)?.clone());
                i += 1;
                while i < argv.len() && !argv[i].starts_with('-') {
                    a.mp_except_names.extend(comma_split(&argv[i]));
                    i += 1;
                }
                continue;
            }
            "--mp-intersect" => {
                i += 1;
                while i < argv.len() && !argv[i].starts_with('-') {
                    a.mp_intersect.extend(comma_split(&argv[i]));
                    i += 1;
                }
                continue;
            }
            "--mp-path" => {
                i += 1;
                a.mp_path = Some(require("--mp-path", argv, i)?.clone());
            }
            "--mp-ttl" => {
                i += 1;
                a.mp_ttl = Some(require("--mp-ttl", argv, i)?.clone());
            }

            // Pruning.
            "--mp-prune-older-than" => {
                i += 1;
                a.mp_prune_older_than = Some(require("--mp-prune-older-than", argv, i)?.clone());
            }
            "--mp-prune-keep" => {
                i += 1;
                a.mp_prune_keep = Some(parse_int("--mp-prune-keep", require("--mp-prune-keep", argv, i)?)?);
            }
            "--mp-prune-tag" => {
                i += 1;
                a.mp_prune_tag = Some(require("--mp-prune-tag", argv, i)?.clone());
            }
            "--mp-prune-all" => a.mp_prune_all = true,
            "--mp-prune-expired" => a.mp_prune_expired = true,
            "--mp-prune-confirm" => a.mp_prune_confirm = true,
            "--mp-prune-dry-run" => a.mp_prune_dry_run = true,

            // Relationships.
            "--mp-link" => {
                i += 1;
                let from = require("--mp-link", argv, i)?.clone();
                i += 1;
                let to = require("--mp-link <to>", argv, i)?.clone();
                i += 1;
                let ty = require("--mp-link <type>", argv, i)?.clone();
                // Optional note.
                let mut note = None;
                if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                    i += 1;
                    note = Some(argv[i].clone());
                }
                a.mp_link = Some((from, to, ty, note));
            }
            "--mp-unlink" => {
                i += 1;
                let from = require("--mp-unlink", argv, i)?.clone();
                i += 1;
                let to = require("--mp-unlink <to>", argv, i)?.clone();
                a.mp_unlink = Some((from, to));
            }
            "--mp-related" => {
                i += 1;
                a.mp_related = Some(require("--mp-related", argv, i)?.clone());
            }
            "--mp-graph" => {
                i += 1;
                let name = require("--mp-graph", argv, i)?.clone();
                let mut depth = 3;
                if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                    i += 1;
                    depth = parse_int("--mp-graph <depth>", &argv[i])?;
                }
                a.mp_graph = Some((name, depth));
            }

            // Similarity over stash content.
            "--mp-similar" => {
                i += 1;
                a.mp_similar = Some(require("--mp-similar", argv, i)?.clone());
            }
            "--term" => {
                i += 1;
                a.mp_similar_term = Some(require("--term", argv, i)?.clone());
            }
            "--score" => {
                i += 1;
                let v: u8 = parse_int("--score", require("--score", argv, i)?)? as u8;
                if !(1..=10).contains(&v) {
                    return Err(ParseError(format!("--score must be 1..10, got: {v}")));
                }
                a.mp_similar_score = Some(v);
            }
            "--match" => {
                i += 1;
                let v = require("--match", argv, i)?;
                match v.as_str() {
                    "note" => { a.mp_similar_match_full = false; a.mp_similar_match_vector = false; }
                    "full" => { a.mp_similar_match_full = true; a.mp_similar_match_vector = false; }
                    "vector" => { a.mp_similar_match_full = false; a.mp_similar_match_vector = true; }
                    _ => return Err(ParseError(format!("--match must be note|full|vector, got: {v}"))),
                }
            }
            "--top" => {
                i += 1;
                a.mp_similar_top = Some(parse_int("--top", require("--top", argv, i)?)?);
            }
            "--no-suggest-links" => a.mp_no_suggest_links = true,
            "--link-threshold" => {
                i += 1;
                let v: u8 = parse_int("--link-threshold", require("--link-threshold", argv, i)?)? as u8;
                if v > 100 {
                    return Err(ParseError(format!("--link-threshold must be 0..100, got: {v}")));
                }
                a.mp_link_threshold = Some(v);
            }

            // Pagination / misc.
            "--page" => {
                i += 1;
                a.page = Some(parse_int("--page", require("--page", argv, i)?)?);
            }
            "--page-size" => {
                i += 1;
                a.page_size = Some(parse_int("--page-size", require("--page-size", argv, i)?)?);
            }
            "--all" => a.all = true,
            "--no-auto-tune" => {} // wide-record auto-tune deferred; accepted, no-op
            "--sort" => {
                i += 1;
                let v = require("--sort", argv, i)?;
                a.sort = Some(match v.as_str() {
                    "default" => SortMode::Default,
                    "recent" => SortMode::Recent,
                    "oldest" => SortMode::Oldest,
                    _ => return Err(ParseError(format!("--sort must be default|recent|oldest, got: {v}"))),
                });
            }
            "--window-curve" => {
                i += 1;
                let v = require("--window-curve", argv, i)?;
                a.window_curve = Some(match v.as_str() {
                    "flat" => WindowCurve::Flat,
                    "linear" => WindowCurve::Linear,
                    "log" => WindowCurve::Log,
                    _ => return Err(ParseError(format!("--window-curve must be flat|linear|log, got: {v}"))),
                });
            }
            "--clip" => {
                i += 1;
                a.clip_chars = Some(parse_int("--clip", require("--clip", argv, i)?)?);
            }
            "--fuzzy" => a.fuzzy = true,
            "--ls" | "--tree" => a.ls = true,

            // Server.
            "--serve" => a.serve = true,
            "--serve-http" => {
                a.serve = true;
                a.serve_http = true;
            }
            "--port" => {
                i += 1;
                let v = require("--port", argv, i)?;
                a.serve_port =
                    Some(v.parse().map_err(|_| ParseError(format!("--port expects a port number, got: {v}")))?);
            }
            "--host" => {
                i += 1;
                a.serve_host = Some(require("--host", argv, i)?.clone());
            }

            "--no-fill" => a.no_fill = true,

            // Positional.
            other if !other.starts_with('-') && a.pattern.is_none() => {
                if other == "tool-spec" {
                    a.tool_spec = true;
                } else {
                    a.pattern = Some(maybe_unescape(other));
                }
            }
            other if !other.starts_with('-') => {
                a.in_paths.extend(comma_split(other));
            }
            other => return Err(ParseError(format!("Unknown argument: {other}"))),
        }
        i += 1;
    }
    Ok(a)
}

/// v0.x `maybeUnescapePattern`: strip one layer of matching surrounding
/// quotes if the whole arg is quoted (shell already removed real quotes,
/// but `--pattern='"x"'` style can leave them). Keep minimal.
fn maybe_unescape(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if raw.len() >= 2
        && ((bytes[0] == b'"' && bytes[raw.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\''))
    {
        return raw[1..raw.len() - 1].to_string();
    }
    raw.to_string()
}

/// `--help` text. Brief; the README is the full reference.
pub const HELP: &str = "scrt — node-centric context retrieval for LLM harnesses\n\
\n\
USAGE\n\
  scrt [<pattern>] [options]\n\
  scrt tool-spec --format <openai|anthropic|gemini>\n\
  scrt --serve [--serve-http --port <n>]\n\
\n\
SOURCES\n\
  -i, --in <path>...     files / dirs (recursive) / globs / @file / @-\n\
      --cmd <cmd>        search a command's stdout\n\
      --stdin            search piped stdin\n\
  -u, --url <url>        fetch and search a URL\n\
\n\
NODE SIZING\n\
  -b, --before <n>       tokens of context before each match (default 500)\n\
  -a, --after <n>        tokens of context after (default 500)\n\
  -n, --max-nodes <n>    cap on nodes returned\n\
      --max-tokens <n>   total token budget\n\
      --strategy <m>     fill|deep (default fill)\n\
  -e, --effort <p>       scan|quick|normal|deep|auto (default quick)\n\
      --clip <n>         sub-line clip mode\n\
      --sort <m>         default|recent|oldest\n\
      --window-curve <m> flat|linear|log\n\
      --fuzzy            typo-tolerant search (edit distance <= 2)\n\
\n\
OUTPUT\n\
  -f, --format <fmt>     llm|markdown|json|text|agent-json (default llm)\n\
      --json             alias for --format json\n\
      --no-fill          strict mode (no fill padding)\n\
\n\
MIND PALACE\n\
      --mp-stash <name> <note>   run search, save result (suggests links)\n\
      --no-suggest-links         silence stash-time link suggestions\n\
      --link-threshold <0-100>   min relevance to suggest a link (default 55)\n\
      --mp-tag <tag>             tag a stash (repeatable)\n\
      --mp-list / --mp-get <n>   inspect stashes\n\
      --mp-drop <name>           remove a stash\n\
      --mp-from / --mp-compose / --mp-intersect / --mp-except\n\
      --mp-link / --mp-graph / --mp-related\n\
      --mp-prune-* / --mp-ttl <dur> / --mp-path <file>\n\
\n\
SIMILARITY (SimHash over stash content)\n\
      --mp-similar <name>        rank stashes similar to <name>\n\
      --term <text>              ...or rank against a raw term instead\n\
      --match note|full|vector   note=intent, full=intent+content (chunked),\n\
                                 vector=weighted-cosine lexical (default note)\n\
      --score <1-10>             falloff: 1=wide net, 10=near-identical only (default 5)\n\
      --top <n>                  keep only the n closest\n\
\n\
DISCOVERY & META\n\
      --ls / --tree      list searchable files\n\
      --print-entry      print the resolved binary path\n\
      --pattern-file <p> read the pattern from a file\n\
  -h, --help / -v, --version\n\
\n\
EXIT CODES\n\
  0 match | 1 no-match | 2 bad-args | 4 palace-error | 99 unexpected\n";
