//! Pure mind-palace operations — port of the function surface in v0.x
//! `src/mind-palace.ts`. Every function operates on a `Palace` data value
//! (and clock/fs effects threaded explicitly), so both the `FilePalace`
//! and `MemoryPalace` backends share identical semantics; only persistence
//! differs.
//!
//! Timestamps (`created_at`/`updated_at`/`expires_at`) come from a `Clock`
//! so tests are deterministic and migration goldens are reproducible.

use sha1::{Digest, Sha1};

use super::types::{Palace, Stash, StashedNode};
// Re-export so the server dispatcher can construct the search-meta sub-object
// via `palace::ops::StashSearch` alongside the other ops it imports.
pub use super::types::StashSearch;
use crate::types::{Node, SourceType};

/// Source of "now" as an ISO-8601 string and epoch-ms. The real clock uses
/// the system time; tests inject a fixed clock.
pub trait Clock {
    /// Current time as an ISO-8601 string, matching JS `new Date().toISOString()`
    /// (UTC, millisecond precision, `Z` suffix — e.g. `2026-06-15T07:38:00.000Z`).
    fn now_iso(&self) -> String;
    /// Current time in milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// System clock (production).
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_iso(&self) -> String {
        iso_from_ms(self.now_ms())
    }
    fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Format epoch-ms as an ISO-8601 UTC string with millisecond precision and
/// a `Z` suffix, byte-identical to JS `new Date(ms).toISOString()`.
pub fn iso_from_ms(ms: i64) -> String {
    // Civil-from-days algorithm (Howard Hinnant), no chrono dependency.
    let total_secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // days since 1970-01-01 -> civil date.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, millis
    )
}

