//! SimHash similarity over palace stashes — the `--mp-similar` engine.
//!
//! Design (DESIGN.md §2.4): each stash gets a two-segment fingerprint
//! computed from its text:
//!
//! ```text
//! <simhash(note)>-<simhash(note + body)>
//!        ▲                  ▲
//!    "what I was        "what I was looking for
//!     looking for"       AND what I found"
//! ```
//!
//! - **note segment** = 64-bit SimHash of the stash note alone (intent).
//! - **full segment** = 64-bit SimHash of note + captured node text
//!   (intent + content).
//!
//! SimHash (Charikar) is **locality-preserving**: near inputs produce near
//! outputs in *Hamming distance*, so similarity is a single XOR + popcount.
//! That is exactly the property a plain hash (the kind that makes an id)
//! destroys — which is why the fingerprint is a SimHash, not a content hash.
//!
//! Feature projection is **per data type** (the user's "process on their
//! axis"): prose, code, JSON and logs each shingle differently, but all
//! project to a set of string features so a single SimHash lands them in one
//! comparable 64-bit space.
//!
//! ## Byte-parity note
//!
//! Fingerprints are NOT stored in the `Stash` struct — that would add a JSON
//! key Node mpg never writes and break the byte-for-byte palace compat
//! guarantee (COMPAT.md §4). They live in a scrt-only **sidecar**
//! `<.mpg>/fingerprints.json` next to the palace file. The palace JSON stays
//! 100% Node-identical; a missing/stale sidecar is recomputed lazily.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::{Palace, Stash};

/// Sidecar filename, written beside the palace file.
pub const FINGERPRINT_SIDECAR: &str = "fingerprints.json";

/// A stash's fingerprint. Two axes, because per-type projection and
/// cross-type comparison are mutually exclusive (see the module header's
/// byte-parity note):
///
/// - `note` — prose SimHash of the note alone (intent; always comparable).
/// - `full_prose` — prose SimHash of note + body. The **universal axis**:
///   a raw `--term` query and any stash, of any type, are comparable here.
/// - `full_typed` — type-projected SimHash of note + body ("process on its
///   axis"). Only comparable between stashes of the **same** `dtype`.
/// - `dtype` — the inferred data type, so the ranker knows when two stashes
///   share the typed axis and can use it instead of the prose one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub note: u64,
    pub full_prose: u64,
    pub full_typed: u64,
    pub dtype: DataType,
}

impl Fingerprint {
    /// Render as the `<note>-<full_prose>` hex id form used in output (the two
    /// universally-comparable segments — the typed axis is an internal detail).
    pub fn to_id(self) -> String {
        format!("{:016x}-{:016x}", self.note, self.full_prose)
    }
}

/// Word-window chunking parameters: a `width`-word window slid by `stride`.
/// Overlap (`width > stride`) softens the "one insertion shifts everything"
/// weakness of fixed windows — adjacent windows share words, so a shift only
/// degrades alignment locally rather than globally.
pub const CHUNK_WIDTH: usize = 12;
pub const CHUNK_STRIDE: usize = 6;
/// Number of MinHash permutations for the chunk-set Jaccard estimate.
pub const MINHASH_K: usize = 16;

/// A chunked fingerprint: the body, windowed into overlapping spans, with one
/// SimHash per window. This restores **local** similarity that the single
/// whole-stash SimHash destroys — two stashes that share one section have one
/// pair of near chunks, even if the rest is unrelated.
///
/// Stored per axis: `prose` (universal — comparable to terms + any stash) and
/// `typed` (same-`dtype` only). Both the raw chunk hashes (for best-pair
/// matching) and a `MINHASH_K`-wide MinHash signature (for set-Jaccard) are
/// kept; the signature is derived from the chunk set, no extra shingling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkSet {
    /// Per-window SimHashes over the prose-projected feature stream.
    pub prose: Vec<u64>,
    /// Per-window SimHashes over the type-projected feature stream.
    pub typed: Vec<u64>,
    /// MinHash signature of the prose chunk set (Jaccard estimator).
    pub prose_minhash: Vec<u64>,
    /// MinHash signature of the typed chunk set.
    pub typed_minhash: Vec<u64>,
}

/// Random-projection ("hashing-trick") embedding: a fixed-dimension float
/// vector per axis. Each token sign-hashes into `RANDPROJ_DIM` dimensions; the
/// vectors are summed and L2-normalized so cosine similarity is a plain dot
/// product. This is **SimHash's cousin that keeps magnitude instead of
/// collapsing to bits** — a *weighted* lexical signal, smoother than Hamming.
///
/// HONEST LIMIT: this is still a **lexical** signal. Two snippets sharing no
/// tokens are near-orthogonal regardless of meaning — "dog Rex" vs "my pet's
/// name" stays ~0. It does NOT bridge the semantic gap; only a trained model
/// does (the `scrt-evolve` path). It's a *better lexical* signal, not a
/// semantic one. Stored as f32 to keep the sidecar compact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StashVector {
    pub prose: Vec<f32>,
    pub typed: Vec<f32>,
}

/// A stash's full cached signature: the scalar `Fingerprint` (cheap single-XOR
/// path + id-form) flattened inline for back-compat, plus the optional chunked
/// `ChunkSet` and the optional random-projection `StashVector`. An older
/// sidecar written before a field existed simply lacks that key — it
/// deserializes with the field `None` and recomputes lazily on next reconcile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashSignature {
    #[serde(flatten)]
    pub scalar: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<ChunkSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<StashVector>,
}

/// The on-disk sidecar: `{ "<stash-name>": {note, full_prose, …, chunks}, … }`.
/// A plain map — no version field, no brand string — so it's trivially
/// forward/back compatible and recomputable from scratch at any time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintSidecar {
    #[serde(flatten)]
    pub by_stash: BTreeMap<String, StashSignature>,
}

/// Which fingerprint segment a query compares against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchAxis {
    /// Compare against the note segment — match on *intent*.
    Note,
    /// Compare against the note+body segment — match on *intent + content*.
    Full,
    /// Compare via the random-projection vector (weighted-cosine lexical).
    /// Same axis-selection rules as `Full` (typed when dtypes match, else prose).
    Vector,
}

/// A data-type axis for feature projection. Inferred per stash from its
/// captured sources / content; drives how text is shingled before hashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Prose,
    Code,
    Json,
    Log,
}

/// The sidecar path for a given palace file: same directory, `fingerprints.json`.
pub fn sidecar_path(palace_path: &Path) -> PathBuf {
    palace_path
        .parent()
        .map(|p| p.join(FINGERPRINT_SIDECAR))
        .unwrap_or_else(|| PathBuf::from(FINGERPRINT_SIDECAR))
}

/// Load the sidecar, or an empty one if absent/corrupt (it's a cache —
/// never fatal; a bad sidecar just means everything recomputes).
pub fn load_sidecar(palace_path: &Path) -> FingerprintSidecar {
    let path = sidecar_path(palace_path);
    match std::fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw).unwrap_or_default(),
        _ => FingerprintSidecar::default(),
    }
}

