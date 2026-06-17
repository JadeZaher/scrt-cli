//! HTTP server — port of `startHttpServer` in v0.x `server.ts`.
//!
//! `POST /`       `{ method, params }` → `{ result }` | `{ error }`
//! `GET  /health` → `{ ok, version, palace_path }`
//!
//! Status codes match v0.x: 200 normally, 404 for an unknown method (and
//! for non-`POST /` routes), 400 on parse / missing-method, 500 on an
//! internal panic-equivalent. Uses axum on tokio. Binds 127.0.0.1 by
//! default.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::dispatcher::{dispatch, ErrorCode, SERVER_VERSION};

/// Build the axum router (exposed for tests via `tower::ServiceExt`).
pub fn app() -> Router {
    Router::new()
        .route("/", post(handle_post))
        .route("/health", get(handle_health))
        .with_state(())
}

/// Start the HTTP server on `host:port`. Returns once bound; serves until
/// the process exits. `port = 0` lets the OS pick (the bound port is
/// logged). Mirrors v0.x default host `127.0.0.1`.
pub async fn serve_http(host: &str, port: u16) -> std::io::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    eprintln!("scrt server listening on http://{bound}");
    axum::serve(listener, app()).await
}

async fn handle_health() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "version": SERVER_VERSION,
        "palace_path": scrt_core::palace::default_palace_path().to_string_lossy(),
    }))
}

async fn handle_post(State(()): State<()>, body: axum::body::Bytes) -> impl IntoResponse {
    // Parse the JSON body manually so a parse error maps to 400 with the
    // v0.x error shape (axum's Json extractor would 422 with its own body).
    let parsed: Result<Value, _> = serde_json::from_slice(&body);
    let body = match parsed {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": { "code": "BAD_PARAMS", "message": "JSON parse error" } })),
            );
        }
    };

    let method = body.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": { "code": "BAD_PARAMS", "message": "method (string) is required" } }),
            ),
        );
    };
    let params = match body.get("params") {
        Some(v) if v.is_object() => v.clone(),
        _ => json!({}),
    };

    match dispatch(method, &params) {
        Ok(result) => (StatusCode::OK, Json(json!({ "result": result }))),
        Err(e) => {
            // v0.x: UNKNOWN_METHOD → 404, otherwise 200 with an error body.
            let status = if e.code == ErrorCode::UnknownMethod {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            };
            (status, Json(json!({ "error": e.to_json() })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_route() {
        let resp = app()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn post_health_method() {
        let req = Request::post("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"method":"health"}"#))
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["result"]["ok"], true);
    }

    #[tokio::test]
    async fn unknown_method_is_404() {
        let req = Request::post("/")
            .body(Body::from(r#"{"method":"nope"}"#))
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "UNKNOWN_METHOD");
    }

    #[tokio::test]
    async fn parse_error_is_400() {
        let req = Request::post("/").body(Body::from("{bad")).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "BAD_PARAMS");
    }
}
