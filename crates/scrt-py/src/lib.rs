//! # scrt-py
//!
//! Python bindings via PyO3 — exposes the `scrt-server` dispatcher
//! **in-process** so Python harnesses call the engine with zero subprocess
//! (the PyO3 analogue of `scrt-napi`).
//!
//! Build: `maturin develop --release` (or `cargo build -p scrt-py
//! --release` + rename the cdylib to `scrt_py.pyd` on Windows / `.so`
//! elsewhere). Then from Python:
//!
//! ```python
//! import scrt_py
//! resp = scrt_py.dispatch("search", '{"pattern": "TODO", "in": ["log.txt"]}')
//! # resp is a JSON string: {"result": ...} | {"error": {code, message}}
//! ```

use pyo3::prelude::*;

use scrt_server::dispatcher::dispatch as core_dispatch;

/// Dispatch a single engine request in-process. `params_json` is a JSON
/// **string** — passing strings across the FFI boundary is measurably cheaper
/// than marshalling deep objects. Returns a JSON string `{"result": ...}` or
/// `{"error": {code, message}}`, mirroring the HTTP transport envelope.
#[pyfunction]
fn dispatch(method: String, params_json: String) -> PyResult<String> {
    let params: serde_json::Value = serde_json::from_str(&params_json).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("params JSON parse error: {e}"))
    })?;
    let params = if params.is_object() {
        params
    } else {
        serde_json::json!({})
    };
    let out = match core_dispatch(&method, &params) {
        Ok(result) => serde_json::json!({ "result": result }),
        Err(e) => serde_json::json!({ "error": e.to_json() }),
    };
    Ok(out.to_string())
}

/// The engine version (for the Python side to assert compatibility).
#[pyfunction]
fn version() -> String {
    scrt_server::dispatcher::SERVER_VERSION.to_string()
}

/// The Python module: `scrt_py`.
#[pymodule]
fn scrt_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(dispatch, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    Ok(())
}
