//! Transport-agnostic dispatcher. One function maps a `(method, params)` pair
//! to a JSON result or a structured `ServerError`. The stdio, HTTP, and NAPI
//! transports all funnel through here so behavior is identical across them.
//!
//! Method set + param names + error codes match v0.x exactly. Like v0.x,
//! palace methods load/save the palace per call (the warm win is process
//! startup, not palace caching).

use serde_json::{json, Value};

use scrt_core::envelope::{build_agent_envelope, format_agent_json, EnvelopeOpts};
use scrt_core::orchestrator::{effort_preset, search_with_meta, SearchConfig};
use scrt_core::palace::ops::{
    add_stash, get_stash, list_stashes, StashOptions, StashSearch, SystemClock,
};
use scrt_core::palace::prune::{prune_expired, prune_keep, prune_older_than, prune_tag};
use scrt_core::palace::relations::{add_relation, traversal_graph, Direction};
use scrt_core::palace::{compose_to_sources_path, default_palace_path, except_to_sources_path, intersect_to_sources_path};
use scrt_core::palace::{FilePalace, Palace};
use scrt_core::types::{Effort, SearchOptions, SortMode, Strategy, WindowCurve};
use scrt_core::SourceInput;

/// Server-version string reported by `health`. Matches the targeted Node
/// reference line (DESIGN.md notes the package.json/index.ts drift).
pub const SERVER_VERSION: &str = "0.1.0";

/// Error codes, byte-identical to v0.x `ErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadParams,
    Internal,
    UnknownMethod,
    NotImplemented,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::BadParams => "BAD_PARAMS",
            ErrorCode::Internal => "INTERNAL",
            ErrorCode::UnknownMethod => "UNKNOWN_METHOD",
            ErrorCode::NotImplemented => "NOT_IMPLEMENTED",
        }
    }
}

/// A dispatch error: `{ code, message }` on the wire.
#[derive(Debug, Clone)]
pub struct ServerError {
    pub code: ErrorCode,
    pub message: String,
}

impl ServerError {
    fn bad(msg: impl Into<String>) -> Self {
        ServerError { code: ErrorCode::BadParams, message: msg.into() }
    }
    fn internal(msg: impl Into<String>) -> Self {
        ServerError { code: ErrorCode::Internal, message: msg.into() }
    }
    /// JSON form: `{ "code": "...", "message": "..." }`.
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code.as_str(), "message": self.message })
    }
}

// ── Param coercion helpers ────────────────────────────────────────────────

fn as_str(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::to_string)
}
fn as_num(params: &Value, key: &str) -> Option<f64> {
    params.get(key).and_then(Value::as_f64)
}
fn as_usize(params: &Value, key: &str) -> Option<usize> {
    as_num(params, key).map(|n| n as usize)
}
fn as_bool(params: &Value, key: &str) -> bool {
    params.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn as_str_array(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}
fn as_str_array_either(params: &Value, a: &str, b: &str) -> Vec<String> {
    let first = as_str_array(params, a);
    if !first.is_empty() {
        first
    } else {
        as_str_array(params, b)
    }
}

fn parse_effort(s: Option<&str>) -> Effort {
    match s {
        Some("scan") => Effort::Scan,
        Some("normal") => Effort::Normal,
        Some("deep") => Effort::Deep,
        Some("auto") => Effort::Auto,
        _ => Effort::Quick,
    }
}

/// Resolve the palace path param (`palace_path` ?? default).
fn palace_path(params: &Value) -> std::path::PathBuf {
    as_str(params, "palace_path")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_palace_path)
}

