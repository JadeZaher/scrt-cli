//! # scrt-server
//!
//! The long-running engine. Wraps `scrt-core` in three transports over a
//! single shared [`dispatcher::dispatch`]:
//!
//! - **stdio NDJSON** ([`stdio`]) — `scrt --serve`. One JSON request per
//!   line in, one response per line out.
//! - **HTTP** ([`http`]) — `scrt --serve --serve-http --port <n>`.
//!   `POST /` with `{ method, params }`, `GET /health`.
//! - the dispatcher is re-exported for the NAPI binding (`scrt-napi`).
//!
//! Request/response shapes and the 17-method set match v0.x `server.ts`
//! exactly (see [`dispatcher`]).

pub mod dispatcher;
pub mod http;
pub mod stdio;

pub use dispatcher::{dispatch, ErrorCode, ServerError, SERVER_VERSION};
