//! Mind palace — instantiable short-term memory.
//!
//! Two backends behind one [`Palace`] trait:
//!   - [`FilePalace`]: on-disk JSON, byte-compatible with v0.x. Atomic
//!     write + snapshot-diff merge so concurrent writers don't clobber.
//!   - [`MemoryPalace`]: in-process, no disk I/O — ephemeral per-task
//!     palaces inside a long-running engine (the new shape this port adds).
//!
//! Both wrap the same pure [`ops`]/[`prune`]/[`relations`] logic over a
//! [`types::Palace`] value, so semantics are identical; only persistence
//! differs. A [`Registry`] holds N named palaces for multi-tenant routing.

pub mod ops;
pub mod prune;
pub mod relations;
pub mod simhash;
pub mod staleness;
pub mod types;

use std::path::{Path, PathBuf};

use ops::{Clock, SystemClock};
use types::{Palace as PalaceData, PALACE_VERSION};

/// Errors surfaced by palace backends.
#[derive(Debug)]
pub enum PalaceError {
    /// Backing file was unreadable/corrupt; saves refuse to clobber it
    /// (tainted), matching v0.x. Inspect the `.corrupt.*` backup.
    Tainted(String),
    Io(String),
    /// An operation-level error (unknown stash, bad duration, etc.).
    Op(String),
}