/// Stable de-dup preserving first-occurrence order (JS `[...new Set(arr)]`).
pub fn dedup(arr: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in arr {
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

/// 12-hex-char SHA1 of a line's **trimmed** text (port of `hashLine`).
/// `None` only on the (unreachable) hashing error path.
pub fn hash_line(line: &str) -> Option<String> {
    let mut h = Sha1::new();
    h.update(line.trim().as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(hex.chars().take(12).collect())
}

/// Pick the anchor line for `match_line_hash` (port of `pickAnchorLine`):
/// prefer non-blank `match_text`, then last non-blank `context_before`,
/// then first non-blank `context_after`, else `match_text`.
fn pick_anchor_line(n: &Node) -> String {
    if !n.match_text.trim().is_empty() {
        return n.match_text.clone();
    }
    for l in n.context_before.iter().rev() {
        if !l.trim().is_empty() {
            return l.clone();
        }
    }
    for l in &n.context_after {
        if !l.trim().is_empty() {
            return l.clone();
        }
    }
    n.match_text.clone()
}

/// File mtime in ms (port of `captureFileMtime`). `None` if stat fails.
fn capture_file_mtime(path: &str) -> Option<f64> {
    use std::time::UNIX_EPOCH;
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
}

/// Convert full `Node`s into compact `StashedNode`s (port of `stashNodes`).
pub fn stash_nodes(nodes: &[Node]) -> Vec<StashedNode> {
    nodes
        .iter()
        .map(|n| {
            let is_file = n.source.source_type == SourceType::File;
            let mut base = StashedNode {
                source: n.source.id.clone(),
                file_path: if is_file {
                    Some(n.source.id.clone())
                } else {
                    None
                },
                source_type: source_type_str(n.source.source_type).to_string(),
                match_line: n.match_line,
                start_line: n.start_line,
                end_line: n.end_line,
                context_before: n.context_before.clone(),
                match_text: n.match_text.clone(),
                context_after: n.context_after.clone(),
                tokens: n.tokens,
                source_mtime_ms: None,
                match_line_hash: None,
            };
            if is_file {
                base.source_mtime_ms = capture_file_mtime(&n.source.id);
                base.match_line_hash = hash_line(&pick_anchor_line(n));
            }
            base
        })
        .collect()
}

/// Lightweight stash form: locations only, no context (port of
/// `stashNodesLocations`). start/end collapse to match_line; tokens = 0.
pub fn stash_nodes_locations(nodes: &[Node]) -> Vec<StashedNode> {
    nodes
        .iter()
        .map(|n| {
            let is_file = n.source.source_type == SourceType::File;
            let mut base = StashedNode {
                source: n.source.id.clone(),
                file_path: if is_file {
                    Some(n.source.id.clone())
                } else {
                    None
                },
                source_type: source_type_str(n.source.source_type).to_string(),
                match_line: n.match_line,
                start_line: n.match_line,
                end_line: n.match_line,
                context_before: Vec::new(),
                match_text: n.match_text.clone(),
                context_after: Vec::new(),
                tokens: 0,
                source_mtime_ms: None,
                match_line_hash: None,
            };
            if is_file {
                base.source_mtime_ms = capture_file_mtime(&n.source.id);
                base.match_line_hash = hash_line(&pick_anchor_line(n));
            }
            base
        })
        .collect()
}

fn source_type_str(t: SourceType) -> &'static str {
    match t {
        SourceType::File => "file",
        SourceType::Command => "command",
        SourceType::Stdin => "stdin",
        SourceType::Url => "url",
        SourceType::Bulk => "bulk",
    }
}

/// Unique file paths to search (port of `deriveFilePaths`). Prefer file
/// sources from nodes; else fall back to filtering `sources` of obvious
/// non-files (`cmd:`, `http(s)://`, `stdin`).
pub fn derive_file_paths(nodes: &[Node], sources: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for n in nodes {
        if n.source.source_type == SourceType::File && seen.insert(n.source.id.clone()) {
            files.push(n.source.id.clone());
        }
    }
    if !files.is_empty() {
        return files;
    }
    let filtered: Vec<String> = sources
        .iter()
        .filter(|s| {
            !s.starts_with("cmd:")
                && !s.starts_with("http://")
                && !s.starts_with("https://")
                && s.as_str() != "stdin"
        })
        .cloned()
        .collect();
    dedup(&filtered)
}

/// Unique file Sources captured in a stash (port of `stashToSources`):
/// dedup by node `source`, all typed `file`.
pub fn stash_to_source_ids(stash: &Stash) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in &stash.nodes {
        if seen.insert(n.source.clone()) {
            out.push(n.source.clone());
        }
    }
    out
}

/// Parse a human duration into milliseconds (port of `parseDuration`).
/// Accepts `30s`, `10m`, `2h`, `7d`, or a bare number (ms).
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let t = s.trim();
    // Mirror /^([\d.]+)\s*(s|sec|m|min|h|hr|d|day|ms)?$/i
    let (num_part, unit_part): (String, String) = {
        let mut num = String::new();
        let mut rest = String::new();
        let mut in_num = true;
        for c in t.chars() {
            if in_num && (c.is_ascii_digit() || c == '.') {
                num.push(c);
            } else if in_num && c.is_whitespace() {
                in_num = false;
            } else {
                in_num = false;
                if !c.is_whitespace() {
                    rest.push(c);
                }
            }
        }
        (num, rest)
    };
    if num_part.is_empty() {
        return Err(format!("Invalid duration: {s}. Use e.g. 30s, 10m, 2h, 7d."));
    }
    let n: f64 = num_part
        .parse()
        .map_err(|_| format!("Invalid duration: {s}. Use e.g. 30s, 10m, 2h, 7d."))?;
    let unit = if unit_part.is_empty() {
        "ms".to_string()
    } else {
        unit_part.to_lowercase()
    };
    let ms = match unit.as_str() {
        "s" | "sec" => n * 1000.0,
        "m" | "min" => n * 60.0 * 1000.0,
        "h" | "hr" => n * 3_600_000.0,
        "d" | "day" => n * 86_400_000.0,
        "ms" => n,
        other => return Err(format!("Invalid duration: {s} (unit {other}).")),
    };
    Ok(ms as i64)
}

