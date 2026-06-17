//! Stash relationships — port of the relationship functions in v0.x
//! `src/mind-palace.ts` (addRelation / removeRelation / getRelated /
//! traversalGraph). Edges are directed and stored on the source stash.

use std::collections::VecDeque;

use super::ops::Clock;
use super::types::{Palace, StashRelation};

/// Direction of an edge relative to the queried stash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outbound,
    Inbound,
}

/// One related stash plus the edge connecting it.
#[derive(Debug, Clone)]
pub struct Related {
    pub stash_name: String,
    pub direction: Direction,
    pub relation: StashRelation,
}

/// One node in a graph traversal.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub stash_name: String,
    pub depth: usize,
    pub direction: Direction,
    pub via: String,
    pub relation: StashRelation,
}

/// Add a directed relationship `from -> to` (port of `addRelation`).
/// Dedups by (target, type), replacing any existing edge of that pair.
pub fn add_relation(
    palace: &mut Palace,
    clock: &dyn Clock,
    from: &str,
    to: &str,
    rel_type: &str,
    note: &str,
) -> Result<StashRelation, String> {
    if !palace.stashes.contains_key(from) {
        return Err(format!("Unknown stash: {from}"));
    }
    if !palace.stashes.contains_key(to) {
        return Err(format!("Unknown stash: {to}"));
    }
    if from == to {
        return Err("Cannot link a stash to itself.".to_string());
    }
    let rel = StashRelation {
        target: to.to_string(),
        rel_type: rel_type.to_string(),
        note: note.to_string(),
        created_at: clock.now_iso(),
    };
    let source = palace.stashes.get_mut(from).unwrap();
    source
        .relations
        .retain(|r| !(r.target == to && r.rel_type == rel_type));
    source.relations.push(rel.clone());
    source.updated_at = clock.now_iso();
    Ok(rel)
}

/// Remove all relationships `from -> to` (port of `removeRelation`).
pub fn remove_relation(
    palace: &mut Palace,
    clock: &dyn Clock,
    from: &str,
    to: &str,
) -> Result<bool, String> {
    let source = palace
        .stashes
        .get_mut(from)
        .ok_or_else(|| format!("Unknown stash: {from}"))?;
    let before = source.relations.len();
    source.relations.retain(|r| r.target != to);
    if source.relations.len() < before {
        source.updated_at = clock.now_iso();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// All stashes related to `name`, outbound then inbound (port of `getRelated`).
pub fn get_related(palace: &Palace, name: &str) -> Vec<Related> {
    let mut out = Vec::new();
    let Some(center) = palace.stashes.get(name) else {
        return out;
    };
    for r in &center.relations {
        if palace.stashes.contains_key(&r.target) {
            out.push(Related {
                stash_name: r.target.clone(),
                direction: Direction::Outbound,
                relation: r.clone(),
            });
        }
    }
    for (other_name, other) in &palace.stashes {
        if other_name == name {
            continue;
        }
        for r in &other.relations {
            if r.target == name {
                out.push(Related {
                    stash_name: other_name.clone(),
                    direction: Direction::Inbound,
                    relation: r.clone(),
                });
            }
        }
    }
    out
}

/// BFS traversal from `name` up to `max_depth` (port of `traversalGraph`).
/// Seeds with both outbound and inbound depth-1 edges; subsequent levels
/// follow outbound edges only (matching v0.x).
pub fn traversal_graph(palace: &Palace, name: &str, max_depth: usize) -> Vec<GraphNode> {
    let mut out = Vec::new();
    if !palace.stashes.contains_key(name) {
        return out;
    }
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    visited.insert(name.to_string());
    let mut queue: VecDeque<GraphNode> = VecDeque::new();

    for r in &palace.stashes[name].relations {
        if palace.stashes.contains_key(&r.target) {
            queue.push_back(GraphNode {
                stash_name: r.target.clone(),
                depth: 1,
                direction: Direction::Outbound,
                via: name.to_string(),
                relation: r.clone(),
            });
        }
    }
    for (other_name, other) in &palace.stashes {
        if other_name == name {
            continue;
        }
        for r in &other.relations {
            if r.target == name {
                queue.push_back(GraphNode {
                    stash_name: other_name.clone(),
                    depth: 1,
                    direction: Direction::Inbound,
                    via: name.to_string(),
                    relation: r.clone(),
                });
            }
        }
    }

    while let Some(item) = queue.pop_front() {
        if visited.contains(&item.stash_name) {
            continue;
        }
        visited.insert(item.stash_name.clone());
        let Some(stash) = palace.stashes.get(&item.stash_name) else {
            continue;
        };
        let depth = item.depth;
        let target = item.stash_name.clone();
        out.push(item);
        if depth >= max_depth {
            continue;
        }
        for r in &stash.relations {
            if !visited.contains(&r.target) {
                queue.push_back(GraphNode {
                    stash_name: r.target.clone(),
                    depth: depth + 1,
                    direction: Direction::Outbound,
                    via: target.clone(),
                    relation: r.clone(),
                });
            }
        }
    }
    out
}
