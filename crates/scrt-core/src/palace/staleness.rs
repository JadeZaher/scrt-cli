//! Staleness detection — port of `computeNodeStaleness` in v0.x
//! `src/mind-palace.ts`. Computed at fetch time (not stored). Rules, in
//! priority order, match v0.x exactly (see COMPAT.md §4.3).

use super::ops::hash_line;
use super::types::{StaleReason, StaleState, StashedNode};

/// Compute the current staleness state of a stashed node.
pub fn compute_node_staleness(node: &StashedNode) -> StaleState {
    // 1. Non-file sources are never stale.
    let Some(file_path) = &node.file_path else {
        return StaleState::Fresh;
    };

    // 2. Legacy stash (no capture metadata) -> unknown.
    if node.source_mtime_ms.is_none() && node.match_line_hash.is_none() {
        return StaleState::Unknown;
    }

    // 3. File missing now.
    if !std::path::Path::new(file_path).exists() {
        return StaleState::Stale(StaleReason::FileMissing);
    }

    // 4. mtime check.
    if let Some(stored_mtime) = node.source_mtime_ms {
        let current_mtime = match std::fs::metadata(file_path).and_then(|m| m.modified()) {
            Ok(t) => t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0),
            Err(_) => return StaleState::Stale(StaleReason::FileMissing),
        };
        if current_mtime > stored_mtime {
            // mtime advanced — check content drift at the match line.
            if let Some(stored_hash) = &node.match_line_hash {
                let current_hash = match std::fs::read_to_string(file_path) {
                    Ok(content) => {
                        // v0.x splits on /\r?\n/; match_line is 1-indexed.
                        let lines: Vec<&str> = split_crlf(&content);
                        let line_text = lines
                            .get((node.match_line as usize).wrapping_sub(1))
                            .copied()
                            .unwrap_or("");
                        hash_line(line_text)
                    }
                    Err(_) => return StaleState::Stale(StaleReason::ContentDrifted),
                };
                return match current_hash {
                    Some(h) if &h == stored_hash => {
                        StaleState::Stale(StaleReason::MtimeAdvancedContentIntact)
                    }
                    _ => StaleState::Stale(StaleReason::ContentDrifted),
                };
            }
            return StaleState::Stale(StaleReason::MtimeAdvanced);
        }
    }

    StaleState::Fresh
}

/// Split on `\r?\n` (mirrors v0.x `content.split(/\r?\n/)`).
fn split_crlf(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for part in content.split('\n') {
        out.push(part.strip_suffix('\r').unwrap_or(part));
    }
    out
}
