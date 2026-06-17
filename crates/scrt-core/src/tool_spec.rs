//! Tool-spec descriptors — port of v0.x `src/tool-spec.ts`. Emits the
//! OpenAI / Anthropic / Gemini function-calling shapes for the five tools.
//!
//! Branding: v0.x names them `mpg_*`; scrt emits `scrt_*` (DESIGN.md §5,
//! COMPAT.md §Branding). The schemas / descriptions are otherwise
//! byte-identical, so the parity harness normalizes `mpg_`↔`scrt_` before
//! diffing.

use serde_json::{json, Value};

/// The five tools' names, descriptions, and parameter schemas. Built as
/// raw JSON so the three provider adapters can wrap them identically.
fn tools() -> Vec<Value> {
    let search_properties = json!({
        "pattern": { "type": "string", "description": "Regex pattern to search for (ripgrep syntax). Required." },
        "in": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Paths to search: files, directories (recursive), globs, @file indirection, @- for a newline-delimited file list on stdin."
        },
        "cmd": { "type": "string", "description": "Search the stdout of a shell command (captured inline)." },
        "url": { "type": "string", "description": "Fetch and search a URL. Capped at 16 MB / 30 s." },
        "effort": {
            "type": "string",
            "enum": ["scan", "normal", "deep", "auto"],
            "description": "Effort preset controlling per-node context windows and node cap. scan=20t/100k-nodes (index pass), normal=500t/30n (default), deep=2000t/100n (detailed drill). Override with max_tokens/max_nodes."
        },
        "max_tokens": { "type": "number", "description": "Total token budget across all returned nodes. Nodes are truncated to fit. Combine with window_curve to shape the distribution." },
        "max_nodes": { "type": "number", "description": "Hard cap on number of nodes returned. Default varies by effort preset." },
        "clip_chars": { "type": "number", "description": "Sub-line clip mode: keep only N chars on each side of the matched span within the match line. Drops line-level before/after context." },
        "sort": {
            "type": "string",
            "enum": ["relevance", "recent", "oldest"],
            "description": "Node ordering. 'recent' = newest-edited files first (good with window_curve:linear). 'oldest' = oldest first. Default: rg traversal order."
        },
        "window_curve": {
            "type": "string",
            "enum": ["flat", "linear", "log"],
            "description": "Token-window decay across ranked nodes. flat=every node gets full window. linear=decays to ~10% at last rank (~40% token savings). log=gentler decay via full/log2(rank+2) (~53% savings). Pair linear/log with sort:recent for budget-efficient scans."
        },
        "retriever": { "type": "string", "description": "Source retriever hint (reserved for future routing; currently ignored)." },
        "mp_from": { "type": "string", "description": "Scope search to the file list from this named mind-palace stash. ~3× cheaper than re-searching the full tree." },
        "mp_stash": { "type": "string", "description": "After searching, save the result into a mind-palace stash with this name." },
        "mp_tag": { "type": "string", "description": "Tag(s) to apply when saving with mp_stash (comma-separated)." },
        "mp_ttl": { "type": "string", "description": "Auto-expiry for the stash created by mp_stash. Examples: '4h', '1d', '7d'. Pruned by palace.prune_expired." },
        "page": { "type": "number", "description": "1-indexed page number (enables pagination)." },
        "page_size": { "type": "number", "description": "Nodes per page. Default 10." }
    });

    let stash_properties = json!({
        "name": { "type": "string", "description": "Mind-palace slot name (kebab-case recommended)." },
        "note": { "type": "string", "description": "Free-form description of what this stash contains." },
        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for filtering and grouping stashes." },
        "replace": { "type": "boolean", "description": "If true, overwrite an existing stash with this name. Default: merge (dedup by file:line)." },
        "ttl": { "type": "string", "description": "Auto-expiry duration, e.g. '4h', '24h', '7d'." },
        "palace_path": { "type": "string", "description": "Override the palace file path." }
    });

    let similar_properties = json!({
        "name": { "type": "string", "description": "An existing stash to find neighbors of. Provide this OR `term`." },
        "term": { "type": "string", "description": "A raw query string to rank stashes against (no stash needed). Provide this OR `name`." },
        "match": {
            "type": "string",
            "enum": ["note", "full", "vector"],
            "description": "Similarity axis. note=intent only (the stash note, fast). full=intent+content via chunked best-pair/Jaccard (best for 'shares a section'). vector=weighted-cosine over a random-projection embedding (smoother lexical). Default: note. NOTE: all three are LEXICAL/structural — they match surface form, not meaning ('dog Rex' vs 'my pet's name' will NOT match)."
        },
        "score": { "type": "number", "description": "1–10 ranking falloff. 1=wide net (loosely related still rank), 10=tight (near-identical only). Does NOT change the displayed relevance, only the ordering spread. Default 5." },
        "top": { "type": "number", "description": "Return only the N closest. Default: all." },
        "palace_path": { "type": "string", "description": "Override the palace file path." }
    });

    vec![
        json!({
            "name": "scrt_search",
            "description": "Token-budgeted codebase search via ripgrep. Returns context nodes with file:line attribution sized in tokens, not lines. Use effort presets to control cost: scan for index passes, normal for targeted queries, deep for detailed drill-downs. Scope repeat searches to prior results with mp_from (3× cheaper than full re-scan). Combine sort:recent + window_curve:linear for recency-weighted scans.",
            "parameters": { "type": "object", "properties": search_properties, "required": ["pattern"] }
        }),
        json!({
            "name": "scrt_stash",
            "description": "Save search results into a named mind-palace slot. Stashes survive compaction and session boundaries. Recall with scrt_get_stash; use as a search target via scrt_search(mp_from). Merges by default (dedup by file:line); set replace:true to overwrite. After saving, scrt surfaces lexically-related existing stashes as LINK SUGGESTIONS (use scrt_link to connect them) — a low-effort way to build an interconnected palace as you go.",
            "parameters": { "type": "object", "properties": stash_properties, "required": ["name", "note"] }
        }),
        json!({
            "name": "scrt_list_stashes",
            "description": "List all named mind-palace slots. Filter by tags. Use before composing or re-searching to see what's already captured.",
            "parameters": { "type": "object", "properties": { "tag_filter": { "type": "array", "items": { "type": "string" }, "description": "Only return stashes carrying all of these tags." }, "palace_path": { "type": "string", "description": "Override palace file path." } }, "required": [] }
        }),
        json!({
            "name": "scrt_get_stash",
            "description": "Retrieve a mind-palace slot. Returns card view by default (metadata, tags, source paths, relations — no node bodies; much cheaper). Pass with_nodes:true to include captured node text.",
            "parameters": { "type": "object", "properties": { "name": { "type": "string", "description": "Stash name." }, "with_nodes": { "type": "boolean", "description": "Include captured node bodies. Default false." }, "palace_path": { "type": "string" } }, "required": ["name"] }
        }),
        json!({
            "name": "scrt_drop_stash",
            "description": "Permanently remove a mind-palace slot. Use when a line of investigation is complete to keep the palace below the 20-stash budget.",
            "parameters": { "type": "object", "properties": { "name": { "type": "string", "description": "Stash name to drop." }, "palace_path": { "type": "string" } }, "required": ["name"] }
        }),
        json!({
            "name": "scrt_similar",
            "description": "Rank mind-palace stashes by similarity to a given stash (`name`) or a raw query (`term`). Returns each neighbor's relevance (0–1), the method used (scalar/chunked/vector), and the axis (note/prose/typed). Use to discover related stashes worth linking (scrt_link), to find prior work before re-searching, or to navigate an accumulated palace. The signal is LEXICAL/structural (SimHash family) — it matches shared words and structure, not meaning; it will not bridge synonyms or paraphrases. Cheap, deterministic, no model.",
            "parameters": { "type": "object", "properties": similar_properties, "required": [] }
        }),
    ]
}

