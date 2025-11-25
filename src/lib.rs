use pyo3::prelude::*;

mod async_client;
mod sync_client;

// Constants
const DEFAULT_CONNECT_TIMEOUT: f64 = 10.0;
const DEFAULT_RECEIVE_TIMEOUT: f64 = 10.0;

#[pymodule]
fn websocket_rs(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize Tokio runtime via pyo3-async-runtimes
    pyo3_async_runtimes::tokio::get_runtime();

    // Register sync.client module
    sync_client::register_sync_client(py, m)?;

    // Register async_client module
    async_client::register_async_client(py, m)?;

    Ok(())
}