impl std::fmt::Display for PalaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PalaceError::Tainted(m) => write!(f, "{m}"),
            PalaceError::Io(m) => write!(f, "{m}"),
            PalaceError::Op(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for PalaceError {}

/// A backend that holds palace data and can persist changes. Operations
/// run via the free functions in [`ops`]/[`prune`]/[`relations`] against
/// the data returned by [`Palace::data_mut`]; backends differ only in
/// [`Palace::load`] / [`Palace::save`].
pub trait Palace {
    /// Immutable view of the underlying data.
    fn data(&self) -> &PalaceData;
    /// Mutable view of the underlying data.
    fn data_mut(&mut self) -> &mut PalaceData;
    /// Persist the current state. `FilePalace` writes to disk (with the
    /// snapshot-diff merge); `MemoryPalace` is a no-op.
    fn save(&mut self) -> Result<(), PalaceError>;
    /// True if the backend is tainted (corrupt on-disk file); saves refuse
    /// to overwrite unless force-reset. Always false for memory.
    fn is_tainted(&self) -> bool {
        false
    }
}

/// On-disk palace, byte-compatible with v0.x `mind-palace.json`.
pub struct FilePalace {
    path: PathBuf,
    data: PalaceData,
    /// Snapshot of the data at load time, used to compute this process's
    /// diff at save time (the v0.x snapshot-diff merge).
    snapshot: PalaceData,
    tainted: bool,
    force_reset: bool,
}

impl FilePalace {
    /// Load (or create-empty) the palace at `path`. On a corrupt file, copy
    /// it aside to `<path>.corrupt.<ms>`, mark tainted, and return an empty
    /// palace so reads still work (v0.x `loadPalace`).
    pub fn load(path: impl AsRef<Path>, clock: &dyn Clock) -> Self {
        let path = path.as_ref().to_path_buf();
        let force_reset = std::env::var("MPG_FORCE_RESET").is_ok();
        if !path.exists() {
            return FilePalace::fresh(path, force_reset);
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => {
                // Unreadable -> tainted empty.
                return FilePalace {
                    path,
                    data: PalaceData::empty(),
                    snapshot: PalaceData::empty(),
                    tainted: true,
                    force_reset,
                };
            }
        };
        if raw.trim().is_empty() {
            return FilePalace::fresh(path, force_reset);
        }
        match serde_json::from_str::<PalaceData>(&raw) {
            Ok(mut parsed) => {
                if parsed.version == 0 {
                    parsed.version = PALACE_VERSION;
                }
                let snapshot = parsed.clone();
                FilePalace { path, data: parsed, snapshot, tainted: false, force_reset }
            }
            Err(_) => {
                // Corrupt: preserve a forensic backup, mark tainted.
                let backup = format!("{}.corrupt.{}", path.display(), clock.now_ms());
                let _ = std::fs::write(&backup, &raw);
                FilePalace {
                    path,
                    data: PalaceData::empty(),
                    snapshot: PalaceData::empty(),
                    tainted: true,
                    force_reset,
                }
            }
        }
    }

    fn fresh(path: PathBuf, force_reset: bool) -> Self {
        FilePalace {
            path,
            data: PalaceData::empty(),
            snapshot: PalaceData::empty(),
            tainted: false,
            force_reset,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Palace for FilePalace {
    fn data(&self) -> &PalaceData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut PalaceData {
        &mut self.data
    }
    fn is_tainted(&self) -> bool {
        self.tainted
    }

    fn save(&mut self) -> Result<(), PalaceError> {
        if self.tainted && !self.force_reset {
            return Err(PalaceError::Tainted(format!(
                "scrt: refusing to save over a tainted palace at {}. The on-disk file was \
                 unreadable or corrupt; inspect the *.corrupt.* backup, then fix it or set \
                 MPG_FORCE_RESET=1 to overwrite.",
                self.path.display()
            )));
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| PalaceError::Io(e.to_string()))?;
            }
        }

        // Snapshot-diff merge (v0.x savePalace): compute what THIS process
        // removed/touched vs the load-time snapshot, re-read the freshest
        // on-disk state, and replay our diff on top — correct under
        // concurrent writers (a stash we dropped stays dropped; one we
        // added/modified wins; untouched stashes are preserved).
        let removed_by_us: Vec<String> = self
            .snapshot
            .stashes
            .keys()
            .filter(|k| !self.data.stashes.contains_key(*k))
            .cloned()
            .collect();
        let touched_by_us: Vec<String> = self
            .data
            .stashes
            .iter()
            .filter(|(name, stash)| match self.snapshot.stashes.get(*name) {
                None => true, // added
                Some(before) => {
                    // Modified iff serialized form changed.
                    serde_json::to_string(before).ok() != serde_json::to_string(stash).ok()
                }
            })
            .map(|(name, _)| name.clone())
            .collect();

        let mut merged = if self.path.exists() {
            match std::fs::read_to_string(&self.path) {
                Ok(raw) if raw.trim().is_empty() => {
                    PalaceData { version: self.data.version, stashes: Default::default() }
                }
                Ok(raw) => match serde_json::from_str::<PalaceData>(&raw) {
                    Ok(on_disk) => on_disk,
                    Err(_) => PalaceData {
                        version: self.data.version,
                        stashes: self.data.stashes.clone(),
                    },
                },
                Err(_) => PalaceData {
                    version: self.data.version,
                    stashes: self.data.stashes.clone(),
                },
            }
        } else {
            PalaceData { version: self.data.version, stashes: Default::default() }
        };

        for name in &removed_by_us {
            merged.stashes.shift_remove(name);
        }
        for name in &touched_by_us {
            if let Some(s) = self.data.stashes.get(name) {
                merged.stashes.insert(name.clone(), s.clone());
            }
        }

        // Persist merge back into memory so the caller keeps a coherent view.
        self.data.stashes = merged.stashes;
        self.data.version = merged.version;
        self.snapshot = self.data.clone();

        // Atomic write: tmp sibling + rename. Body is
        // `JSON.stringify(palace, null, 2) + "\n"`.
        let body = serde_json::to_string_pretty(&self.data)
            .map_err(|e| PalaceError::Io(e.to_string()))?
            + "\n";
        let tmp = self.path.with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, body).map_err(|e| PalaceError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            PalaceError::Io(e.to_string())
        })?;
        Ok(())
    }
}

/// In-process palace — no disk I/O. The new shape: ephemeral per-task
/// memory inside a long-running engine, without filesystem churn.
#[derive(Default)]
pub struct MemoryPalace {
    data: PalaceData,
}

impl MemoryPalace {
    pub fn new() -> Self {
        MemoryPalace { data: PalaceData::empty() }
    }
    /// Seed an in-memory palace from existing data (e.g. cloned from a file).
    pub fn from_data(data: PalaceData) -> Self {
        MemoryPalace { data }
    }
}

impl Palace for MemoryPalace {
    fn data(&self) -> &PalaceData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut PalaceData {
        &mut self.data
    }
    fn save(&mut self) -> Result<(), PalaceError> {
        Ok(()) // nothing to persist
    }
}