/// Dispatch a single request. Returns the JSON result or a `ServerError`.
pub fn dispatch(method: &str, params: &Value) -> Result<Value, ServerError> {
    match method {
        // ── Core search ─────────────────────────────────────────────────
        "search" => {
            let pattern = as_str(params, "pattern")
                .ok_or_else(|| ServerError::bad("search: params.pattern (string) is required"))?;
            let effort = parse_effort(params.get("effort").and_then(Value::as_str));
            let (pb, pa, pn) = effort_preset(effort);

            // Build the input list: in[] paths, cmd, url, stdin.
            let mut inputs: Vec<SourceInput> = as_str_array(params, "in")
                .into_iter()
                .map(SourceInput::Path)
                .collect();
            if let Some(cmd) = as_str(params, "cmd") {
                inputs.push(SourceInput::Command(cmd));
            }
            if let Some(url) = as_str(params, "url") {
                inputs.push(SourceInput::Url(url));
            }
            if as_bool(params, "stdin") {
                inputs.push(SourceInput::Stdin);
            }

            let opts = SearchOptions::default();
            let config = SearchConfig {
                pattern,
                inputs,
                effort,
                strategy: match params.get("strategy").and_then(Value::as_str) {
                    Some("deep") => Strategy::Deep,
                    _ => Strategy::Fill,
                },
                before_tokens: as_usize(params, "before").unwrap_or(pb),
                after_tokens: as_usize(params, "after").unwrap_or(pa),
                max_nodes: as_usize(params, "max_nodes").unwrap_or(pn),
                max_tokens: as_usize(params, "max_tokens"),
                clip_chars: as_usize(params, "clip_chars"),
                sort: match params.get("sort").and_then(Value::as_str) {
                    Some("recent") => SortMode::Recent,
                    Some("oldest") => SortMode::Oldest,
                    _ => SortMode::Default,
                },
                window_curve: match params.get("window_curve").and_then(Value::as_str) {
                    Some("linear") => WindowCurve::Linear,
                    Some("log") => WindowCurve::Log,
                    _ => WindowCurve::Flat,
                },
                rg_options: opts,
                page: as_usize(params, "page"),
                page_size: as_usize(params, "page_size"),
                all: as_bool(params, "all"),
                fuzzy: as_bool(params, "fuzzy"),
                stdin_content: None,
            };

            let (result, meta) =
                search_with_meta(&config).map_err(|e| ServerError::internal(e.to_string()))?;

            // The SERVER path threads fuzzy/no-fill into the envelope (unlike
            // the CLI). When `format: "agent-json"` is requested, return the
            // envelope; otherwise the raw SearchResult.
            match params.get("format").and_then(Value::as_str) {
                Some("agent-json") => {
                    let envelope = build_agent_envelope(
                        &result,
                        EnvelopeOpts {
                            no_fill: as_bool(params, "no_fill"),
                            fuzzy_fired: meta.fuzzy_fired,
                            literal_match_count: Some(meta.literal_match_count),
                        },
                    );
                    // Re-parse the pretty string into a Value so the transport
                    // serializes it as a nested object, not a string.
                    serde_json::from_str(&format_agent_json(&envelope))
                        .map_err(|e| ServerError::internal(e.to_string()))
                }
                _ => serde_json::to_value(&result).map_err(|e| ServerError::internal(e.to_string())),
            }
        }

        // ── Health ──────────────────────────────────────────────────────
        "health" => Ok(json!({
            "ok": true,
            "version": SERVER_VERSION,
            "palace_path": default_palace_path().to_string_lossy(),
        })),

        // ── Tool spec ───────────────────────────────────────────────────
        "tool_spec" => {
            let fmt = as_str(params, "format").unwrap_or_else(|| "anthropic".to_string());
            scrt_core::tool_spec::build_tool_spec(&fmt)
                .map_err(|e| ServerError::bad(format!("tool_spec: {e}")))
        }

        // ── Palace: list ────────────────────────────────────────────────
        "palace.list" => {
            let tags = as_str_array_either(params, "tag_filter", "tags");
            let palace = FilePalace::load(palace_path(params), &SystemClock);
            let stashes = list_stashes(palace.data(), &tags);
            serde_json::to_value(stashes).map_err(|e| ServerError::internal(e.to_string()))
        }

        // ── Palace: get ─────────────────────────────────────────────────
        "palace.get" => {
            let name = as_str(params, "name")
                .ok_or_else(|| ServerError::bad("palace.get: params.name (string) is required"))?;
            let palace = FilePalace::load(palace_path(params), &SystemClock);
            match get_stash(palace.data(), &name) {
                None => Ok(Value::Null),
                Some(s) => {
                    if as_bool(params, "with_nodes") {
                        serde_json::to_value(s).map_err(|e| ServerError::internal(e.to_string()))
                    } else {
                        // Card view: omit the `nodes` key.
                        let mut v = serde_json::to_value(s)
                            .map_err(|e| ServerError::internal(e.to_string()))?;
                        if let Some(obj) = v.as_object_mut() {
                            obj.remove("nodes");
                        }
                        Ok(v)
                    }
                }
            }
        }

        // ── Palace: stash ───────────────────────────────────────────────
        "palace.stash" => {
            let name = as_str(params, "name")
                .ok_or_else(|| ServerError::bad("palace.stash: params.name (string) is required"))?;
            let note = as_str(params, "note")
                .ok_or_else(|| ServerError::bad("palace.stash: params.note (string) is required"))?;
            // Accept pre-built nodes (Node[]). Default empty.
            let nodes: Vec<scrt_core::types::Node> = params
                .get("nodes")
                .and_then(|n| serde_json::from_value(n.clone()).ok())
                .unwrap_or_default();
            let tags = as_str_array(params, "tags");
            let options = StashOptions {
                replace: as_bool(params, "replace"),
                locations: as_bool(params, "locations"),
                ttl: as_str(params, "ttl"),
            };
            let path = palace_path(params);
            let mut palace = FilePalace::load(&path, &SystemClock);
            let meta = StashSearch {
                pattern: as_str(params, "pattern").unwrap_or_default(),
                effort: as_str(params, "effort").unwrap_or_else(|| "quick".into()),
                sources_count: 0,
            };
            let action = add_stash(
                palace.data_mut(),
                &SystemClock,
                &name,
                &note,
                &nodes,
                meta,
                &[],
                &tags,
                &options,
            )
            .map_err(ServerError::internal)?;
            palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            Ok(json!({ "action": action_str(action), "name": name }))
        }

        // ── Palace: drop ────────────────────────────────────────────────
        "palace.drop" => {
            let name = as_str(params, "name")
                .ok_or_else(|| ServerError::bad("palace.drop: params.name (string) is required"))?;
            let path = palace_path(params);
            let mut palace = FilePalace::load(&path, &SystemClock);
            let ok = scrt_core::palace::ops::drop_stash(palace.data_mut(), &name);
            if ok {
                palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            }
            Ok(json!({ "dropped": ok }))
        }

        // ── Palace: compose / intersect / except (return [id]) ───────────
        "palace.compose" => {
            let names = as_str_array(params, "names");
            if names.is_empty() {
                return Err(ServerError::bad(
                    "palace.compose: params.names (string[]) is required",
                ));
            }
            compose_to_sources_path(&palace_path(params), &names)
                .map(|ids| json!(ids))
                .map_err(ServerError::internal)
        }
        "palace.intersect" => {
            let names = as_str_array(params, "names");
            if names.is_empty() {
                return Err(ServerError::bad(
                    "palace.intersect: params.names (string[]) is required",
                ));
            }
            intersect_to_sources_path(&palace_path(params), &names)
                .map(|ids| json!(ids))
                .map_err(ServerError::internal)
        }
        "palace.except" => {
            let base = as_str(params, "base")
                .ok_or_else(|| ServerError::bad("palace.except: params.base (string) is required"))?;
            let exclude = as_str_array(params, "exclude");
            if exclude.is_empty() {
                return Err(ServerError::bad(
                    "palace.except: params.exclude (string[]) is required",
                ));
            }
            except_to_sources_path(&palace_path(params), &base, &exclude)
                .map(|ids| json!(ids))
                .map_err(ServerError::internal)
        }

        // ── Palace: link ────────────────────────────────────────────────
        "palace.link" => {
            let from = as_str(params, "from");
            let to = as_str(params, "to");
            let rel_type = as_str(params, "type");
            let (from, to, rel_type) = match (from, to, rel_type) {
                (Some(f), Some(t), Some(ty)) => (f, t, ty),
                _ => {
                    return Err(ServerError::bad(
                        "palace.link: from, to, type (string) are required",
                    ))
                }
            };
            let note = as_str(params, "note").unwrap_or_default();
            let path = palace_path(params);
            let mut palace = FilePalace::load(&path, &SystemClock);
            add_relation(palace.data_mut(), &SystemClock, &from, &to, &rel_type, &note)
                .map_err(ServerError::internal)?;
            palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            Ok(json!({ "linked": true, "from": from, "to": to, "type": rel_type }))
        }

        // ── Palace: graph ───────────────────────────────────────────────
        "palace.graph" => {
            let name = as_str(params, "name")
                .ok_or_else(|| ServerError::bad("palace.graph: params.name (string) is required"))?;
            let depth = as_usize(params, "depth").unwrap_or(3);
            let palace = FilePalace::load(palace_path(params), &SystemClock);
            let graph = traversal_graph(palace.data(), &name, depth);
            Ok(graph_to_json(&graph))
        }

        // ── Palace: similar (SimHash similarity / link discovery) ────────
        "palace.similar" => {
            use scrt_core::palace::simhash::{
                load_sidecar, rank_similar, reconcile, save_sidecar, signature_stash, AxisUsed,
                MatchAxis, SimMethod, SimQuery,
            };
            let path = palace_path(params);
            let palace = FilePalace::load(&path, &SystemClock);

            // Reconcile + lazily backfill the fingerprint sidecar.
            let (sidecar, changed) = reconcile(palace.data(), &load_sidecar(&path));
            if changed {
                let _ = save_sidecar(&path, &sidecar);
            }

            let axis = match params.get("match").and_then(Value::as_str) {
                Some("full") => MatchAxis::Full,
                Some("vector") => MatchAxis::Vector,
                _ => MatchAxis::Note,
            };
            let score = as_usize(params, "score").unwrap_or(5).clamp(1, 10) as u8;
            let top = as_usize(params, "top");

            // Query is either an existing stash (`name`) or a raw `term`.
            let (query, exclude) = if let Some(name) = as_str(params, "name") {
                let stash = palace.data().stashes.get(&name).ok_or_else(|| {
                    ServerError::bad(format!("palace.similar: no such stash: {name}"))
                })?;
                let sig = sidecar
                    .by_stash
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| signature_stash(stash));
                (SimQuery::from_signature(&sig), Some(name))
            } else if let Some(term) = as_str(params, "term") {
                (SimQuery::from_term(&term), None)
            } else {
                return Err(ServerError::bad(
                    "palace.similar: either params.name (stash) or params.term (string) is required",
                ));
            };

            let hits = rank_similar(
                palace.data(),
                &sidecar,
                &query,
                axis,
                score,
                exclude.as_deref(),
                top,
            );
            let arr: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "name": h.name,
                        "relevance": (h.relevance * 1000.0).round() / 1000.0,
                        "method": match h.method {
                            SimMethod::Scalar => "scalar",
                            SimMethod::Chunked => "chunked",
                            SimMethod::RandProj => "vector",
                        },
                        "axis": match h.axis_used {
                            AxisUsed::Note => "note",
                            AxisUsed::FullProse => "prose",
                            AxisUsed::FullTyped => "typed",
                        },
                        "best_pair": h.best_pair,
                        "jaccard": h.jaccard,
                    })
                })
                .collect();
            Ok(json!(arr))
        }

        // ── Palace: prune_* ─────────────────────────────────────────────
        "palace.prune_expired" => {
            let path = palace_path(params);
            let dry = as_bool(params, "dry_run");
            let mut palace = FilePalace::load(&path, &SystemClock);
            let r = prune_expired(palace.data_mut(), &SystemClock, dry);
            if !dry {
                palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            }
            Ok(prune_to_json(&r))
        }
        "palace.prune_tag" => {
            let tag = as_str(params, "tag")
                .ok_or_else(|| ServerError::bad("palace.prune_tag: params.tag (string) is required"))?;
            let path = palace_path(params);
            let dry = as_bool(params, "dry_run");
            let mut palace = FilePalace::load(&path, &SystemClock);
            let r = prune_tag(palace.data_mut(), &tag, dry);
            if !dry {
                palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            }
            Ok(prune_to_json(&r))
        }
        "palace.prune_older_than" => {
            let duration = as_str(params, "duration").ok_or_else(|| {
                ServerError::bad(
                    "palace.prune_older_than: params.duration (string) is required, e.g. '24h'",
                )
            })?;
            let path = palace_path(params);
            let dry = as_bool(params, "dry_run");
            let mut palace = FilePalace::load(&path, &SystemClock);
            let r = prune_older_than(palace.data_mut(), &SystemClock, &duration, dry)
                .map_err(ServerError::bad)?;
            if !dry {
                palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            }
            Ok(prune_to_json(&r))
        }
        "palace.prune_keep" => {
            let n = as_usize(params, "n")
                .ok_or_else(|| ServerError::bad("palace.prune_keep: params.n (number) is required"))?;
            let path = palace_path(params);
            let dry = as_bool(params, "dry_run");
            let mut palace = FilePalace::load(&path, &SystemClock);
            let r = prune_keep(palace.data_mut(), n, dry);
            if !dry {
                palace.save().map_err(|e| ServerError::internal(e.to_string()))?;
            }
            Ok(prune_to_json(&r))
        }

        _ => Err(ServerError {
            code: ErrorCode::UnknownMethod,
            message: format!("Unknown method: {method}"),
        }),
    }
}

fn action_str(a: scrt_core::palace::ops::StashAction) -> &'static str {
    use scrt_core::palace::ops::StashAction::*;
    match a {
        Created => "created",
        Replaced => "replaced",
        Merged => "merged",
    }
}

fn prune_to_json(r: &scrt_core::palace::prune::PruneResult) -> Value {
    json!({ "removed": r.removed, "names": r.names, "dry_run": r.dry_run })
}

fn graph_to_json(graph: &[scrt_core::palace::relations::GraphNode]) -> Value {
    let arr: Vec<Value> = graph
        .iter()
        .map(|g| {
            json!({
                "stash": g.stash_name,
                "depth": g.depth,
                "direction": match g.direction { Direction::Outbound => "outbound", Direction::Inbound => "inbound" },
                "via": g.via,
                "relation": {
                    "target": g.relation.target,
                    "type": g.relation.rel_type,
                    "note": g.relation.note,
                    "created_at": g.relation.created_at,
                },
            })
        })
        .collect();
    json!(arr)
}
