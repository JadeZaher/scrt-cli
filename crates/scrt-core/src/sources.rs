//! Source resolution — port of v0.x `src/sources.ts`.
//!
//! A *source* is a stream of text we can search: file / glob / directory
//! (recursive, `.gitignore`-respecting via the `ignore` crate) / command
//! stdout / URL fetch / stdin. v0.x fed non-file content to rg via a temp
//! file; scrt searches in-memory content directly (`search::search_content`),
//! so the temp-file dance is gone.
//!
//! Path classification mirrors `classifyPathSpecs`: existing files →
//! `files`, directories → `bulk` (walked here, since we own the walker),
//! glob-meta specs → expanded to files, non-existent non-glob specs →
//! passed through so the engine surfaces the error.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::types::{SearchOptions, Source, SourceType};

/// A resolved source: its identity plus, for non-file sources, the
/// captured content. File sources carry `None` and are read by the engine.
#[derive(Debug, Clone)]
pub struct ResolvedSource {
    pub source: Source,
    pub content: Option<String>,
}

/// A raw input spec from the CLI, pre-resolution (v0.x `SourceInput`).
#[derive(Debug, Clone)]
pub enum SourceInput {
    Path(String),
    Command(String),
    Stdin,
    #[allow(dead_code)]
    Url(String),
}

/// Does this spec contain glob metacharacters? (v0.x `hasGlobMeta`.)
fn has_glob_meta(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}

/// Classify path specs into literal files and directories ("bulk").
/// Returns absolute paths so dedup is stable across relative/absolute
/// inputs. `@file` / `@-` indirection is expanded inline.
///
/// Unlike v0.x (which hands dirs to rg), we own the walker, so dirs are
/// walked here via `ignore` and their files land in `files`. Glob specs
/// are expanded against the walked tree.
pub fn classify_path_specs(
    specs: &[String],
    opts: &SearchOptions,
    stdin_content: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut files: BTreeSet<String> = BTreeSet::new();

    let classify = |spec: &str, files: &mut BTreeSet<String>| -> Result<(), String> {
        let p = Path::new(spec);
        if p.exists() {
            if p.is_file() {
                files.insert(abs(spec));
                return Ok(());
            }
            if p.is_dir() {
                for f in walk_dir(spec, opts) {
                    files.insert(f);
                }
                return Ok(());
            }
        }
        if has_glob_meta(spec) {
            for f in expand_glob(spec, opts) {
                files.insert(f);
            }
            return Ok(());
        }
        // Non-existent, non-glob: keep it so the engine reports the error
        // (mirrors v0.x pushing the spec to `bulk` for rg to surface).
        files.insert(spec.to_string());
        Ok(())
    };

    for spec in specs {
        if spec == "@-" {
            let text = stdin_content.unwrap_or("").to_string();
            for line in text.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                classify(t, &mut files)?;
            }
            continue;
        }
        if let Some(file_path) = spec.strip_prefix('@') {
            let text = std::fs::read_to_string(file_path)
                .map_err(|e| format!("Cannot read path list from @{file_path}: {e}"))?;
            for line in text.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                classify(t, &mut files)?;
            }
            continue;
        }
        classify(spec, &mut files)?;
    }

    Ok(files.into_iter().collect())
}

