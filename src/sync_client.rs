use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect as tungstenite_connect, Message, WebSocket};

use crate::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT};

/// Sync client connection (pure sync, no async runtime overhead)
#[pyclass(name = "ClientConnection", module = "websocket_rs.sync.client")]
pub struct SyncClientConnection {
    url: String,
    ws: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    connect_timeout: f64,
    receive_timeout: f64,
    local_addr: Option<String>,
    remote_addr: Option<String>,
    close_code: Option<u16>,
    close_reason: Option<String>,
}

#[pymethods]
impl SyncClientConnection {
    #[new]
    #[pyo3(signature = (url, connect_timeout=None, receive_timeout=None))]
    fn new(url: String, connect_timeout: Option<f64>, receive_timeout: Option<f64>) -> Self {
        SyncClientConnection {
            url,
            ws: None,
            connect_timeout: connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            receive_timeout: receive_timeout.unwrap_or(DEFAULT_RECEIVE_TIMEOUT),
            local_addr: None,
            remote_addr: None,
            close_code: None,
            close_reason: None,
        }
    }

    /// Internal connect implementation
    fn __connect(&mut self, py: Python<'_>) -> PyResult<()> {
        let url = self.url.clone();
        let receive_timeout = self.receive_timeout;

        py.allow_threads(|| {
            let (mut ws, _) = tungstenite_connect(&url)
                .map_err(|e| PyConnectionError::new_err(format!("Connection failed: {}", e)))?;

            // Set read timeout and get addresses
            match ws.get_mut() {
                MaybeTlsStream::Plain(stream) => {
                    let timeout = Duration::from_secs_f64(receive_timeout);
                    stream.set_read_timeout(Some(timeout)).map_err(|e| {
                        PyRuntimeError::new_err(format!("Set timeout failed: {}", e))
                    })?;

                    if let Ok(addr) = stream.local_addr() {
                        self.local_addr = Some(addr.to_string());
                    }
                    if let Ok(addr) = stream.peer_addr() {
                        self.remote_addr = Some(addr.to_string());
                    }
                }
                MaybeTlsStream::NativeTls(stream) => {
                    let tcp_stream = stream.get_ref();
                    let timeout = Duration::from_secs_f64(receive_timeout);
                    tcp_stream.set_read_timeout(Some(timeout)).map_err(|e| {
                        PyRuntimeError::new_err(format!("Set timeout failed: {}", e))
                    })?;

                    if let Ok(addr) = tcp_stream.local_addr() {
                        self.local_addr = Some(addr.to_string());
                    }
                    if let Ok(addr) = tcp_stream.peer_addr() {
                        self.remote_addr = Some(addr.to_string());
                    }
                }
                _ => {}
            }

            self.ws = Some(ws);
            Ok(())
        })
    }

    /// Send a message
    fn send<'py>(&mut self, py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<()> {
        let msg = if let Ok(s) = message.cast::<PyString>() {
            Message::Text(s.to_string_lossy().to_string())
        } else if let Ok(b) = message.cast::<PyBytes>() {
            Message::Binary(b.as_bytes().to_vec())
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };

        py.allow_threads(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.send(msg)
                .map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))
        })
    }

    /// Receive a message
    fn recv(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let result = py.allow_threads(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            loop {
                let msg = ws.read().map_err(|e| {
                    if e.to_string().contains("timed out") {
                        PyTimeoutError::new_err(format!(
                            "Receive timed out ({} seconds)",
                            self.receive_timeout
                        ))
                    } else {
                        PyRuntimeError::new_err(format!("Receive failed: {}", e))
                    }
                })?;

                match msg {
                    Message::Text(text) => {
                        return Ok((true, text.into_bytes()));
                    }
                    Message::Binary(data) => {
                        return Ok((false, data));
                    }
                    Message::Ping(_) | Message::Pong(_) => {
                        continue;
                    }
                    Message::Close(frame) => {
                        if let Some(f) = frame {
                            self.close_code = Some(f.code.into());
                            self.close_reason = Some(f.reason.to_string());
                        }
                        return Err(PyRuntimeError::new_err("Connection closed by server"));
                    }
                    _ => {
                        return Err(PyRuntimeError::new_err("Received unsupported message type"));
                    }
                }
            }
        })?;

        // Create Python object with GIL
        let (is_text, data) = result;
        if is_text {
            Ok(PyString::new(py, std::str::from_utf8(&data).unwrap())
                .into_any()
                .unbind())
        } else {
            Ok(PyBytes::new(py, &data).into_any().unbind())
        }
    }

    /// Close the connection
    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            if let Some(mut ws) = self.ws.take() {
                if let Err(e) = ws.close(None) {
                    eprintln!("Error closing WebSocket: {}", e);
                }
            }
            Ok(())
        })
    }

    /// Ping
    fn ping(&mut self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let data = data.unwrap_or_default();

        py.allow_threads(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.send(Message::Ping(data))
                .map_err(|e| PyRuntimeError::new_err(format!("Ping failed: {}", e)))
        })
    }

    /// Pong
    fn pong(&mut self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let data = data.unwrap_or_default();

        py.allow_threads(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.send(Message::Pong(data))
                .map_err(|e| PyRuntimeError::new_err(format!("Pong failed: {}", e)))
        })
    }

    /// Check if connection is open
    #[getter]
    fn open(&self) -> bool {
        self.ws.is_some()
    }

    /// Check if connection is closed
    #[getter]
    fn closed(&self) -> bool {
        self.ws.is_none()
    }

    /// Local address
    #[getter]
    fn local_address(&self) -> Option<(String, u16)> {
        self.local_addr.as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    /// Remote address
    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    /// Close code
    #[getter]
    fn close_code(&self) -> Option<u16> {
        self.close_code
    }

    /// Close reason
    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.close_reason.clone()
    }

    /// Context manager - enter
    fn __enter__<'py>(
        mut slf: PyRefMut<'py, Self>,
        py: Python<'py>,
    ) -> PyResult<PyRefMut<'py, Self>> {
        slf.__connect(py)?;
        Ok(slf)
    }

    /// Context manager - exit
    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }

    /// Iterator support - return self
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Iterator support - return next message
    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        match self.recv(py) {
            Ok(msg) => Ok(Some(msg)),
            Err(e) => {
                if e.is_instance_of::<PyRuntimeError>(py)
                    && e.to_string().contains("Connection closed")
                {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Connect to a WebSocket server (sync)
#[pyfunction]
#[pyo3(signature = (uri, **_kwargs))]
pub fn connect(uri: String, _kwargs: Option<&Bound<'_, PyAny>>) -> PyResult<SyncClientConnection> {
    Ok(SyncClientConnection::new(uri, None, None))
}

pub fn register_sync_client(py: Python<'_>, parent_module: &Bound<'_, PyModule>) -> PyResult<()> {
    let sync_module = PyModule::new(py, "sync")?;
    let client_module = PyModule::new(py, "client")?;

    client_module.add_class::<SyncClientConnection>()?;
    client_module.add_function(wrap_pyfunction!(connect, &client_module)?)?;

    sync_module.add_submodule(&client_module)?;
    parent_module.add_submodule(&sync_module)?;

    Ok(())
}