/// Persist the sidecar (atomic tmp+rename, like the palace). Best-effort:
/// the fingerprint cache is recomputable, so a write failure is non-fatal.
pub fn save_sidecar(palace_path: &Path, sidecar: &FingerprintSidecar) -> std::io::Result<()> {
    let path = sidecar_path(palace_path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let body = serde_json::to_string_pretty(sidecar).unwrap_or_else(|_| "{}".into()) + "\n";
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Reconcile the sidecar against the live palace: recompute fingerprints for
/// stashes that are missing or whose names changed, drop entries for stashes
/// that no longer exist. Returns the up-to-date sidecar and whether it changed
/// (so the caller can decide to persist).
pub fn reconcile(palace: &Palace, sidecar: &FingerprintSidecar) -> (FingerprintSidecar, bool) {
    let mut out = FingerprintSidecar::default();
    let mut changed = false;
    for (name, stash) in &palace.stashes {
        match sidecar.by_stash.get(name) {
            // Reuse only a *complete* signature; an older entry missing chunks
            // OR the vector is upgraded (recomputed) so every path has data.
            Some(sig) if sig.chunks.is_some() && sig.vector.is_some() => {
                out.by_stash.insert(name.clone(), sig.clone());
            }
            _ => {
                out.by_stash.insert(name.clone(), signature_stash(stash));
                changed = true;
            }
        }
    }
    // Dropped stashes: present in old sidecar, absent from palace.
    if sidecar
        .by_stash
        .keys()
        .any(|k| !palace.stashes.contains_key(k))
    {
        changed = true;
    }
    (out, changed)
}

/// Compute a stash's scalar fingerprint from its text. Produces both axes:
/// `full_prose` (universal, comparable to terms and cross-type) and
/// `full_typed` (type-projected, comparable only to same-`dtype` stashes).
pub fn fingerprint_stash(stash: &Stash) -> Fingerprint {
    let dtype = infer_data_type(stash);
    let note = simhash(&project(&stash.note, DataType::Prose)); // the note is always prose
    let body = stash_body(stash);
    let full_text = if body.is_empty() {
        stash.note.clone()
    } else {
        format!("{}\n{}", stash.note, body)
    };
    let full_prose = simhash(&project(&full_text, DataType::Prose));
    // When the content IS prose, the typed axis equals the prose axis — no
    // need to recompute, and it keeps same-type prose stashes self-consistent.
    let full_typed = if dtype == DataType::Prose {
        full_prose
    } else {
        simhash(&project(&full_text, dtype))
    };
    Fingerprint {
        note,
        full_prose,
        full_typed,
        dtype,
    }
}

/// Compute the FULL signature: the scalar fingerprint **plus** the chunked
/// per-window SimHash arrays + their MinHash signatures, on both axes.
pub fn signature_stash(stash: &Stash) -> StashSignature {
    let scalar = fingerprint_stash(stash);
    let body = stash_body(stash);
    let full_text = if body.is_empty() {
        stash.note.clone()
    } else {
        format!("{}\n{}", stash.note, body)
    };
    let chunks = chunk_set(&full_text, scalar.dtype);
    let vector = vector_set(&full_text, scalar.dtype);
    StashSignature {
        scalar,
        chunks: Some(chunks),
        vector: Some(vector),
    }
}

/// Build the chunked fingerprint for a body of text on both axes.
fn chunk_set(full_text: &str, dtype: DataType) -> ChunkSet {
    let prose = chunk_hashes(full_text, DataType::Prose);
    // Prose content reuses the prose chunks for the typed axis (same projection).
    let typed = if dtype == DataType::Prose {
        prose.clone()
    } else {
        chunk_hashes(full_text, dtype)
    };
    let prose_minhash = minhash_of(&prose);
    let typed_minhash = if dtype == DataType::Prose {
        prose_minhash.clone()
    } else {
        minhash_of(&typed)
    };
    ChunkSet {
        prose,
        typed,
        prose_minhash,
        typed_minhash,
    }
}

/// Build the random-projection vectors for a body of text on both axes.
fn vector_set(full_text: &str, dtype: DataType) -> StashVector {
    let prose = randproj_vector(full_text, DataType::Prose);
    let typed = if dtype == DataType::Prose {
        prose.clone()
    } else {
        randproj_vector(full_text, dtype)
    };
    StashVector { prose, typed }
}

/// Project text on its axis, then slide a `CHUNK_WIDTH`-feature window by
/// `CHUNK_STRIDE`, emitting one SimHash per window. Short inputs (fewer
/// features than a window) collapse to a single whole-content chunk so they're
/// still comparable. Returns chunk hashes in scan order; duplicates kept
/// (repetition is signal for the MinHash set).
fn chunk_hashes(full_text: &str, dtype: DataType) -> Vec<u64> {
    let feats = project(full_text, dtype);
    if feats.is_empty() {
        return Vec::new();
    }
    if feats.len() <= CHUNK_WIDTH {
        return vec![simhash(&feats)];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < feats.len() {
        let end = (start + CHUNK_WIDTH).min(feats.len());
        out.push(simhash(&feats[start..end]));
        if end == feats.len() {
            break;
        }
        start += CHUNK_STRIDE;
    }
    out
}

/// MinHash signature of a chunk-hash multiset: for each of `MINHASH_K`
/// permutations (xor-mix by a distinct seed), keep the minimum permuted chunk
/// hash. Two stashes' Jaccard ≈ fraction of signature positions that agree.
fn minhash_of(chunks: &[u64]) -> Vec<u64> {
    if chunks.is_empty() {
        return vec![u64::MAX; MINHASH_K];
    }
    (0..MINHASH_K)
        .map(|i| {
            let seed = MINHASH_SEEDS[i];
            chunks
                .iter()
                .map(|&h| mix64(h ^ seed))
                .min()
                .unwrap_or(u64::MAX)
        })
        .collect()
}

/// Distinct seeds for the MinHash permutations (a fixed table → deterministic,
/// reproducible signatures across processes). Generated by mixing an index.
static MINHASH_SEEDS: [u64; MINHASH_K] = {
    // splitmix64-style constants spread across the table.
    [
        0x9e3779b97f4a7c15,
        0xbf58476d1ce4e5b9,
        0x94d049bb133111eb,
        0x2545f4914f6cdd1d,
        0xd1b54a32d192ed03,
        0xaef17502108ef2d9,
        0xf1bbcdcbfa53e0ab,
        0x6a09e667f3bcc909,
        0x3c6ef372fe94f82b,
        0x510e527fade682d1,
        0x1f83d9abfb41bd6b,
        0x5be0cd19137e2179,
        0xc3a5c85c97cb3127,
        0xb492b66fbe98f273,
        0x9ae16a3b2f90404f,
        0xcbf29ce484222325,
    ]
};

/// A fast 64-bit avalanche mix (splitmix64 finalizer) — used as the MinHash
/// permutation function over chunk hashes.
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Join a stash's captured node text (match + context) into one body string.
fn stash_body(stash: &Stash) -> String {
    stash
        .nodes
        .iter()
        .flat_map(|n| {
            n.context_before
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(n.match_text.as_str()))
                .chain(n.context_after.iter().map(String::as_str))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Infer the dominant data type of a stash from its captured sources (file
/// extensions) and content shape. Cheap heuristic — the projection only needs
/// to be *roughly* right to help, and prose is a safe default.
pub fn infer_data_type(stash: &Stash) -> DataType {
    // 1. Extension vote from file sources.
    let mut code = 0u32;
    let mut json = 0u32;
    let mut log = 0u32;
    for src in &stash.sources {
        let lower = src.to_ascii_lowercase();
        if lower.ends_with(".json") || lower.ends_with(".ndjson") {
            json += 1;
        } else if lower.ends_with(".log") || lower.contains(".log.") {
            log += 1;
        } else if is_code_ext(&lower) {
            code += 1;
        }
    }
    if json > code && json > log && json > 0 {
        return DataType::Json;
    }
    if log >= code && log > 0 {
        return DataType::Log;
    }
    if code > 0 {
        return DataType::Code;
    }
    // 2. No telling extensions — sniff the body.
    let body = stash_body(stash);
    sniff_data_type(&body)
}

fn is_code_ext(lower: &str) -> bool {
    const CODE_EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".c", ".h", ".cpp", ".hpp",
        ".cc", ".cs", ".rb", ".php", ".swift", ".kt", ".scala", ".sh", ".lua", ".sql", ".toml",
        ".yaml", ".yml",
    ];
    CODE_EXTS.iter().any(|e| lower.ends_with(e))
}

/// Content sniff when extensions don't decide it.
fn sniff_data_type(body: &str) -> DataType {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        // Looks JSON-ish if it's mostly braces/brackets/quotes/colons.
        let structural = body
            .chars()
            .filter(|c| matches!(c, '{' | '}' | '[' | ']' | ':' | '"' | ','))
            .count();
        if structural * 6 >= body.len().max(1) {
            return DataType::Json;
        }
    }
    // Log heuristic: many lines starting with a timestamp/level token.
    let mut log_lines = 0u32;
    let mut total_lines = 0u32;
    for line in body.lines().take(64) {
        total_lines += 1;
        if looks_like_log_line(line) {
            log_lines += 1;
        }
    }
    if total_lines > 0 && log_lines * 2 >= total_lines {
        return DataType::Log;
    }
    // Code heuristic: presence of common syntax markers across lines.
    let code_markers = body.matches([';', '{', '}', '(', ')']).count();
    if code_markers * 12 >= body.len().max(1) {
        return DataType::Code;
    }
    DataType::Prose
}

fn looks_like_log_line(line: &str) -> bool {
    let l = line.trim_start();
    // ISO-ish timestamp prefix, or a level token near the start.
    let timestamp_prefix = l.starts_with(|c: char| c.is_ascii_digit())
        && l.contains(':')
        && (l.contains('-') || l.contains('/'));
    timestamp_prefix
        || l.starts_with("INFO")
        || l.starts_with("WARN")
        || l.starts_with("ERROR")
        || l.starts_with("DEBUG")
        || l.starts_with("TRACE")
        || l.starts_with("FATAL")
}

/// Project text to a set of weighted string features, shingled on the axis
/// of its data type. The returned vec may contain duplicates — repetition is
/// the feature weight SimHash sums over.
pub fn project(text: &str, dtype: DataType) -> Vec<String> {
    match dtype {
        DataType::Prose => prose_shingles(text),
        DataType::Code => code_shingles(text),
        DataType::Json => json_features(text),
        DataType::Log => log_shingles(text),
    }
}

/// Prose: lowercased, stopword-stripped, with BOTH unigrams and 3-shingles.
///
/// The unigrams carry shared *vocabulary* (so two notes with the same words in
/// a different order still register as similar — without them, short reordered
/// notes have zero contiguous-trigram overlap and look unrelated). The
/// 3-shingles add *phrase* signal so longer texts that share whole phrases pull
/// closer than ones that merely share a bag of words. Each unigram is tagged so
/// it can't collide with a trigram feature.
fn prose_shingles(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| !is_stopword(w))
        .collect();
    let mut feats: Vec<String> = words.iter().map(|w| format!("w:{w}")).collect();
    feats.extend(shingle(&words, 3));
    feats
}

/// Code: token unigrams + 2-grams, identifiers length-normalized so `userId`
/// and `userName` don't blow up the feature space. Unigrams carry shared
/// identifiers/keywords (order-independent); 2-grams add local structure.
fn code_shingles(text: &str) -> Vec<String> {
    let tokens: Vec<String> = text
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '(' | ')' | '{' | '}' | '[' | ']' | ';' | ',' | '.')
        })
        .filter(|t| !t.is_empty())
        .map(normalize_ident)
        .collect();
    let mut feats: Vec<String> = tokens.iter().map(|t| format!("t:{t}")).collect();
    feats.extend(shingle(&tokens, 2));
    feats
}

