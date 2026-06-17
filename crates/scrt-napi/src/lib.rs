//! # scrt-napi
//!
//! Node addon (napi-rs): exposes the `scrt-server` dispatcher in-process so
//! JS harnesses call the engine with zero subprocess and zero serialization
//! round-trip beyond the N-API boundary. The exported `dispatch` mirrors the
//! stdio/HTTP `{method, params} -> {result}|{error}` contract.
//!
//! Build: `napi build --release` (via @napi-rs/cli) or
//! `cargo build -p scrt-napi --release`, producing `scrt_napi.node`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use scrt_server::dispatcher::dispatch as core_dispatch;

/// Dispatch a single engine request in-process.
///
/// `method` is the method name; `params` is a JSON value (object). Returns
/// a JSON value: `{ "result": ... }` on success or `{ "error": { code,
/// message } }` on a dispatch error — the same envelope the HTTP transport
/// returns, so JS callers share one response handler across transports.
#[napi]
pub fn dispatch(method: String, params: serde_json::Value) -> serde_json::Value {
    // Coerce a non-object params into {} to match the transports.
    let params = if params.is_object() {
        params
    } else {
        serde_json::json!({})
    };
    match core_dispatch(&method, &params) {
        Ok(result) => serde_json::json!({ "result": result }),
        Err(e) => serde_json::json!({ "error": e.to_json() }),
    }
}

/// Convenience: dispatch from a JSON **string** (for callers that prefer to
/// pass a pre-serialized request body, e.g. relaying an NDJSON line).
#[napi]
pub fn dispatch_json(method: String, params_json: String) -> Result<serde_json::Value> {
    let params: serde_json::Value = serde_json::from_str(&params_json)
        .map_err(|e| Error::from_reason(format!("params JSON parse error: {e}")))?;
    Ok(dispatch(method, params))
}

/// The engine version, for the JS side to assert compatibility.
#[napi]
pub fn version() -> String {
    scrt_server::dispatcher::SERVER_VERSION.to_string()
}