/// Walk a directory honoring ignore rules, returning absolute file paths.
/// `--hidden` and `--no-ignore` map onto the `ignore` walker; `--type`,
/// `--include`/`--exclude` are applied as overrides (CLI wiring in the
/// binary crate; the plumbing lives here).
fn walk_dir(dir: &str, opts: &SearchOptions) -> Vec<String> {
    let mut wb = ignore::WalkBuilder::new(dir);
    wb.hidden(!opts.hidden) // ignore::hidden(true) means *skip* hidden
        .git_ignore(!opts.no_ignore)
        .git_global(!opts.no_ignore)
        .git_exclude(!opts.no_ignore)
        .ignore(!opts.no_ignore)
        .parents(!opts.no_ignore);

    // include/exclude globs as an override set.
    if !opts.include_globs.is_empty() || !opts.exclude_globs.is_empty() {
        let mut ob = ignore::overrides::OverrideBuilder::new(dir);
        for g in &opts.include_globs {
            let _ = ob.add(g);
        }
        for g in &opts.exclude_globs {
            let _ = ob.add(&format!("!{g}"));
        }
        if let Ok(ov) = ob.build() {
            wb.overrides(ov);
        }
    }

    // Collect raw file paths from the walk, then canonicalize in parallel:
    // `abs()` does a `canonicalize` syscall per path, which dominates on a
    // large tree (10k files = 10k syscalls). rayon fans them across cores.
    use rayon::prelude::*;
    let raw: Vec<String> = wb
        .build()
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .collect();
    raw.par_iter().map(|s| abs(s)).collect()
}

/// Expand a glob spec to absolute file paths. Walks from the glob's
/// non-wildcard root with `ignore`, filtering by a compiled `globset`.
fn expand_glob(pattern: &str, opts: &SearchOptions) -> Vec<String> {
    let normalized = pattern.replace('\\', "/");
    let glob = match globset::Glob::new(&normalized) {
        Ok(g) => g.compile_matcher(),
        Err(_) => return Vec::new(),
    };
    // Root = longest leading path segment without glob meta, else ".".
    let root = glob_root(&normalized);
    let mut wb = ignore::WalkBuilder::new(&root);
    wb.hidden(!opts.hidden)
        .git_ignore(!opts.no_ignore)
        .ignore(!opts.no_ignore);
    let mut out = Vec::new();
    for entry in wb.build().flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let path = entry.path();
            let norm = path.to_string_lossy().replace('\\', "/");
            if glob.is_match(&norm) || glob.is_match(path) {
                if let Some(s) = path.to_str() {
                    out.push(abs(s));
                }
            }
        }
    }
    out
}

/// The non-wildcard leading directory of a glob pattern, or ".".
fn glob_root(pattern: &str) -> String {
    let mut root = String::new();
    for seg in pattern.split('/') {
        if has_glob_meta(seg) {
            break;
        }
        if !root.is_empty() {
            root.push('/');
        }
        root.push_str(seg);
    }
    if root.is_empty() || has_glob_meta(&root) {
        ".".to_string()
    } else if Path::new(&root).is_dir() {
        root
    } else {
        // root was a file prefix; back off to its parent or ".".
        Path::new(&root)
            .parent()
            .and_then(|p| p.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(".")
            .to_string()
    }
}

/// Resolve to an absolute path string, falling back to the input on error.
fn abs(p: &str) -> String {
    std::fs::canonicalize(p)
        .ok()
        .and_then(|pb| pb.to_str().map(strip_unc))
        .unwrap_or_else(|| {
            std::path::absolute(p)
                .ok()
                .and_then(|pb| pb.to_str().map(str::to_string))
                .unwrap_or_else(|| p.to_string())
        })
}

/// Strip the Windows `\\?\` UNC verbatim prefix that `canonicalize` adds,
/// so paths match v0.x `path.resolve` output (which has no UNC prefix).
fn strip_unc(s: &str) -> String {
    s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
}

// ── Command / stdin / url capture ───────────────────────────────────────

/// Cap captured command stdout at 64 MB (v0.x `COMMAND_OUTPUT_MAX_BYTES`).
const COMMAND_OUTPUT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Capture a shell command's stdout for searching. Runs through the
/// platform shell (`cmd /c` on Windows, `bash -c` elsewhere) so quoting
/// parses the way the user typed it. Mirrors `captureCommand`'s shell
/// choice and truncation marker. The CLI path is synchronous; async
/// timeout enforcement lives in the server crate.
pub fn capture_command(cmd: &str) -> Result<String, String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Err("Empty command".to_string());
    }
    let output = if cfg!(windows) {
        Command::new("cmd").args(["/c", trimmed]).output()
    } else {
        Command::new("bash").args(["-c", trimmed]).output()
    }
    .map_err(|e| format!("Failed to run command: {e}"))?;

    if !output.status.success() {
        // v0.x rejects on non-zero exit (unless truncated). Keep parity.
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut msg = format!(
            "Command exited with code {code}: {}",
            &trimmed[..trimmed.len().min(200)]
        );
        if !stderr.is_empty() {
            msg.push_str(&format!("\nstderr: {}", &stderr[..stderr.len().min(500)]));
        }
        return Err(msg);
    }

    let mut bytes = output.stdout;
    let mut truncated = false;
    if bytes.len() > COMMAND_OUTPUT_MAX_BYTES {
        bytes.truncate(COMMAND_OUTPUT_MAX_BYTES);
        truncated = true;
    }
    let mut s = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        s.push_str(&format!(
            "\n[scrt: command output truncated at {COMMAND_OUTPUT_MAX_BYTES} bytes]\n"
        ));
    }
    Ok(s)
}