/// JSON: the sorted SET of key-paths (shape, not values). Identical shape →
/// identical feature set regardless of value churn.
fn json_features(text: &str) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => {
            let mut paths = Vec::new();
            collect_key_paths(&v, String::new(), &mut paths);
            paths.sort();
            paths.dedup();
            if paths.is_empty() {
                prose_shingles(text)
            } else {
                paths
            }
        }
        // Not valid JSON (e.g. a snippet) — fall back to prose shingles.
        Err(_) => prose_shingles(text),
    }
}

/// Logs: strip volatile tokens (timestamps, ips, numbers, hex/uuids) to
/// `<*>`, then shingle the resulting templates.
fn log_shingles(text: &str) -> Vec<String> {
    let templates: Vec<String> = text.lines().map(templatize_log_line).collect();
    let joined = templates.join(" ");
    let words: Vec<String> = joined
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    shingle(&words, 3)
}

/// Replace volatile tokens in a log line with `<*>` so two lines of the same
/// shape collide.
fn templatize_log_line(line: &str) -> String {
    line.split_whitespace()
        .map(|tok| {
            if is_volatile_token(tok) {
                "<*>".to_string()
            } else {
                tok.to_ascii_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_volatile_token(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | ',' | ';' | '"'));
    if t.is_empty() {
        return false;
    }
    // Pure number, or contains a digit and is mostly digits/punct (ids, ips,
    // timestamps, hex), or long hex/uuid-ish.
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let total = t.chars().count();
    if digits == total {
        return true;
    }
    if digits > 0 && digits * 2 >= total {
        return true;
    }
    // Hex/uuid-ish: long and all hex+dashes.
    total >= 8 && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Walk a JSON value, collecting dotted key-paths (with `[]` for arrays).
fn collect_key_paths(v: &serde_json::Value, prefix: String, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.push(path.clone());
                collect_key_paths(child, path, out);
            }
        }
        serde_json::Value::Array(items) => {
            let path = format!("{prefix}[]");
            // Recurse into the first element only — array shape, not length.
            if let Some(first) = items.first() {
                collect_key_paths(first, path, out);
            } else {
                out.push(path);
            }
        }
        _ => {} // scalar leaf: the key-path was already pushed by the parent
    }
}

/// Normalize an identifier so casing/separator variants converge a little:
/// lowercased; runs of digits collapsed to `#`.
fn normalize_ident(tok: &str) -> String {
    let lower = tok.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_digit = false;
    for c in lower.chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out
}

/// k-gram shingles over a token list. k=1 falls back to the tokens themselves.
fn shingle(tokens: &[String], k: usize) -> Vec<String> {
    if tokens.is_empty() {
        return Vec::new();
    }
    if tokens.len() < k || k <= 1 {
        return tokens.to_vec();
    }
    tokens.windows(k).map(|w| w.join("\u{1f}")).collect()
}

/// A tiny English stopword set — enough to keep prose shingles signal-heavy
/// without pulling a dictionary dependency.
fn is_stopword(w: &str) -> bool {
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "for", "with", "is", "are",
        "was", "were", "be", "been", "this", "that", "it", "as", "at", "by", "from", "we", "i",
        "you", "they", "he", "she", "if", "so", "do", "does", "did", "not", "no",
    ];
    STOP.contains(&w)
}