/// Resolve the default palace path (port of v0.x `defaultPalacePath`):
/// `MPG_MIND_PALACE` env override, then git root, then nearest existing
/// palace walking up, then `<cwd>/.mpg/mind-palace.json`. `MPG_MIND_PALACE`
/// and `.mpg/` are kept (not rebranded) for migration — see DESIGN.md §5.
pub fn default_palace_path() -> PathBuf {
    use types::{DEFAULT_PALACE_DIR, DEFAULT_PALACE_FILENAME};
    if let Ok(env_path) = std::env::var("MPG_MIND_PALACE") {
        return PathBuf::from(env_path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(git_root) = find_git_root(&cwd) {
        return git_root.join(DEFAULT_PALACE_DIR).join(DEFAULT_PALACE_FILENAME);
    }
    if let Some(existing) = find_existing_palace(&cwd) {
        return existing;
    }
    cwd.join(DEFAULT_PALACE_DIR).join(DEFAULT_PALACE_FILENAME)
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..32 {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => break,
        }
    }
    None
}

// ── Path-based set-composition convenience (used by the server) ──────────
//
// v0.x's server loads the palace then calls composeToSources/etc and maps
// to `.id`. These wrappers do the same: load a FilePalace at `path`, run the
// pure op, return the file-id list. They never mutate, so no save.

/// Union of stash file sets (returns file ids). Port of the server's
/// `palace.compose` body.
pub fn compose_to_sources_path(path: &Path, names: &[String]) -> Result<Vec<String>, String> {
    let palace = FilePalace::load(path, &SystemClock);
    ops::compose_to_sources(palace.data(), names)
}

/// Intersection of stash file sets (returns file ids).
pub fn intersect_to_sources_path(path: &Path, names: &[String]) -> Result<Vec<String>, String> {
    let palace = FilePalace::load(path, &SystemClock);
    ops::intersect_to_sources(palace.data(), names)
}

/// Difference: files in `base` not in any `exclude` (returns file ids).
pub fn except_to_sources_path(
    path: &Path,
    base: &str,
    exclude: &[String],
) -> Result<Vec<String>, String> {
    let palace = FilePalace::load(path, &SystemClock);
    ops::except_to_sources(palace.data(), base, exclude)
}

fn find_existing_palace(start: &Path) -> Option<PathBuf> {
    use types::{DEFAULT_PALACE_DIR, DEFAULT_PALACE_FILENAME};
    let mut dir = start.to_path_buf();
    for _ in 0..16 {
        let candidate = dir.join(DEFAULT_PALACE_DIR).join(DEFAULT_PALACE_FILENAME);
        if candidate.exists() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => break,
        }
    }
    None
}

/// Holds N named palaces concurrently, routing operations by palace ID.
/// An ID maps to either a file path ([`FilePalace`]) or an in-memory handle
/// ([`MemoryPalace`]). One engine process, many tenants.
#[derive(Default)]
pub struct Registry {
    palaces: std::collections::HashMap<String, Box<dyn Palace + Send>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry { palaces: std::collections::HashMap::new() }
    }

    /// Open (or create) a file-backed palace under `id`. Idempotent: an
    /// already-open id is returned as-is rather than reloaded.
    pub fn open_file(&mut self, id: &str, path: impl AsRef<Path>) {
        if self.palaces.contains_key(id) {
            return;
        }
        let palace = FilePalace::load(path, &SystemClock);
        self.palaces.insert(id.to_string(), Box::new(palace));
    }

    pub fn open_memory(&mut self, id: &str) {
        self.palaces
            .entry(id.to_string())
            .or_insert_with(|| Box::new(MemoryPalace::new()));
    }

    pub fn get(&self, id: &str) -> Option<&(dyn Palace + Send)> {
        self.palaces.get(id).map(|b| b.as_ref())
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut (dyn Palace + Send)> {
        self.palaces.get_mut(id).map(|b| b.as_mut() as &mut (dyn Palace + Send))
    }

    /// Drop a palace from the registry (does not delete its file).
    pub fn close(&mut self, id: &str) -> bool {
        self.palaces.remove(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.palaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.palaces.is_empty()
    }
}
