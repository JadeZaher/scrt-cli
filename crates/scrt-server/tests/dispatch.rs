//! Dispatcher behavior + response-shape parity with v0.x `server.ts`.
//! Drives `dispatch()` directly (transport-independent) and asserts the
//! method set, param validation, error codes, and the result/error shapes
//! match the Node server.

use serde_json::json;

use scrt_server::dispatch;

#[test]
fn health_shape() {
    let r = dispatch("health", &json!({})).unwrap();
    assert_eq!(r["ok"], true);
    assert!(r["version"].is_string());
    assert!(r["palace_path"].is_string());
}

#[test]
fn tool_spec_default_anthropic_and_branding() {
    // Default format is anthropic (v0.x server.ts:140).
    let r = dispatch("tool_spec", &json!({})).unwrap();
    let arr = r.as_array().unwrap();
    assert_eq!(arr.len(), 6); // 5 mpg-inherited + scrt_similar
    // Anthropic shape uses input_schema, and names are scrt_*-branded.
    assert_eq!(arr[0]["name"], "scrt_search");
    assert!(arr[0]["input_schema"].is_object());
    assert_eq!(arr[5]["name"], "scrt_similar");
}

#[test]
fn unknown_method_code() {
    let e = dispatch("does.not.exist", &json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), "UNKNOWN_METHOD");
}

#[test]
fn search_requires_pattern() {
    let e = dispatch("search", &json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), "BAD_PARAMS");
}

