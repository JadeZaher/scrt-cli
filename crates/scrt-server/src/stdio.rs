//! Stdio NDJSON server — port of `startStdioServer` in v0.x `server.ts`.
//!
//! One JSON request per line on stdin: `{ id, method, params }`.
//! One JSON response per line on stdout:
//!   `{ id, result }` | `{ id, error: { code, message } }`.
//!
//! Requests are processed **strictly sequentially** (matching v0.x), so a
//! later request never observes a half-applied palace mutation from an
//! earlier one. Uses tokio for async line I/O.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::dispatcher::{dispatch, ErrorCode};

/// Run the stdio server until stdin closes (EOF). Each line is one request.
pub async fn serve_stdio() -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let resp = handle_line(trimmed);
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"id":null,"error":{"code":"INTERNAL","message":"serialize failed"}}"#.to_string()
        });
        out.push('\n');
        stdout.write_all(out.as_bytes()).await?;
        stdout.flush().await?;
    }
    Ok(())
}

/// Parse one request line and dispatch it, producing the response Value.
/// Mirrors v0.x: malformed JSON → `{id:null, error:{BAD_PARAMS}}`; missing
/// method → BAD_PARAMS; otherwise dispatch and wrap as result/error.
fn handle_line(line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            return json!({
                "id": null,
                "error": { "code": ErrorCode::BadParams.as_str(), "message": "JSON parse error" }
            });
        }
    };

    // id passes through verbatim (defaults to null).
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str);
    // params must be an object; anything else → {}.
    let params = match req.get("params") {
        Some(v) if v.is_object() => v.clone(),
        _ => json!({}),
    };

    let Some(method) = method else {
        return json!({
            "id": id,
            "error": { "code": ErrorCode::BadParams.as_str(), "message": "method (string) is required" }
        });
    };

    match dispatch(method, &params) {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(e) => json!({ "id": id, "error": e.to_json() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_request_shape() {
        let resp = handle_line(r#"{"id":1,"method":"health"}"#);
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["ok"], true);
        assert!(resp["result"]["version"].is_string());
    }

    #[test]
    fn malformed_json_yields_null_id_bad_params() {
        let resp = handle_line("{not json");
        assert_eq!(resp["id"], Value::Null);
        assert_eq!(resp["error"]["code"], "BAD_PARAMS");
    }

    #[test]
    fn missing_method_is_bad_params() {
        let resp = handle_line(r#"{"id":"x","params":{}}"#);
        assert_eq!(resp["id"], "x");
        assert_eq!(resp["error"]["code"], "BAD_PARAMS");
    }

    #[test]
    fn unknown_method_code() {
        let resp = handle_line(r#"{"id":2,"method":"nope"}"#);
        assert_eq!(resp["error"]["code"], "UNKNOWN_METHOD");
    }

    #[test]
    fn search_missing_pattern() {
        let resp = handle_line(r#"{"id":3,"method":"search","params":{}}"#);
        assert_eq!(resp["error"]["code"], "BAD_PARAMS");
    }
}
