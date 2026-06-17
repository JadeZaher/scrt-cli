//! Behavioral tests for palace prune / relationships / multi-tenant
//! registry — the Prompt-4 operation set, exercised end-to-end through the
//! `MemoryPalace` and `Registry` (the in-process / multi-tenant shapes).

use scrt_core::palace::ops::{add_stash, Clock, StashOptions};
use scrt_core::palace::prune::{prune_expired, prune_keep, prune_tag};
use scrt_core::palace::relations::{add_relation, get_related, traversal_graph, Direction};
use scrt_core::palace::types::{Palace as PalaceData, StashSearch};
use scrt_core::palace::{MemoryPalace, Palace, Registry};
use scrt_core::types::Source;

/// Deterministic clock.
struct FixedClock(i64);
impl Clock for FixedClock {
    fn now_iso(&self) -> String {
        scrt_core::palace::ops::iso_from_ms(self.0)
    }
    fn now_ms(&self) -> i64 {
        self.0
    }
}

fn node(id: &str, line: u64) -> scrt_core::types::Node {
    scrt_core::types::Node {
        id: 1,
        source: Source::file(id),
        match_line: line,
        start_line: line,
        end_line: line,
        context_before: vec![],
        match_text: format!("m{line}"),
        context_after: vec![],
        match_spans: vec![[0, 1]],
        tokens: 1,
    }
}

fn meta() -> StashSearch {
    StashSearch {
        pattern: "p".into(),
        effort: "normal".into(),
        sources_count: 1,
    }
}

fn seed(p: &mut PalaceData, clock: &dyn Clock, name: &str, tags: &[String], ttl: Option<&str>) {
    let opts = StashOptions {
        ttl: ttl.map(String::from),
        ..Default::default()
    };
    add_stash(
        p,
        clock,
        name,
        "note",
        &[node("f.txt", 1)],
        meta(),
        &[],
        tags,
        &opts,
    )
    .unwrap();
}

#[test]
fn prune_tag_removes_only_tagged() {
    let clock = FixedClock(1_000_000);
    let mut p = PalaceData::empty();
    seed(&mut p, &clock, "a", &["keep".into()], None);
    seed(&mut p, &clock, "b", &["temp".into()], None);
    seed(&mut p, &clock, "c", &["temp".into()], None);
    let r = prune_tag(&mut p, "temp", false);
    assert_eq!(r.removed, 2);
    assert_eq!(
        p.stashes.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["a"]
    );
}

#[test]
fn prune_expired_uses_ttl() {
    // Seed at t=0 with a 1s TTL, prune at t=2s.
    let mut p = PalaceData::empty();
    seed(&mut p, &FixedClock(0), "old", &[], Some("1s"));
    seed(&mut p, &FixedClock(0), "permanent", &[], None);
    let r = prune_expired(&mut p, &FixedClock(2000), false);
    assert_eq!(r.removed, 1);
    assert_eq!(r.names, vec!["old"]);
    assert!(p.stashes.contains_key("permanent"));
}

#[test]
fn prune_expired_dry_run_keeps_data() {
    let mut p = PalaceData::empty();
    seed(&mut p, &FixedClock(0), "old", &[], Some("1s"));
    let r = prune_expired(&mut p, &FixedClock(2000), true);
    assert_eq!(r.removed, 1);
    assert!(r.dry_run);
    assert!(p.stashes.contains_key("old")); // not actually removed
}

#[test]
fn prune_keep_n_most_recent() {
    let mut p = PalaceData::empty();
    // Distinct updated_at via distinct clocks.
    seed(&mut p, &FixedClock(1000), "oldest", &[], None);
    seed(&mut p, &FixedClock(2000), "mid", &[], None);
    seed(&mut p, &FixedClock(3000), "newest", &[], None);
    let r = prune_keep(&mut p, 2, false);
    assert_eq!(r.removed, 1);
    assert_eq!(r.names, vec!["oldest"]);
}

#[test]
fn relations_and_graph_traversal() {
    let clock = FixedClock(1);
    let mut p = PalaceData::empty();
    for n in ["a", "b", "c"] {
        seed(&mut p, &clock, n, &[], None);
    }
    add_relation(&mut p, &clock, "a", "b", "depends-on", "x").unwrap();
    add_relation(&mut p, &clock, "b", "c", "see-also", "").unwrap();

    // get_related on b: outbound to c, inbound from a.
    let rel = get_related(&p, "b");
    assert_eq!(rel.len(), 2);
    assert!(rel
        .iter()
        .any(|r| r.direction == Direction::Outbound && r.stash_name == "c"));
    assert!(rel
        .iter()
        .any(|r| r.direction == Direction::Inbound && r.stash_name == "a"));

    // graph from a, depth 3: reaches b (depth 1) and c (depth 2).
    let g = traversal_graph(&p, "a", 3);
    let names: Vec<&str> = g.iter().map(|n| n.stash_name.as_str()).collect();
    assert!(names.contains(&"b"));
    assert!(names.contains(&"c"));
    let c_depth = g.iter().find(|n| n.stash_name == "c").unwrap().depth;
    assert_eq!(c_depth, 2);

    // self-link rejected.
    assert!(add_relation(&mut p, &clock, "a", "a", "x", "").is_err());
}

#[test]
fn registry_multi_tenant_isolation() {
    let clock = FixedClock(42);
    let mut reg = Registry::new();
    reg.open_memory("tenant-a");
    reg.open_memory("tenant-b");
    assert_eq!(reg.len(), 2);

    // Stash into A only; B stays empty — tenants are isolated.
    {
        let a = reg.get_mut("tenant-a").unwrap();
        seed(a.data_mut(), &clock, "only-in-a", &[], None);
        a.save().unwrap(); // memory save is a no-op
    }
    assert_eq!(reg.get("tenant-a").unwrap().data().stashes.len(), 1);
    assert_eq!(reg.get("tenant-b").unwrap().data().stashes.len(), 0);

    assert!(reg.close("tenant-a"));
    assert_eq!(reg.len(), 1);
}

#[test]
fn memory_palace_holds_state_without_disk() {
    let clock = FixedClock(7);
    let mut mp = MemoryPalace::new();
    seed(mp.data_mut(), &clock, "s", &[], None);
    mp.save().unwrap();
    assert_eq!(mp.data().stashes.len(), 1);
    assert!(!mp.is_tainted());
}