/// 64-bit SimHash (Charikar) over a feature multiset. Each feature votes ±1
/// on each of 64 bit positions, weighted by its hash; the sign of each
/// column's sum becomes the output bit. Repeated features increase weight.
pub fn simhash(features: &[String]) -> u64 {
    if features.is_empty() {
        return 0;
    }
    let mut acc = [0i32; 64];
    for f in features {
        let h = feature_hash(f);
        for (bit, slot) in acc.iter_mut().enumerate() {
            if (h >> bit) & 1 == 1 {
                *slot += 1;
            } else {
                *slot -= 1;
            }
        }
    }
    let mut out = 0u64;
    for (bit, &v) in acc.iter().enumerate() {
        if v > 0 {
            out |= 1 << bit;
        }
    }
    out
}

/// Hash one feature to 64 bits. Uses the std `DefaultHasher` (SipHash) — no
/// crypto needed (this isn't security), just a good avalanche so each
/// feature spreads across the 64 columns.
fn feature_hash(f: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.hash(&mut h);
    h.finish()
}

/// Hamming distance between two 64-bit SimHashes (0 = identical, 64 = opposite).
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// **Best-pair** similarity between two chunk-hash arrays, in 0.0..=1.0.
///
/// For each query chunk, find its closest candidate chunk (min Hamming),
/// convert to a per-chunk closeness, and average the **top-k** of those. This
/// rewards stashes that share *any* section: one strongly-matching chunk pair
/// pulls the score up even when the rest is unrelated — the locality the single
/// whole-stash SimHash throws away. `k` is min(len) capped, so a tiny stash
/// isn't diluted by a huge one's unmatched chunks.
pub fn best_pair_similarity(query: &[u64], cand: &[u64]) -> f64 {
    if query.is_empty() || cand.is_empty() {
        return 0.0;
    }
    let mut per_chunk: Vec<f64> = query
        .iter()
        .map(|&q| {
            let best = cand.iter().map(|&c| hamming(q, c)).min().unwrap_or(64);
            1.0 - (best as f64 / 64.0)
        })
        .collect();
    // Average the top-k closeness values, k = the top third of query chunks
    // (floor 1). Small k → "shares a section" wins over "matches on average":
    // one strongly-matched chunk pulls the score up without being diluted by
    // the query's unrelated remainder. This is the locality property.
    per_chunk.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let k = (query.len().div_ceil(3)).max(1).min(per_chunk.len());
    per_chunk.iter().take(k).sum::<f64>() / k as f64
}

/// **Set-Jaccard** estimate between two MinHash signatures, in 0.0..=1.0: the
/// fraction of signature positions where the two minima agree. Measures overall
/// content overlap / near-duplication (vs best-pair's "share one section").
pub fn minhash_jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let agree = a.iter().zip(b).filter(|(x, y)| x == y).count();
    agree as f64 / a.len() as f64
}

/// Dimensionality of the random-projection vector. 128 floats ≈ 512 bytes per
/// axis — small enough for the sidecar, wide enough that token collisions are
/// rare for short snippets.
pub const RANDPROJ_DIM: usize = 128;

/// Project text on its axis, then build the hashing-trick vector: each feature
/// hashes to (a) a dimension index and (b) a ±1 sign; we accumulate the signs
/// into that dimension, then L2-normalize. Summing signed contributions (the
/// "signed hashing trick") keeps the estimate unbiased under collisions. The
/// result is a unit vector, so cosine similarity is a plain dot product.
pub fn randproj_vector(text: &str, dtype: DataType) -> Vec<f32> {
    let feats = project(text, dtype);
    let mut v = vec![0.0f32; RANDPROJ_DIM];
    for f in &feats {
        let h = feature_hash(f);
        let dim = (h % RANDPROJ_DIM as u64) as usize;
        // A separate bit of the hash picks the sign (decorrelated from `dim`).
        let sign = if (h >> 63) & 1 == 1 { 1.0 } else { -1.0 };
        v[dim] += sign;
    }
    l2_normalize(&mut v);
    v
}

/// L2-normalize in place; a zero vector stays zero (cosine with it is 0).
fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity between two L2-normalized vectors = their dot product,
/// clamped to 0..1 (negative cosines — anti-correlated — read as "unrelated").
/// Returns 0 for mismatched/empty vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    (dot as f64).clamp(0.0, 1.0)
}

/// Blend the two chunk metrics into one 0.0..=1.0 score. Best-pair drives
/// retrieval ("shares a section"); Jaccard contributes the near-dup signal.
/// Weighted toward best-pair because partial-overlap retrieval is the common
/// case; Jaccard mostly rescues true duplicates that best-pair already scores
/// high anyway.
///
/// **Additive, not averaged.** A weighted average (`0.7·bp + 0.3·jac`) punishes
/// the common small-palace case where `jaccard == 0` for real-but-not-duplicate
/// matches — it'd drag an 0.86 best-pair down to 0.60. Instead best-pair is the
/// **base** and Jaccard only *adds* confidence on top (capped at 1.0). So a
/// strong partial-overlap match keeps its score when there's no dup signal, and
/// a true near-dup (high Jaccard) gets a bonus.
pub fn blend_chunk_score(best_pair: f64, jaccard: f64) -> f64 {
    (best_pair + 0.25 * jaccard).min(1.0)
}

/// One ranked neighbor in a `--mp-similar` result.
#[derive(Debug, Clone)]
pub struct SimilarHit {
    pub name: String,
    /// Hamming distance on the chosen axis (0..=64). Lower = closer. For the
    /// chunked path this is the scalar whole-stash distance, kept for display.
    pub distance: u32,
    /// The HONEST, method-normalized closeness in 0.0..=1.0 (1.0 = identical).
    /// This is the **displayed** score and what a link threshold compares
    /// against — NOT distorted by the `--score` gamma (that shapes ranking
    /// only). Comparable across methods (chunked blend vs scalar Hamming).
    pub relevance: f64,
    /// The gamma-shaped sort key (`relevance ^ gamma`). Drives ranking order
    /// so `--score` can widen/tighten the spread without changing the shown
    /// `relevance`. Not for display.
    pub rank_weight: f64,
    /// Which axis the distance was measured on.
    pub axis_used: AxisUsed,
    /// How the relevance was computed — chunked blend vs scalar fallback.
    pub method: SimMethod,
    /// Best-pair chunk similarity (0..1), when the chunked path ran.
    pub best_pair: Option<f64>,
    /// MinHash set-Jaccard (0..1), when the chunked path ran.
    pub jaccard: Option<f64>,
    pub fingerprint: Fingerprint,
}

/// The axis a hit's distance was measured on — surfaced so the formatter (and
/// a curious user) can see when a same-type typed comparison was used vs the
/// universal prose fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisUsed {
    Note,
    FullProse,
    FullTyped,
}

/// How a hit's relevance was scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimMethod {
    /// Single whole-stash SimHash Hamming closeness (note axis, or no chunks).
    Scalar,
    /// Chunked: blend of best-pair + MinHash-Jaccard over per-window hashes.
    Chunked,
    /// Random-projection cosine (weighted-cosine lexical "embedding").
    RandProj,
}