/// Read all of stdin to a string. (Caching across the @- and content uses
/// is handled by the caller, which reads once and threads the value.)
pub fn read_stdin() -> Result<String, String> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("Failed to read stdin: {e}"))?;
    Ok(buf)
}

/// Cap fetched URL body at 16 MB (v0.x `URL_FETCH_MAX_BYTES`).
#[cfg(feature = "url-source")]
const URL_FETCH_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Fetch a URL body for searching, with the v0.x content-type guard and
/// size cap. Behind the `url-source` feature so a pure CLI/lib build that
/// never fetches a URL doesn't pull in reqwest. This blocking path uses
/// reqwest's blocking client; async timeout enforcement lives in the server
/// crate.
#[cfg(feature = "url-source")]
pub fn capture_url(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("scrt/0.1 (+https://github.com/JadeZaher/scrt-cli)")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("Failed to fetch {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Failed to fetch {url}: {}", resp.status()));
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let text_ok = ct.is_empty()
        || ct.starts_with("text/")
        || [
            "json",
            "xml",
            "yaml",
            "javascript",
            "csv",
            "html",
            "markdown",
        ]
        .iter()
        .any(|t| ct.contains(t));
    if !text_ok {
        return Err(format!(
            "Refusing to fetch non-text content-type \"{ct}\" from {url}."
        ));
    }
    let body = resp.bytes().map_err(|e| e.to_string())?;
    if body.len() > URL_FETCH_MAX_BYTES {
        return Err(format!(
            "Fetched body exceeded {URL_FETCH_MAX_BYTES} bytes from {url}."
        ));
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Build the command source identity (v0.x `cmd:<cmd>` id, `$ <cmd>` label).
pub fn command_source(cmd: &str) -> Source {
    Source {
        id: format!("cmd:{cmd}"),
        source_type: SourceType::Command,
        label: Some(format!("$ {cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_meta_detection() {
        assert!(has_glob_meta("**/*.ts"));
        assert!(has_glob_meta("a?b"));
        assert!(!has_glob_meta("src/main.rs"));
    }

    #[test]
    fn glob_root_extraction() {
        // `src` may or may not be a dir here; either way the root is non-empty
        // and extraction must not panic.
        assert!(!glob_root("src/**/*.ts").is_empty());
        assert_eq!(glob_root("*.ts"), ".");
    }

    #[test]
    fn empty_command_errors() {
        assert!(capture_command("   ").is_err());
    }

    #[test]
    fn command_source_shape() {
        let s = command_source("git log");
        assert_eq!(s.id, "cmd:git log");
        assert_eq!(s.label.as_deref(), Some("$ git log"));
        assert_eq!(s.source_type, SourceType::Command);
    }
}