/// Build the provider-shaped tool descriptor. `format` is
/// `openai` | `anthropic` | `gemini` (port of `buildToolSpec`).
pub fn build_tool_spec(format: &str) -> Result<Value, String> {
    let tools = tools();
    match format {
        // OpenAI: array of { type:"function", function:{ name, description, parameters } }.
        "openai" => Ok(Value::Array(
            tools
                .into_iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t["name"],
                            "description": t["description"],
                            "parameters": t["parameters"],
                        }
                    })
                })
                .collect(),
        )),
        // Anthropic: array of { name, description, input_schema }.
        "anthropic" => Ok(Value::Array(
            tools
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t["name"],
                        "description": t["description"],
                        "input_schema": t["parameters"],
                    })
                })
                .collect(),
        )),
        // Gemini: { functionDeclarations: [ { name, description, parameters } ] }.
        "gemini" => Ok(json!({
            "functionDeclarations": tools.into_iter().map(|t| json!({
                "name": t["name"],
                "description": t["description"],
                "parameters": t["parameters"],
            })).collect::<Vec<_>>()
        })),
        other => Err(format!("Unknown tool-spec format: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_shape_and_branding() {
        let v = build_tool_spec("openai").unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 6);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "scrt_search");
        assert_eq!(arr[5]["function"]["name"], "scrt_similar");
        assert!(arr[0]["function"]["parameters"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("pattern")));
    }

    #[test]
    fn anthropic_uses_input_schema() {
        let v = build_tool_spec("anthropic").unwrap();
        assert_eq!(v[0]["name"], "scrt_search");
        assert!(v[0]["input_schema"]["properties"]["pattern"].is_object());
    }

    #[test]
    fn gemini_wraps_function_declarations() {
        let v = build_tool_spec("gemini").unwrap();
        assert_eq!(v["functionDeclarations"][1]["name"], "scrt_stash");
    }

    #[test]
    fn unknown_format_errors() {
        assert!(build_tool_spec("nope").is_err());
    }
}