/// A resolved query: the scalar SimHashes to compare against, the query's data
/// type when it came from a named stash (enables the typed axis), and the
/// query's **chunk arrays** for the chunked best-pair / Jaccard comparison.
/// A raw `--term` has no type, so `typed` is `None` and it always compares on
/// the universal prose axis.
#[derive(Debug, Clone)]
pub struct SimQuery {
    pub note: u64,
    pub full_prose: u64,
    /// (typed hash, dtype) — present only for a named-stash query.
    pub typed: Option<(u64, DataType)>,
    /// The query's own chunk set (both axes), driving the chunked comparison.
    pub chunks: ChunkSet,
    /// The query's random-projection vectors (both axes), for `--match vector`.
    pub vector: StashVector,
}

impl SimQuery {
    /// Query built from a full stash signature (carries the typed axis + chunks
    /// + vector).
    pub fn from_signature(sig: &StashSignature) -> Self {
        let fp = sig.scalar;
        SimQuery {
            note: fp.note,
            full_prose: fp.full_prose,
            typed: Some((fp.full_typed, fp.dtype)),
            chunks: sig.chunks.clone().unwrap_or_default(),
            vector: sig.vector.clone().unwrap_or_default(),
        }
    }

    /// Query built from a raw term (prose only — no typed axis, so it stays
    /// comparable across every stash type). Chunks + vectors the term on prose.
    pub fn from_term(term: &str) -> Self {
        let prose = simhash(&project(term, DataType::Prose));
        let prose_chunks = chunk_hashes(term, DataType::Prose);
        let prose_minhash = minhash_of(&prose_chunks);
        let prose_vec = randproj_vector(term, DataType::Prose);
        SimQuery {
            note: prose,
            full_prose: prose,
            typed: None,
            chunks: ChunkSet {
                prose: prose_chunks.clone(),
                typed: prose_chunks,
                prose_minhash: prose_minhash.clone(),
                typed_minhash: prose_minhash,
            },
            vector: StashVector {
                prose: prose_vec.clone(),
                typed: prose_vec,
            },
        }
    }
}

