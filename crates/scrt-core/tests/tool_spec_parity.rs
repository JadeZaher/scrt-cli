//! tool-spec contract test.
//!
//! HISTORY: scrt began as a byte-for-byte port of the Node `mpg` tool-spec
//! (5 tools), and this test diffed against brand-normalized goldens. As of the
//! 2026-06 direction shift, **parity with mpg is no longer a goal** — scrt is
//! its own product and extends the surface (e.g. `scrt_similar`). So this test
//! now guards the *scrt* contract, not mpg-identity:
//!   1. all three provider shapes build and are well-formed,
//!   2. the 5 original mpg-derived tools are still present (a recognizable
//!      superset — agents migrating from mpg keep working),
//!   3. scrt's own extensions (scrt_similar) are present,
//!   4. per-tool key order is preserved (the load-bearing serde_json
//!      `preserve_order` detail that function-calling clients can be picky about).

use serde_json::Value;

/// The 5 tools inherited from the mpg port, in order, plus scrt's extensions.
const EXPECTED_TOOLS: &[&str] = &[
    "scrt_search",
    "scrt_stash",
    "scrt_list_stashes",
    "scrt_get_stash",
    "scrt_drop_stash",
    "scrt_similar", // scrt-only extension
];

fn spec(provider: &str) -> Value {
    scrt_core::tool_spec::build_tool_spec(provider).unwrap()
}

/// Extract the ordered tool-name list from any provider shape.
fn tool_names(provider: &str) -> Vec<String> {
    let v = spec(provider);
    let arr = match provider {
        "gemini" => v["functionDeclarations"].as_array().unwrap().clone(),
        _ => v.as_array().unwrap().clone(),
    };
    arr.iter()
        .map(|t| {
            // openai nests under `function`; anthropic/gemini are flat.
            t.get("function")
                .and_then(|f| f["name"].as_str())
                .or_else(|| t["name"].as_str())
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn all_providers_expose_the_same_ordered_tool_set() {
    for provider in ["openai", "anthropic", "gemini"] {
        assert_eq!(
            tool_names(provider),
            EXPECTED_TOOLS,
            "provider {provider} tool set/order drifted"
        );
    }
}

#[test]
fn mpg_inherited_tools_remain_a_superset() {
    // Migrating mpg agents rely on these 5 names continuing to exist.
    let names = tool_names("anthropic");
    for t in &EXPECTED_TOOLS[..5] {
        assert!(names.contains(&t.to_string()), "missing inherited tool {t}");
    }
}

#[test]
fn scrt_similar_is_present_and_well_formed() {
    let v = spec("anthropic");
    let similar = v
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "scrt_similar")
        .expect("scrt_similar tool present");
    let props = &similar["input_schema"]["properties"];
    assert!(props["name"].is_object(), "scrt_similar.name param");
    assert!(props["term"].is_object(), "scrt_similar.term param");
    assert!(props["match"].is_object(), "scrt_similar.match param");
    // `match` enumerates the three axes.
    let axes = props["match"]["enum"].as_array().unwrap();
    assert!(axes.contains(&Value::from("vector")), "vector axis advertised");
}

#[test]
fn search_tool_key_order_is_preserved() {
    // serde_json `preserve_order` must keep description before input_schema,
    // etc. — some function-calling clients are order-sensitive. Check the
    // anthropic shape's first tool emits keys in declaration order.
    let pretty = serde_json::to_string_pretty(&spec("anthropic")).unwrap();
    let name_at = pretty.find("\"name\"").unwrap();
    let desc_at = pretty.find("\"description\"").unwrap();
    let schema_at = pretty.find("\"input_schema\"").unwrap();
    assert!(name_at < desc_at && desc_at < schema_at, "tool key order drifted");
}

#[test]
fn unknown_format_errors() {
    assert!(scrt_core::tool_spec::build_tool_spec("nope").is_err());
}