/// Expiry timestamp from now + duration (port of `expiryFromNow`).
pub fn expiry_from_now(clock: &dyn Clock, duration: &str) -> Result<String, String> {
    let ms = clock.now_ms() + parse_duration(duration)?;
    Ok(iso_from_ms(ms))
}

/// What `add_stash` did, for the side-channel confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashAction {
    Created,
    Replaced,
    Merged,
}

/// Options for `add_stash`.
#[derive(Debug, Clone, Default)]
pub struct StashOptions {
    pub replace: bool,
    pub locations: bool,
    pub ttl: Option<String>,
}

/// Add or merge a stash (port of `addStash`). Merge dedups nodes by
/// `(source, match_line)`, keeping first occurrence.
#[allow(clippy::too_many_arguments)]
pub fn add_stash(
    palace: &mut Palace,
    clock: &dyn Clock,
    name: &str,
    note: &str,
    nodes: &[Node],
    meta: StashSearch,
    sources: &[String],
    tags: &[String],
    options: &StashOptions,
) -> Result<StashAction, String> {
    let now = clock.now_iso();
    let expires_at = match &options.ttl {
        Some(ttl) => Some(expiry_from_now(clock, ttl)?),
        None => None,
    };
    let new_nodes = if options.locations {
        stash_nodes_locations(nodes)
    } else {
        stash_nodes(nodes)
    };
    let new_file_paths = derive_file_paths(nodes, sources);

    if !palace.stashes.contains_key(name) {
        let stash = Stash {
            name: name.to_string(),
            note: note.to_string(),
            tags: tags.to_vec(),
            created_at: now.clone(),
            updated_at: now,
            expires_at,
            search: meta,
            sources: dedup(sources),
            nodes: new_nodes,
            file_paths: new_file_paths,
            relations: Vec::new(),
        };
        palace.stashes.insert(name.to_string(), stash);
        return Ok(StashAction::Created);
    }

    let existing = palace.stashes.get_mut(name).unwrap();
    if options.replace {
        existing.note = note.to_string();
        existing.tags = tags.to_vec();
        existing.updated_at = now;
        existing.expires_at = expires_at;
        existing.search = meta;
        existing.sources = dedup(sources);
        existing.nodes = new_nodes;
        existing.file_paths = new_file_paths;
        return Ok(StashAction::Replaced);
    }

    let mut seen: std::collections::HashSet<String> = existing
        .nodes
        .iter()
        .map(|n| format!("{}:{}", n.source, n.match_line))
        .collect();
    for n in new_nodes {
        let key = format!("{}:{}", n.source, n.match_line);
        if seen.insert(key) {
            existing.nodes.push(n);
        }
    }
    let mut merged_sources = existing.sources.clone();
    merged_sources.extend_from_slice(sources);
    existing.sources = dedup(&merged_sources);
    let mut merged_files = existing.file_paths.clone();
    merged_files.extend(new_file_paths);
    existing.file_paths = dedup(&merged_files);
    if !tags.is_empty() {
        let mut tagset = existing.tags.clone();
        for t in tags {
            if !tagset.contains(t) {
                tagset.push(t.clone());
            }
        }
        existing.tags = tagset;
    }
    if !note.is_empty() {
        existing.note = note.to_string();
    }
    if expires_at.is_some() {
        existing.expires_at = expires_at;
    }
    existing.updated_at = now;
    Ok(StashAction::Merged)
}

pub fn get_stash<'a>(palace: &'a Palace, name: &str) -> Option<&'a Stash> {
    palace.stashes.get(name)
}

/// Drop a stash; returns whether it existed. v0.x uses `delete`, which on
/// an IndexMap we mirror with **shift_remove** to preserve the order of
/// the remaining keys (swap_remove would reorder and break byte-equivalence).
pub fn drop_stash(palace: &mut Palace, name: &str) -> bool {
    palace.stashes.shift_remove(name).is_some()
}