#[test]
fn search_returns_searchresult_shape() {
    // Write a tiny corpus and search it through the dispatcher.
    let dir = std::env::temp_dir().join(format!("scrt-disp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("c.txt");
    std::fs::write(&file, "alpha TODO beta\nplain line\n").unwrap();

    let r = dispatch(
        "search",
        &json!({ "pattern": "TODO", "in": [file.to_string_lossy()], "effort": "normal" }),
    )
    .unwrap();
    assert_eq!(r["status"], "ok");
    assert_eq!(r["total_nodes"], 1);
    assert_eq!(r["nodes"][0]["match_text"], "alpha TODO beta");
    // Result key order is the v0.x literal order.
    assert!(r.get("pattern").is_some() && r.get("page_tokens").is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_agent_json_format_returns_envelope() {
    let dir = std::env::temp_dir().join(format!("scrt-disp-aj-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("c.txt");
    std::fs::write(&file, "x\n").unwrap();

    // No match + agent-json -> envelope with no_matches status.
    let r = dispatch(
        "search",
        &json!({ "pattern": "ZZZNOPE", "in": [file.to_string_lossy()], "format": "agent-json" }),
    )
    .unwrap();
    assert_eq!(r["status"], "no_matches");
    assert!(r.get("n_literal_matches").is_some());
    assert!(r.get("fallback_used").is_some()); // present (null)
    assert!(r["errors"].is_array());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palace_round_trip_via_dispatch() {
    let dir = std::env::temp_dir().join(format!("scrt-disp-pal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pal = dir.join("mind-palace.json");
    let pal_s = pal.to_string_lossy().to_string();

    let node = json!({
        "id": 1, "source": { "id": "f.txt", "type": "file" },
        "match_line": 1, "start_line": 1, "end_line": 1,
        "context_before": [], "match_text": "hit", "context_after": [],
        "match_spans": [[0, 3]], "tokens": 1
    });

    // stash
    let r = dispatch(
        "palace.stash",
        &json!({ "name": "s1", "note": "n", "tags": ["a"], "palace_path": pal_s, "nodes": [node] }),
    )
    .unwrap();
    assert_eq!(r["action"], "created");

    // list
    let r = dispatch("palace.list", &json!({ "palace_path": pal_s })).unwrap();
    assert_eq!(r.as_array().unwrap().len(), 1);

    // get (card view omits nodes)
    let r = dispatch("palace.get", &json!({ "name": "s1", "palace_path": pal_s })).unwrap();
    assert!(r.get("nodes").is_none(), "card view must omit nodes");
    assert_eq!(r["note"], "n");

    // get with_nodes includes them
    let r = dispatch(
        "palace.get",
        &json!({ "name": "s1", "palace_path": pal_s, "with_nodes": true }),
    )
    .unwrap();
    assert!(r.get("nodes").is_some());

    // drop
    let r = dispatch("palace.drop", &json!({ "name": "s1", "palace_path": pal_s })).unwrap();
    assert_eq!(r["dropped"], true);

    // get a missing stash -> null
    let r = dispatch("palace.get", &json!({ "name": "s1", "palace_path": pal_s })).unwrap();
    assert!(r.is_null());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palace_similar_ranks_and_requires_a_query() {
    let dir = std::env::temp_dir().join(format!("scrt-disp-sim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pal = dir.join("mind-palace.json");
    let pal_s = pal.to_string_lossy().to_string();

    let mk = |src: &str, text: &str| {
        json!({
            "id": 1, "source": { "id": src, "type": "file" },
            "match_line": 1, "start_line": 1, "end_line": 1,
            "context_before": [], "match_text": text, "context_after": [],
            "match_spans": [[0, 1]], "tokens": 1
        })
    };
    dispatch("palace.stash", &json!({ "name": "auth", "note": "auth login rate limit throttle attempts",
        "palace_path": pal_s, "nodes": [mk("auth.rs", "fn check_auth_rate_limit() {}")] })).unwrap();
    dispatch("palace.stash", &json!({ "name": "authz", "note": "auth login throttle rate limit guard",
        "palace_path": pal_s, "nodes": [mk("authz.rs", "fn check_auth_rate_limit() {}")] })).unwrap();
    dispatch("palace.stash", &json!({ "name": "pizza", "note": "pizza dessert food recipes toppings cheese",
        "palace_path": pal_s, "nodes": [mk("food.md", "best pizza here")] })).unwrap();

    // Similar to the "auth" stash: authz should outrank pizza.
    let r = dispatch("palace.similar", &json!({ "name": "auth", "match": "full", "palace_path": pal_s })).unwrap();
    let hits = r.as_array().unwrap();
    assert!(hits.len() >= 2, "self excluded, others ranked");
    assert_eq!(hits[0]["name"], "authz", "related stash ranks first");
    assert!(hits[0]["relevance"].as_f64().unwrap() > hits.last().unwrap()["relevance"].as_f64().unwrap());
    assert_eq!(hits[0]["method"], "chunked");

    // A raw term query (no stash) works too.
    let r = dispatch("palace.similar", &json!({ "term": "auth login throttle", "palace_path": pal_s })).unwrap();
    assert!(!r.as_array().unwrap().is_empty());

    // Neither name nor term -> BAD_PARAMS.
    let e = dispatch("palace.similar", &json!({ "palace_path": pal_s })).unwrap_err();
    assert_eq!(e.code.as_str(), "BAD_PARAMS");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palace_compose_intersect_except_return_id_lists() {
    let dir = std::env::temp_dir().join(format!("scrt-disp-set-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pal = dir.join("mind-palace.json");
    let pal_s = pal.to_string_lossy().to_string();

    let mk = |src: &str| {
        json!({
            "id": 1, "source": { "id": src, "type": "file" },
            "match_line": 1, "start_line": 1, "end_line": 1,
            "context_before": [], "match_text": "m", "context_after": [],
            "match_spans": [[0, 1]], "tokens": 1
        })
    };
    dispatch("palace.stash", &json!({ "name": "a", "note": "", "palace_path": pal_s, "nodes": [mk("x.txt"), mk("y.txt")] })).unwrap();
    dispatch("palace.stash", &json!({ "name": "b", "note": "", "palace_path": pal_s, "nodes": [mk("y.txt"), mk("z.txt")] })).unwrap();

    let comp = dispatch("palace.compose", &json!({ "names": ["a", "b"], "palace_path": pal_s })).unwrap();
    assert_eq!(comp.as_array().unwrap().len(), 3); // union

    let inter = dispatch("palace.intersect", &json!({ "names": ["a", "b"], "palace_path": pal_s })).unwrap();
    assert_eq!(inter, json!(["y.txt"]));

    let exc = dispatch("palace.except", &json!({ "base": "a", "exclude": ["b"], "palace_path": pal_s })).unwrap();
    assert_eq!(exc, json!(["x.txt"]));

    // missing required params.
    assert_eq!(dispatch("palace.compose", &json!({})).unwrap_err().code.as_str(), "BAD_PARAMS");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palace_prune_keep_via_dispatch() {
    let dir = std::env::temp_dir().join(format!("scrt-disp-prune-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pal_s = dir.join("mind-palace.json").to_string_lossy().to_string();
    let node = json!({ "id":1,"source":{"id":"f.txt","type":"file"},"match_line":1,"start_line":1,"end_line":1,"context_before":[],"match_text":"m","context_after":[],"match_spans":[[0,1]],"tokens":1 });
    for n in ["a", "b", "c"] {
        dispatch("palace.stash", &json!({ "name": n, "note": "", "palace_path": pal_s, "nodes": [node.clone()] })).unwrap();
    }
    let r = dispatch("palace.prune_keep", &json!({ "n": 2, "palace_path": pal_s })).unwrap();
    assert_eq!(r["removed"], 1);
    assert!(r["dry_run"] == json!(false));
    // prune_keep needs numeric n.
    assert_eq!(dispatch("palace.prune_keep", &json!({ "palace_path": pal_s })).unwrap_err().code.as_str(), "BAD_PARAMS");

    let _ = std::fs::remove_dir_all(&dir);
}
