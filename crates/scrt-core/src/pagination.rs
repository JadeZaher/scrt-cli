//! Pagination — port of v0.x `src/pagination.ts`.
//!
//! Opt-in via `--page N`. Default is "give me everything". `PaginationMeta`
//! serializes exactly as COMPAT.md §2 (key order: page, page_size,
//! total_items, total_pages, has_next, has_prev).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default)]
pub struct PaginationOptions {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationMeta {
    pub page: usize,
    pub page_size: usize,
    pub total_items: usize,
    pub total_pages: usize,
    pub has_next: bool,
    pub has_prev: bool,
}

struct Resolved {
    enabled: bool,
    page: usize,
    page_size: usize,
}

/// Mirror `resolvePagination`: enabled iff not `--all` and `page > 0`.
/// Default page size is 10 (the v0.x default; callers pass 20 for stash
/// lists). `page` defaults to 1, `page_size` to 10.
fn resolve(opts: PaginationOptions) -> Resolved {
    let enabled = !opts.all && matches!(opts.page, Some(p) if p > 0);
    let page = if enabled { opts.page.unwrap_or(1) } else { 1 };
    let page_size = opts.page_size.unwrap_or(10);
    Resolved {
        enabled,
        page,
        page_size,
    }
}

/// Apply pagination to a vector, returning the slice and metadata.
/// Mirrors `paginate<T>` including the page clamp to `[1, total_pages]`.
pub fn paginate<T>(items: Vec<T>, opts: PaginationOptions) -> (Vec<T>, Option<PaginationMeta>) {
    let r = resolve(opts);
    if !r.enabled {
        return (items, None);
    }
    let total_items = items.len();
    let total_pages = std::cmp::max(1, total_items.div_ceil(r.page_size));
    let clamped = r.page.clamp(1, total_pages);
    let start = (clamped - 1) * r.page_size;
    let end = std::cmp::min(start + r.page_size, total_items);
    let slice: Vec<T> = items
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    let meta = PaginationMeta {
        page: clamped,
        page_size: r.page_size,
        total_items,
        total_pages,
        has_next: clamped < total_pages,
        has_prev: clamped > 1,
    };
    (slice, Some(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_returns_all_no_meta() {
        let (items, meta) = paginate(vec![1, 2, 3], PaginationOptions::default());
        assert_eq!(items, vec![1, 2, 3]);
        assert!(meta.is_none());
    }

    #[test]
    fn first_page_of_two() {
        let opts = PaginationOptions {
            page: Some(1),
            page_size: Some(2),
            all: false,
        };
        let (items, meta) = paginate(vec![1, 2, 3], opts);
        assert_eq!(items, vec![1, 2]);
        let m = meta.unwrap();
        assert_eq!(m.total_pages, 2);
        assert!(m.has_next);
        assert!(!m.has_prev);
    }

    #[test]
    fn clamps_overshoot_page() {
        let opts = PaginationOptions {
            page: Some(99),
            page_size: Some(2),
            all: false,
        };
        let (items, meta) = paginate(vec![1, 2, 3], opts);
        assert_eq!(items, vec![3]); // last page
        let m = meta.unwrap();
        assert_eq!(m.page, 2);
        assert!(m.has_prev);
        assert!(!m.has_next);
    }

    #[test]
    fn all_flag_disables() {
        let opts = PaginationOptions {
            page: Some(1),
            page_size: Some(1),
            all: true,
        };
        let (items, meta) = paginate(vec![1, 2, 3], opts);
        assert_eq!(items, vec![1, 2, 3]);
        assert!(meta.is_none());
    }
}