/// Rank all stashes (except the query itself, if it's a named stash) by
/// similarity. `score` (1..=10) reshapes the falloff steepness — it is NOT a
/// cutoff: the full ranked list is always returned, `top` just truncates it.
///
/// Axis selection (the correctness fix): on `MatchAxis::Full`, the ranker uses
/// the **typed** axis only when the query has a type AND the candidate shares
/// it — otherwise it falls back to the universal **prose** axis. This makes
/// every comparison well-defined: you never measure Hamming distance between
/// two SimHashes computed under different projections.
///
/// Falloff: relevance = (1 - distance/64) ^ gamma, where gamma grows with the
/// score. score 1 → gentle (gamma≈0.5, wide shape); score 10 → steep
/// (gamma≈5.5, only near-identical stay high).
pub fn rank_similar(
    palace: &Palace,
    sidecar: &FingerprintSidecar,
    query: &SimQuery,
    axis: MatchAxis,
    score: u8,
    exclude: Option<&str>,
    top: Option<usize>,
) -> Vec<SimilarHit> {
    let gamma = score_to_gamma(score);
    let mut hits: Vec<SimilarHit> = palace
        .stashes
        .iter()
        .filter(|(name, _)| exclude != Some(name.as_str()))
        .map(|(name, stash)| {
            // Use the cached full signature; recompute if absent.
            let sig = sidecar
                .by_stash
                .get(name)
                .cloned()
                .unwrap_or_else(|| signature_stash(stash));
            let fp = sig.scalar;

            let (target_scalar, q_scalar, axis_used, use_typed) = match axis {
                MatchAxis::Note => (fp.note, query.note, AxisUsed::Note, false),
                MatchAxis::Full | MatchAxis::Vector => match query.typed {
                    Some((q_typed, q_dtype)) if q_dtype == fp.dtype => {
                        (fp.full_typed, q_typed, AxisUsed::FullTyped, true)
                    }
                    _ => (fp.full_prose, query.full_prose, AxisUsed::FullProse, false),
                },
            };

            let distance = hamming(q_scalar, target_scalar);

            // Chunked blend: only on the Full axis, and only when BOTH sides
            // have chunk data on the chosen axis. Notes are too short to chunk.
            let chunked = if axis == MatchAxis::Full {
                sig.chunks.as_ref().and_then(|cand| {
                    let (q_chunks, q_mh, c_chunks, c_mh) = if use_typed {
                        (
                            &query.chunks.typed,
                            &query.chunks.typed_minhash,
                            &cand.typed,
                            &cand.typed_minhash,
                        )
                    } else {
                        (
                            &query.chunks.prose,
                            &query.chunks.prose_minhash,
                            &cand.prose,
                            &cand.prose_minhash,
                        )
                    };
                    if q_chunks.is_empty() || c_chunks.is_empty() {
                        return None;
                    }
                    let bp = best_pair_similarity(q_chunks, c_chunks);
                    let jac = minhash_jaccard(q_mh, c_mh);
                    Some((bp, jac))
                })
            } else {
                None
            };

            // Random-projection cosine: only on the Vector axis, when both sides
            // have a vector on the chosen sub-axis.
            let cos = if axis == MatchAxis::Vector {
                sig.vector.as_ref().and_then(|cand| {
                    let (qv, cv) = if use_typed {
                        (&query.vector.typed, &cand.typed)
                    } else {
                        (&query.vector.prose, &cand.prose)
                    };
                    if qv.is_empty() || cv.is_empty() {
                        return None;
                    }
                    Some(cosine(qv, cv))
                })
            } else {
                None
            };

            let (base, method, best_pair, jaccard) = match (cos, chunked) {
                (Some(c), _) => (c, SimMethod::RandProj, None, None),
                (None, Some((bp, jac))) => (
                    blend_chunk_score(bp, jac),
                    SimMethod::Chunked,
                    Some(bp),
                    Some(jac),
                ),
                (None, None) => (
                    1.0 - (distance as f64 / 64.0),
                    SimMethod::Scalar,
                    None,
                    None,
                ),
            };
            // `relevance` is the HONEST, method-normalized closeness (0..1) —
            // what's displayed and what a link threshold compares against. The
            // gamma `--score` falloff shapes only the SORT key (`rank_weight`),
            // so it spreads the ranking without distorting the shown number.
            // Scalar Hamming closeness clusters high (~0.5–0.9); the chunked
            // blend sits lower (~0.3–0.7), so each method gets its own affine
            // stretch to a common 0..1 display scale before gamma.
            let relevance = normalize_relevance(base, method).clamp(0.0, 1.0);
            let rank_weight = relevance.powf(gamma);
            SimilarHit {
                name: name.clone(),
                distance,
                relevance,
                rank_weight,
                axis_used,
                method,
                best_pair,
                jaccard,
                fingerprint: fp,
            }
        })
        .collect();

    hits.sort_by(|a, b| {
        b.rank_weight
            .partial_cmp(&a.rank_weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    if let Some(n) = top {
        hits.truncate(n);
    }
    hits
}

/// A stash-time link suggestion: an existing stash the freshly-saved one looks
/// related to, with the honest display relevance (0..1) that cleared the bar.
#[derive(Debug, Clone)]
pub struct LinkSuggestion {
    pub name: String,
    pub relevance: f64,
    pub method: SimMethod,
}

/// Suggest stashes the just-saved `new_name` looks related to (the "suggest
/// links as you stash" flow). Ranks it against the rest of the palace on the
/// rich `Full` axis at a neutral falloff, keeps hits whose **displayed**
/// relevance ≥ `threshold`, drops any already linked to `new_name`, and caps at
/// `max`. Returns nothing if the new stash is unknown or has no comparable
/// neighbors — callers print only when this is non-empty.
///
/// This is a *suggestion* surface: the signal is lexical/structural (SimHash),
/// so it's emitted as advice + a ready `--mp-link` command, never auto-applied.
pub fn suggest_links(
    palace: &Palace,
    sidecar: &FingerprintSidecar,
    new_name: &str,
    threshold: f64,
    max: usize,
) -> Vec<LinkSuggestion> {
    let Some(sig) = sidecar.by_stash.get(new_name) else {
        return Vec::new();
    };
    // Already-linked neighbors (either direction) are not re-suggested.
    let linked: std::collections::HashSet<String> = super::relations::get_related(palace, new_name)
        .into_iter()
        .map(|r| r.stash_name)
        .collect();

    let query = SimQuery::from_signature(sig);
    // Neutral score (5) — display relevance is score-independent anyway; we
    // filter on `relevance`, the honest normalized closeness.
    rank_similar(
        palace,
        sidecar,
        &query,
        MatchAxis::Full,
        5,
        Some(new_name),
        None,
    )
    .into_iter()
    .filter(|h| h.relevance >= threshold && !linked.contains(&h.name))
    .take(max)
    .map(|h| LinkSuggestion {
        name: h.name,
        relevance: h.relevance,
        method: h.method,
    })
    .collect()
}

/// Map a 1..=10 score to the falloff exponent. Clamped; out-of-range scores
/// snap to the ends. score 1 → 0.5 (wide), 5/6 → ~2.7 (balanced), 10 → 5.5
/// (tight).
fn score_to_gamma(score: u8) -> f64 {
    let s = score.clamp(1, 10) as f64;
    0.5 + (s - 1.0) * (5.0 / 9.0)
}

/// Stretch a raw method score to an intuitive 0..1 **display** scale, so a
/// "73% best-pair" reads ~73% and a threshold is meaningful across methods.
///
/// Each method has a different *useful* range — the floor is where unrelated
/// content sits (not 0), the ceiling is where identical content sits:
/// - **Scalar** Hamming closeness: two random 64-bit SimHashes differ in ~32
///   bits → closeness ~0.5 for unrelated, ~1.0 for identical. Useful band
///   `[0.5, 1.0]`.
/// - **Chunked** blend (0.7·best-pair + 0.3·jaccard): unrelated chunk sets
///   still share *some* nearest-pair closeness, so the floor is higher (~0.3);
///   near-identical tops out around ~0.95. Useful band `[0.30, 0.95]`.
///
/// Affine-map the band to `[0, 1]` and clamp. This is display-only — ordering
/// (driven by `rank_weight`) is unaffected by a monotonic remap.
fn normalize_relevance(raw: f64, method: SimMethod) -> f64 {
    let (lo, hi) = match method {
        SimMethod::Scalar => (0.5, 1.0),
        // best-pair for UNRELATED short content still floats ~0.60 (a nearest
        // chunk always shares some bits), so the floor is high; a genuine
        // shared section lands ~0.85+. Empirically calibrated against the smoke
        // corpus: pizza-vs-auth ≈ 0.63 → ~10%, shared-fn ≈ 0.86 → ~75%.
        SimMethod::Chunked => (0.60, 0.95),
        // Random-projection cosine: UNRELATED token sets are near-orthogonal
        // (~0), shared-vocab content climbs; non-identical text rarely tops
        // ~0.7. Band [0.05, 0.7] so a faint overlap reads low and a strong
        // lexical match reads high.
        SimMethod::RandProj => (0.05, 0.70),
    };
    ((raw - lo) / (hi - lo)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palace::types::{Stash, StashSearch, StashedNode};

    fn stash(name: &str, note: &str, body_lines: &[&str], sources: &[&str]) -> Stash {
        let nodes = if body_lines.is_empty() {
            vec![]
        } else {
            vec![StashedNode {
                source: sources.first().copied().unwrap_or("x").to_string(),
                file_path: sources.first().map(|s| s.to_string()),
                source_type: "file".into(),
                match_line: 1,
                start_line: 1,
                end_line: body_lines.len() as u64,
                context_before: vec![],
                match_text: body_lines.join("\n"),
                context_after: vec![],
                tokens: 0,
                source_mtime_ms: None,
                match_line_hash: None,
            }]
        };
        Stash {
            name: name.into(),
            note: note.into(),
            tags: vec![],
            created_at: "t".into(),
            updated_at: "t".into(),
            expires_at: None,
            search: StashSearch {
                pattern: "p".into(),
                effort: "quick".into(),
                sources_count: 0,
            },
            sources: sources.iter().map(|s| s.to_string()).collect(),
            nodes,
            file_paths: vec![],
            relations: vec![],
        }
    }

    #[test]
    fn simhash_identical_text_is_equal() {
        let a = simhash(&project("the quick brown fox jumps", DataType::Prose));
        let b = simhash(&project("the quick brown fox jumps", DataType::Prose));
        assert_eq!(a, b);
        assert_eq!(hamming(a, b), 0);
    }

    #[test]
    fn simhash_similar_text_is_closer_than_unrelated() {
        let base = simhash(&project(
            "rate limiting for the auth login endpoint",
            DataType::Prose,
        ));
        let near = simhash(&project(
            "rate limiting on the auth login route",
            DataType::Prose,
        ));
        let far = simhash(&project(
            "favourite pizza toppings and dessert recipes",
            DataType::Prose,
        ));
        assert!(
            hamming(base, near) < hamming(base, far),
            "near={} far={}",
            hamming(base, near),
            hamming(base, far)
        );
    }

    #[test]
    fn fingerprint_has_two_segments_and_id_form() {
        let s = stash(
            "auth",
            "auth rate limiting",
            &["fn login() {}"],
            &["src/auth.rs"],
        );
        let fp = fingerprint_stash(&s);
        let id = fp.to_id();
        assert!(id.contains('-'));
        assert_eq!(id.len(), 33); // 16 + '-' + 16
    }

    #[test]
    fn json_features_are_shape_not_values() {
        // Same shape, different values → identical feature set → same hash.
        let a = json_features(r#"{"user":{"id":1,"name":"alice"}}"#);
        let b = json_features(r#"{"user":{"id":999,"name":"bob"}}"#);
        assert_eq!(a, b);
        assert!(a.iter().any(|p| p == "user.id"));
    }

    #[test]
    fn log_templatizing_collapses_volatile_tokens() {
        let a =
            templatize_log_line("2026-06-15T10:00:00 ERROR request 4821 failed for ip 10.0.0.3");
        let b =
            templatize_log_line("2026-06-15T11:30:00 ERROR request 9999 failed for ip 10.0.0.9");
        assert_eq!(a, b, "same-shape log lines should templatize identically");
        assert!(a.contains("<*>"));
        assert!(a.contains("error"));
    }

    #[test]
    fn data_type_inference() {
        let code = stash("c", "n", &["fn f() { return 1; }"], &["a.rs"]);
        assert_eq!(infer_data_type(&code), DataType::Code);
        let json = stash("j", "n", &[r#"{"a":1}"#], &["a.json"]);
        assert_eq!(infer_data_type(&json), DataType::Json);
        let log = stash("l", "n", &["INFO started ok"], &["app.log"]);
        assert_eq!(infer_data_type(&log), DataType::Log);
    }

    #[test]
    fn rank_excludes_self_and_orders_by_distance() {
        let mut p = Palace::empty();
        // Use prose stashes with clearly overlapping vs disjoint vocabulary so
        // the ranking is driven by real shingle overlap, not tiny-input noise.
        let queries = [
            stash(
                "auth",
                "auth rate limiting login throttle attempts",
                &[],
                &[],
            ),
            stash(
                "auth2",
                "rate limiting auth login throttle requests",
                &[],
                &[],
            ),
            stash(
                "pizza",
                "pizza dessert recipes food toppings cheese",
                &[],
                &[],
            ),
        ];
        for s in queries {
            p.stashes.insert(s.name.clone(), s);
        }
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_signature(&sidecar.by_stash["auth"]);
        let hits = rank_similar(&p, &sidecar, &q, MatchAxis::Full, 5, Some("auth"), None);
        assert_eq!(hits.len(), 2, "self excluded");
        assert_eq!(
            hits[0].name, "auth2",
            "the other auth stash should rank first"
        );
        assert!(
            hits[0].relevance >= hits[1].relevance,
            "ranked by relevance desc"
        );
    }

    #[test]
    fn typed_axis_only_used_between_same_type() {
        // A code stash and a json stash: even though both have a typed axis,
        // their dtypes differ, so the ranker must fall back to the prose axis
        // (never compare a code SimHash to a json SimHash — that's noise).
        let mut p = Palace::empty();
        p.stashes
            .insert("code".into(), stash("code", "n", &["fn f() {}"], &["a.rs"]));
        p.stashes.insert(
            "json".into(),
            stash("json", "n", &[r#"{"k":1}"#], &["a.json"]),
        );
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_signature(&sidecar.by_stash["code"]);
        let hits = rank_similar(&p, &sidecar, &q, MatchAxis::Full, 5, Some("code"), None);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].axis_used,
            AxisUsed::FullProse,
            "cross-type must use prose axis"
        );
    }

    #[test]
    fn term_query_is_always_prose_comparable() {
        // A raw term has no type, so it compares on the prose axis against any
        // stash — including a code stash — and the distance is well-defined.
        let mut p = Palace::empty();
        p.stashes.insert(
            "auth".into(),
            stash(
                "auth",
                "auth rate limiting login",
                &["fn login() {}"],
                &["a.rs"],
            ),
        );
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_term("auth rate limiting login");
        assert!(q.typed.is_none(), "term carries no typed axis");
        let hits = rank_similar(&p, &sidecar, &q, MatchAxis::Full, 5, None, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].axis_used, AxisUsed::FullProse);
        // The term equals the note text, so prose-full distance should be small.
        assert!(
            hits[0].distance < 32,
            "distance {} should be well under half",
            hits[0].distance
        );
    }

    #[test]
    fn score_reshapes_falloff_never_empties() {
        let mut p = Palace::empty();
        for s in [
            stash("a", "alpha beta gamma delta epsilon zeta", &[], &[]),
            stash("b", "alpha beta gamma something else here", &[], &[]),
            stash("c", "totally unrelated words appear over there", &[], &[]),
        ] {
            p.stashes.insert(s.name.clone(), s);
        }
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_signature(&sidecar.by_stash["a"]);
        let wide = rank_similar(&p, &sidecar, &q, MatchAxis::Note, 1, Some("a"), None);
        let tight = rank_similar(&p, &sidecar, &q, MatchAxis::Note, 10, Some("a"), None);
        // Same set returned (never empty), same ordering — only relevance scales.
        assert_eq!(wide.len(), tight.len());
        assert_eq!(wide.len(), 2);
        // Gamma now shapes rank_weight (the sort key), not the displayed
        // relevance. A non-identical neighbor keeps more rank_weight at score 1
        // (wide) than 10 (tight); its displayed relevance is unchanged by score.
        let wide_b = wide.iter().find(|h| h.name == "b").unwrap();
        let tight_b = tight.iter().find(|h| h.name == "b").unwrap();
        assert!(wide_b.rank_weight >= tight_b.rank_weight);
        assert!(
            (wide_b.relevance - tight_b.relevance).abs() < 1e-9,
            "displayed relevance must not depend on --score"
        );
    }

    // ── Chunked similarity ───────────────────────────────────────────────

    #[test]
    fn long_body_produces_multiple_chunks() {
        // A body with many words must window into more than one chunk; a short
        // one collapses to a single chunk.
        let long: Vec<&str> = vec![
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu \
             nu xi omicron pi rho sigma tau upsilon phi chi psi omega one two three",
        ];
        let s = stash("long", "note", &long, &[]);
        let sig = signature_stash(&s);
        let chunks = sig.chunks.unwrap();
        assert!(
            chunks.prose.len() > 1,
            "long body should window into >1 chunk, got {}",
            chunks.prose.len()
        );
        assert_eq!(chunks.prose_minhash.len(), MINHASH_K);

        let short = stash("short", "note", &["only a few words here"], &[]);
        let sc = signature_stash(&short).chunks.unwrap();
        assert_eq!(sc.prose.len(), 1, "short body collapses to one chunk");
    }

    #[test]
    fn best_pair_rewards_one_shared_section() {
        // q shares its FIRST chunk's content with cand, but the rest differs.
        // best-pair should score high (a shared section), whereas a whole-stash
        // SimHash would be dragged down by the unrelated remainder.
        let shared = simhash(&project(
            "auth rate limit login throttle attempts handler",
            DataType::Prose,
        ));
        let q = vec![
            shared,
            simhash(&project(
                "totally different filler words here",
                DataType::Prose,
            )),
        ];
        let cand = vec![
            simhash(&project(
                "unrelated header preamble lines about something",
                DataType::Prose,
            )),
            shared,
            simhash(&project(
                "more unrelated trailing content at the end",
                DataType::Prose,
            )),
        ];
        let bp = best_pair_similarity(&q, &cand);
        assert!(
            bp > 0.9,
            "shared section should yield high best-pair, got {bp}"
        );

        // No shared chunk → low best-pair.
        let disjoint = vec![simhash(&project(
            "xray yankee zulu quebec foxtrot",
            DataType::Prose,
        ))];
        let bp2 = best_pair_similarity(&q, &disjoint);
        assert!(bp2 < bp, "disjoint should score lower ({bp2} < {bp})");
    }

    #[test]
    fn minhash_jaccard_high_for_near_dup_low_for_distinct() {
        let body_a = ["the quick brown fox jumps over the lazy dog again and again today"];
        let body_b = ["the quick brown fox jumps over the lazy dog again and again today"]; // identical
        let body_c =
            ["completely different sentence with no shared vocabulary whatsoever here now"];
        let a = signature_stash(&stash("a", "n", &body_a, &[]))
            .chunks
            .unwrap();
        let b = signature_stash(&stash("b", "n", &body_b, &[]))
            .chunks
            .unwrap();
        let c = signature_stash(&stash("c", "n", &body_c, &[]))
            .chunks
            .unwrap();
        let dup = minhash_jaccard(&a.prose_minhash, &b.prose_minhash);
        let diff = minhash_jaccard(&a.prose_minhash, &c.prose_minhash);
        assert!(dup > 0.9, "near-dup jaccard should be high, got {dup}");
        assert!(
            diff < dup,
            "distinct jaccard {diff} should be below dup {dup}"
        );
    }

    #[test]
    fn chunked_path_ranks_partial_overlap_above_unrelated() {
        // auth-impl and auth-test share a function-name section; pizza shares
        // nothing. With chunking, the shared section should rank auth-test
        // first even though each stash also has unrelated lines.
        let mut p = Palace::empty();
        p.stashes.insert(
            "auth-impl".into(),
            stash(
                "auth-impl",
                "auth login implementation",
                &[
                    "fn check_auth_rate_limit() { throttle_login_attempts(); }",
                    "fn unrelated_helper_one() { do_something_else_entirely(); }",
                ],
                &["auth.rs"],
            ),
        );
        p.stashes.insert(
            "auth-test".into(),
            stash(
                "auth-test",
                "auth login tests",
                &[
                    "fn test_misc() { assert_unrelated_thing(); }",
                    "fn check_auth_rate_limit() { throttle_login_attempts(); }",
                ],
                &["auth_test.rs"],
            ),
        );
        p.stashes.insert(
            "pizza".into(),
            stash(
                "pizza",
                "pizza recipes",
                &["best pizza dessert toppings cheese and favourite food recipes here"],
                &["food.md"],
            ),
        );
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_signature(&sidecar.by_stash["auth-impl"]);
        let hits = rank_similar(
            &p,
            &sidecar,
            &q,
            MatchAxis::Full,
            5,
            Some("auth-impl"),
            None,
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].name, "auth-test",
            "shared code section should rank first"
        );
        assert_eq!(
            hits[0].method,
            SimMethod::Chunked,
            "full axis with chunks uses the chunked path"
        );
        assert!(hits[0].best_pair.unwrap() > hits[1].best_pair.unwrap());
    }

    #[test]
    fn suggest_links_returns_related_above_threshold_excluding_linked() {
        let mut p = Palace::empty();
        p.stashes.insert(
            "auth-impl".into(),
            stash(
                "auth-impl",
                "auth login rate limit throttle implementation",
                &["fn check_auth_rate_limit() { throttle_login_attempts(); }"],
                &["auth.rs"],
            ),
        );
        p.stashes.insert(
            "auth-test".into(),
            stash(
                "auth-test",
                "auth login rate limit throttle tests",
                &["fn check_auth_rate_limit() { throttle_login_attempts(); }"],
                &["auth_test.rs"],
            ),
        );
        p.stashes.insert(
            "pizza".into(),
            stash(
                "pizza",
                "pizza dessert food recipes toppings cheese",
                &[],
                &[],
            ),
        );
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());

        // Low threshold: auth-test should be suggested for auth-impl; pizza not.
        let sugg = suggest_links(&p, &sidecar, "auth-impl", 0.4, 5);
        assert!(
            sugg.iter().any(|s| s.name == "auth-test"),
            "related stash suggested"
        );
        assert!(
            !sugg.iter().any(|s| s.name == "pizza"),
            "unrelated stash not suggested"
        );

        // A very high threshold suppresses everything.
        let none = suggest_links(&p, &sidecar, "auth-impl", 0.99, 5);
        assert!(none.is_empty(), "nothing clears a 99% bar here");

        // Already-linked stashes are not re-suggested.
        struct C;
        impl crate::palace::ops::Clock for C {
            fn now_iso(&self) -> String {
                "t".into()
            }
            fn now_ms(&self) -> i64 {
                1
            }
        }
        crate::palace::relations::add_relation(
            &mut p,
            &C,
            "auth-impl",
            "auth-test",
            "see-also",
            "",
        )
        .unwrap();
        let after = suggest_links(&p, &sidecar, "auth-impl", 0.4, 5);
        assert!(
            !after.iter().any(|s| s.name == "auth-test"),
            "linked stash excluded"
        );
    }

    #[test]
    fn older_sidecar_without_chunks_or_vector_is_upgraded() {
        // Simulate a sidecar written before chunking/vectors: scalar only.
        let mut p = Palace::empty();
        p.stashes.insert(
            "s".into(),
            stash("s", "hello world note", &["body line here"], &[]),
        );
        let scalar_only = FingerprintSidecar {
            by_stash: [(
                "s".to_string(),
                StashSignature {
                    scalar: fingerprint_stash(&p.stashes["s"]),
                    chunks: None,
                    vector: None,
                },
            )]
            .into_iter()
            .collect(),
        };
        let (upgraded, changed) = reconcile(&p, &scalar_only);
        assert!(changed, "missing chunks/vector should force a recompute");
        assert!(upgraded.by_stash["s"].chunks.is_some(), "chunks backfilled");
        assert!(upgraded.by_stash["s"].vector.is_some(), "vector backfilled");
    }

    // ── Random-projection vector ─────────────────────────────────────────

    #[test]
    fn randproj_is_deterministic_and_unit_norm() {
        let a = randproj_vector("auth rate limit login throttle", DataType::Prose);
        let b = randproj_vector("auth rate limit login throttle", DataType::Prose);
        assert_eq!(a, b, "same text → same vector");
        assert_eq!(a.len(), RANDPROJ_DIM);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "L2-normalized, norm={norm}");
    }

    #[test]
    fn cosine_related_above_unrelated() {
        let base = randproj_vector("auth rate limit login throttle attempts", DataType::Prose);
        let near = randproj_vector("auth login rate limit throttle requests", DataType::Prose);
        let far = randproj_vector(
            "pizza dessert food recipes toppings cheese",
            DataType::Prose,
        );
        let c_near = cosine(&base, &near);
        let c_far = cosine(&base, &far);
        assert!(
            c_near > c_far,
            "related cosine {c_near} > unrelated {c_far}"
        );
        assert!(
            c_near > 0.3,
            "shared vocab should give real cosine, got {c_near}"
        );
    }

    #[test]
    fn cosine_does_not_bridge_semantic_gap() {
        // The honest limit: no shared tokens → near-orthogonal, regardless of
        // meaning. "dog Rex" vs "my pet's name" share nothing lexical.
        let a = randproj_vector("dog rex", DataType::Prose);
        let b = randproj_vector("my pet's name", DataType::Prose);
        let c = cosine(&a, &b);
        assert!(c < 0.2, "hashing-trick cannot bridge meaning; cosine={c}");
    }

    #[test]
    fn vector_axis_ranks_related_first() {
        let mut p = Palace::empty();
        for s in [
            stash(
                "auth",
                "auth rate limit login throttle attempts handler",
                &[],
                &[],
            ),
            stash(
                "auth2",
                "login auth throttle rate limit requests guard",
                &[],
                &[],
            ),
            stash(
                "pizza",
                "pizza dessert food recipes toppings cheese dough",
                &[],
                &[],
            ),
        ] {
            p.stashes.insert(s.name.clone(), s);
        }
        let (sidecar, _) = reconcile(&p, &FingerprintSidecar::default());
        let q = SimQuery::from_signature(&sidecar.by_stash["auth"]);
        let hits = rank_similar(&p, &sidecar, &q, MatchAxis::Vector, 5, Some("auth"), None);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].name, "auth2",
            "lexically-related ranks first on vector axis"
        );
        assert_eq!(hits[0].method, SimMethod::RandProj);
    }
}