/// List stashes, optionally filtered to those carrying ALL `tag_filter` tags.
pub fn list_stashes<'a>(palace: &'a Palace, tag_filter: &[String]) -> Vec<&'a Stash> {
    palace
        .stashes
        .values()
        .filter(|s| tag_filter.is_empty() || tag_filter.iter().all(|t| s.tags.contains(t)))
        .collect()
}

/// Union of multiple stashes' file source ids (port of `composeToSources`).
/// Errors listing all unknown names.
pub fn compose_to_sources(palace: &Palace, names: &[String]) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for name in names {
        match palace.stashes.get(name) {
            None => missing.push(name.clone()),
            Some(stash) => {
                for id in stash_to_source_ids(stash) {
                    if seen.insert(id.clone()) {
                        out.push(id);
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "Unknown stashes: {}. Run 'scrt --mp-list' to see available stashes.",
            missing.join(", ")
        ));
    }
    Ok(out)
}

/// Set difference: files in `a` not in any of `b` (port of `exceptToSources`).
pub fn except_to_sources(palace: &Palace, a: &str, b: &[String]) -> Result<Vec<String>, String> {
    let base = palace.stashes.get(a).ok_or_else(|| unknown_stash(a))?;
    let mut exclude = std::collections::HashSet::new();
    for name in b {
        let stash = palace
            .stashes
            .get(name)
            .ok_or_else(|| unknown_stash(name))?;
        for id in stash_to_source_ids(stash) {
            exclude.insert(id);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in stash_to_source_ids(base) {
        if exclude.contains(&id) || !seen.insert(id.clone()) {
            continue;
        }
        out.push(id);
    }
    Ok(out)
}

/// Set intersection: files in ALL given stashes (port of `intersectToSources`).
pub fn intersect_to_sources(palace: &Palace, names: &[String]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut sets: Vec<std::collections::HashSet<String>> = Vec::new();
    for name in names {
        let stash = palace
            .stashes
            .get(name)
            .ok_or_else(|| unknown_stash(name))?;
        sets.push(stash_to_source_ids(stash).into_iter().collect());
    }
    let (first, rest) = sets.split_first().unwrap();
    // Preserve first stash's file order (iterate its ids, not the set).
    let first_ordered = {
        let stash = palace.stashes.get(&names[0]).unwrap();
        stash_to_source_ids(stash)
    };
    let mut out = Vec::new();
    for id in first_ordered {
        if first.contains(&id) && rest.iter().all(|s| s.contains(&id)) {
            out.push(id);
        }
    }
    Ok(out)
}

fn unknown_stash(name: &str) -> String {
    format!("Unknown stash: {name}. Run 'scrt --mp-list' to see available stashes.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Source;

    /// Fixed clock for deterministic timestamps in tests.
    pub struct FixedClock(pub i64);
    impl Clock for FixedClock {
        fn now_iso(&self) -> String {
            iso_from_ms(self.0)
        }
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn iso_matches_js_to_iso_string() {
        // Reference values captured from Node `new Date(ms).toISOString()`.
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1), "1970-01-01T00:00:00.001Z");
        assert_eq!(iso_from_ms(1718270520000), "2024-06-13T09:22:00.000Z");
        assert_eq!(iso_from_ms(1750000000123), "2025-06-15T15:06:40.123Z");
        assert_eq!(iso_from_ms(1577836800000), "2020-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1735689599999), "2024-12-31T23:59:59.999Z");
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s").unwrap(), 30_000);
        assert_eq!(parse_duration("10m").unwrap(), 600_000);
        assert_eq!(parse_duration("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration("7d").unwrap(), 604_800_000);
        assert_eq!(parse_duration("500").unwrap(), 500); // bare ms
        assert!(parse_duration("nonsense").is_err());
    }

    #[test]
    fn hash_line_is_12_hex() {
        let h = hash_line("hello world").unwrap();
        assert_eq!(h.len(), 12);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Trimmed: leading/trailing ws doesn't change the hash.
        assert_eq!(hash_line("  hello world  ").unwrap(), h);
    }

    fn node(id: &str, line: u64) -> Node {
        Node {
            id: 1,
            source: Source::file(id),
            match_line: line,
            start_line: line,
            end_line: line,
            context_before: vec![],
            match_text: format!("line {line}"),
            context_after: vec![],
            match_spans: vec![[0, 1]],
            tokens: 1,
        }
    }

    fn search_meta() -> StashSearch {
        StashSearch {
            pattern: "p".into(),
            effort: "normal".into(),
            sources_count: 1,
        }
    }

    #[test]
    fn add_create_then_merge_dedups() {
        let clock = FixedClock(1000);
        let mut p = Palace::empty();
        let nodes = vec![node("a.txt", 1), node("a.txt", 2)];
        let act = add_stash(
            &mut p,
            &clock,
            "s",
            "note",
            &nodes,
            search_meta(),
            &["a.txt".into()],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        assert_eq!(act, StashAction::Created);
        assert_eq!(p.stashes["s"].nodes.len(), 2);

        // Merge with an overlapping + a new node -> dedup keeps 3 total.
        let more = vec![node("a.txt", 2), node("a.txt", 3)];
        let act = add_stash(
            &mut p,
            &clock,
            "s",
            "note",
            &more,
            search_meta(),
            &["a.txt".into()],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        assert_eq!(act, StashAction::Merged);
        assert_eq!(p.stashes["s"].nodes.len(), 3);
    }

    #[test]
    fn replace_overwrites_nodes() {
        let clock = FixedClock(1000);
        let mut p = Palace::empty();
        add_stash(
            &mut p,
            &clock,
            "s",
            "n",
            &[node("a.txt", 1), node("a.txt", 2)],
            search_meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        let opts = StashOptions {
            replace: true,
            ..Default::default()
        };
        let act = add_stash(
            &mut p,
            &clock,
            "s",
            "n2",
            &[node("a.txt", 9)],
            search_meta(),
            &[],
            &[],
            &opts,
        )
        .unwrap();
        assert_eq!(act, StashAction::Replaced);
        assert_eq!(p.stashes["s"].nodes.len(), 1);
        assert_eq!(p.stashes["s"].note, "n2");
    }

    #[test]
    fn compose_intersect_except() {
        let clock = FixedClock(1);
        let mut p = Palace::empty();
        add_stash(
            &mut p,
            &clock,
            "a",
            "",
            &[node("x.txt", 1), node("y.txt", 1)],
            search_meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();
        add_stash(
            &mut p,
            &clock,
            "b",
            "",
            &[node("y.txt", 1), node("z.txt", 1)],
            search_meta(),
            &[],
            &[],
            &StashOptions::default(),
        )
        .unwrap();

        let comp = compose_to_sources(&p, &["a".into(), "b".into()]).unwrap();
        assert_eq!(comp, vec!["x.txt", "y.txt", "z.txt"]); // union, first-seen order

        let inter = intersect_to_sources(&p, &["a".into(), "b".into()]).unwrap();
        assert_eq!(inter, vec!["y.txt"]); // common

        let exc = except_to_sources(&p, "a", &["b".into()]).unwrap();
        assert_eq!(exc, vec!["x.txt"]); // in a, not b

        assert!(compose_to_sources(&p, &["nope".into()]).is_err());
    }

    #[test]
    fn drop_preserves_order_of_rest() {
        let clock = FixedClock(1);
        let mut p = Palace::empty();
        for n in ["a", "b", "c"] {
            add_stash(
                &mut p,
                &clock,
                n,
                "",
                &[node("x.txt", 1)],
                search_meta(),
                &[],
                &[],
                &StashOptions::default(),
            )
            .unwrap();
        }
        assert!(drop_stash(&mut p, "b"));
        let keys: Vec<&str> = p.stashes.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["a", "c"]); // order preserved (shift_remove)
        assert!(!drop_stash(&mut p, "b"));
    }
}
