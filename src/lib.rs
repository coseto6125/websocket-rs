use pyo3::prelude::*;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::WebSocketStream as TokioWebSocketStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio::net::TcpStream;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

mod sync_client;
mod async_client;

// Type aliases
type WebSocketStream = TokioWebSocketStream<MaybeTlsStream<TcpStream>>;

// Constants
const DEFAULT_CONNECT_TIMEOUT: f64 = 10.0;
const DEFAULT_RECEIVE_TIMEOUT: f64 = 10.0;

// Global runtime
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn get_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Runtime::new().expect("Failed to create Tokio runtime")
    })
}



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
