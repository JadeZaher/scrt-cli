//! Pruning operations — port of the `prune*` functions in v0.x
//! `src/mind-palace.ts`. All take a `dry_run` flag and return what was
//! (or would be) removed.

use super::ops::{parse_duration, Clock};
use super::types::Palace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneResult {
    pub removed: usize,
    pub names: Vec<String>,
    pub dry_run: bool,
}

/// Compare two ISO-8601 timestamps by their epoch-ms. v0.x uses
/// `new Date(iso).getTime()`; we parse the fixed `...Z` shape we emit.
fn iso_to_ms(iso: &str) -> i64 {
    parse_iso_ms(iso).unwrap_or(0)
}

/// Parse the exact ISO shape `YYYY-MM-DDTHH:MM:SS.mmmZ` (what we and v0.x
/// emit) back to epoch-ms. Returns None on a shape we didn't write.
pub fn parse_iso_ms(iso: &str) -> Option<i64> {
    // Lenient: accept optional fractional seconds and trailing Z.
    let s = iso.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut dparts = date.split('-');
    let year: i64 = dparts.next()?.parse().ok()?;
    let month: i64 = dparts.next()?.parse().ok()?;
    let day: i64 = dparts.next()?.parse().ok()?;
    let mut tparts = time.split(':');
    let hour: i64 = tparts.next()?.parse().ok()?;
    let minute: i64 = tparts.next()?.parse().ok()?;
    let sec_str = tparts.next()?;
    let (sec, millis) = match sec_str.split_once('.') {
        Some((s, frac)) => {
            let sec: i64 = s.parse().ok()?;
            // Take up to 3 fractional digits, right-pad to ms.
            let mut f = frac.to_string();
            f.truncate(3);
            while f.len() < 3 {
                f.push('0');
            }
            (sec, f.parse::<i64>().ok()?)
        }
        None => (sec_str.parse().ok()?, 0),
    };
    // days_from_civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + hour * 3600 + minute * 60 + sec) * 1000) + millis)
}

/// Remove stashes whose `updated_at` is older than `duration`.
pub fn prune_older_than(
    palace: &mut Palace,
    clock: &dyn Clock,
    duration: &str,
    dry_run: bool,
) -> Result<PruneResult, String> {
    let cutoff = clock.now_ms() - parse_duration(duration)?;
    let names: Vec<String> = palace
        .stashes
        .iter()
        .filter(|(_, s)| iso_to_ms(&s.updated_at) < cutoff)
        .map(|(n, _)| n.clone())
        .collect();
    if !dry_run {
        for n in &names {
            palace.stashes.shift_remove(n);
        }
    }
    Ok(PruneResult {
        removed: names.len(),
        names,
        dry_run,
    })
}

/// Remove stashes whose `expires_at` is in the past.
pub fn prune_expired(palace: &mut Palace, clock: &dyn Clock, dry_run: bool) -> PruneResult {
    let now = clock.now_ms();
    let names: Vec<String> = palace
        .stashes
        .iter()
        .filter(|(_, s)| {
            s.expires_at
                .as_ref()
                .map(|e| iso_to_ms(e) < now)
                .unwrap_or(false)
        })
        .map(|(n, _)| n.clone())
        .collect();
    if !dry_run {
        for n in &names {
            palace.stashes.shift_remove(n);
        }
    }
    PruneResult {
        removed: names.len(),
        names,
        dry_run,
    }
}

/// Keep the N most recently updated stashes; remove the rest.
pub fn prune_keep(palace: &mut Palace, n: usize, dry_run: bool) -> PruneResult {
    // Sort by updated_at descending (v0.x `b.updated_at.localeCompare`).
    let mut sorted: Vec<&str> = palace.stashes.keys().map(String::as_str).collect();
    sorted.sort_by(|a, b| {
        let ua = &palace.stashes[*a].updated_at;
        let ub = &palace.stashes[*b].updated_at;
        ub.cmp(ua) // descending
    });
    let names: Vec<String> = sorted.into_iter().skip(n).map(String::from).collect();
    if !dry_run {
        for nm in &names {
            palace.stashes.shift_remove(nm);
        }
    }
    PruneResult {
        removed: names.len(),
        names,
        dry_run,
    }
}

/// Remove all stashes carrying `tag`.
pub fn prune_tag(palace: &mut Palace, tag: &str, dry_run: bool) -> PruneResult {
    let names: Vec<String> = palace
        .stashes
        .iter()
        .filter(|(_, s)| s.tags.iter().any(|t| t == tag))
        .map(|(n, _)| n.clone())
        .collect();
    if !dry_run {
        for n in &names {
            palace.stashes.shift_remove(n);
        }
    }
    PruneResult {
        removed: names.len(),
        names,
        dry_run,
    }
}

/// Remove all stashes. Requires explicit confirmation.
pub fn prune_all(
    palace: &mut Palace,
    confirmed: bool,
    dry_run: bool,
) -> Result<PruneResult, String> {
    let names: Vec<String> = palace.stashes.keys().cloned().collect();
    if !confirmed {
        return Err(format!(
            "This would remove {} stashes. Pass --mp-prune-confirm to actually delete them. \
             Use --mp-prune-dry-run to see what would be removed.",
            names.len()
        ));
    }
    if !dry_run {
        palace.stashes.clear();
    }
    Ok(PruneResult {
        removed: names.len(),
        names,
        dry_run,
    })
}
