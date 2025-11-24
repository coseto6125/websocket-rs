use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use pyo3::exceptions::{PyRuntimeError, PyTimeoutError, PyConnectionError};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time::timeout;

use crate::{get_runtime, AsyncMutex, WebSocketStream, DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT};

/// Sync client connection
#[pyclass(name = "ClientConnection", module = "websocket_rs.sync.client")]
pub struct SyncClientConnection {
    url: String,
    stream: Arc<AsyncMutex<Option<WebSocketStream>>>,
    stream_sync: Arc<RwLock<bool>>,
    connect_timeout: f64,
    receive_timeout: f64,
    // Connection info
    local_addr: Arc<RwLock<Option<String>>>,
    remote_addr: Arc<RwLock<Option<String>>>,
    subprotocol: Arc<RwLock<Option<String>>>,
    // Close info
    close_code: Arc<RwLock<Option<u16>>>,
    close_reason: Arc<RwLock<Option<String>>>,
}

#[pymethods]
impl SyncClientConnection {
    #[new]
    #[pyo3(signature = (url, connect_timeout=None, receive_timeout=None))]
    fn new(url: String, connect_timeout: Option<f64>, receive_timeout: Option<f64>) -> Self {
        SyncClientConnection {
            url,
            stream: Arc::new(AsyncMutex::new(None)),
            stream_sync: Arc::new(RwLock::new(false)),
            connect_timeout: connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            receive_timeout: receive_timeout.unwrap_or(DEFAULT_RECEIVE_TIMEOUT),
            local_addr: Arc::new(RwLock::new(None)),
            remote_addr: Arc::new(RwLock::new(None)),
            subprotocol: Arc::new(RwLock::new(None)),
            close_code: Arc::new(RwLock::new(None)),
            close_reason: Arc::new(RwLock::new(None)),
        }
    }

    /// Internal connect implementation
    fn __connect(&self, py: Python<'_>) -> PyResult<()> {
        let url = self.url.clone();
        let connect_timeout = self.connect_timeout;
        let stream = self.stream.clone();
        let stream_sync = self.stream_sync.clone();
        let local_addr = self.local_addr.clone();
        let remote_addr = self.remote_addr.clone();

        py.detach(|| {
            let rt = get_runtime();

            let result = rt.block_on(async {
                timeout(
                    Duration::from_secs_f64(connect_timeout),
                    connect_async(&url),
                )
                .await
            });

            match result {
                Ok(Ok((ws_stream, _))) => {
                    // Try to get addresses
                    match ws_stream.get_ref() {
                        MaybeTlsStream::Plain(s) => {
                            if let Ok(addr) = s.local_addr() {
                                *local_addr.write().unwrap() = Some(addr.to_string());
                            }
                            if let Ok(addr) = s.peer_addr() {
                                *remote_addr.write().unwrap() = Some(addr.to_string());
                            }
                        }
                        MaybeTlsStream::NativeTls(s) => {
                            if let Ok(addr) = s.get_ref().get_ref().get_ref().local_addr() {
                                *local_addr.write().unwrap() = Some(addr.to_string());
                            }
                            if let Ok(addr) = s.get_ref().get_ref().get_ref().peer_addr() {
                                *remote_addr.write().unwrap() = Some(addr.to_string());
                            }
                        }
                        _ => {}
                    }
                    
                    rt.block_on(async {
                        *stream.lock().await = Some(ws_stream);
                    });
                    *stream_sync.write().unwrap() = true;
                    Ok(())
                }
                Ok(Err(e)) => Err(PyConnectionError::new_err(e.to_string())),
                Err(_) => Err(PyTimeoutError::new_err(format!(
                    "Connection timed out ({} seconds)",
                    connect_timeout
                ))),
            }
        })
    }

    /// Send a message
    fn send<'py>(&self, py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<()> {
        let msg = if let Ok(s) = message.cast::<PyString>() {
            let text = s.to_string_lossy().to_string();
            Message::Text(text)
        } else if let Ok(b) = message.cast::<PyBytes>() {
            Message::Binary(b.as_bytes().to_vec())
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };

        let stream = self.stream.clone();

        py.detach(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                ws.send(msg)
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))
            })
        })
    }

    /// Receive a message
    fn recv(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let stream = self.stream.clone();
        let receive_timeout = self.receive_timeout;

        py.detach(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                // Loop until we get a non-Ping/Pong message
                loop {
                    let msg = timeout(Duration::from_secs_f64(receive_timeout), ws.next())
                        .await
                        .map_err(|_| {
                            PyTimeoutError::new_err(format!(
                                "Receive timed out ({} seconds)",
                                receive_timeout
                            ))
                        })?
                        .ok_or_else(|| PyRuntimeError::new_err("Connection closed"))?
                        .map_err(|e| PyRuntimeError::new_err(format!("Receive failed: {}", e)))?;

                    match msg {
                        Message::Text(text) => {
                            return Python::attach(|py| {
                                Ok(PyString::new(py, &text).into_any().unbind())
                            });
                        },
                        Message::Binary(data) => {
                            return Python::attach(|py| {
                                let bytes = PyBytes::new_with(py, data.len(), |b| {
                                    b.copy_from_slice(&data);
                                    Ok(())
                                }).unwrap();
                                Ok(bytes.into_any().unbind())
                            });
                        },
                        Message::Ping(_) | Message::Pong(_) => {
                            // Skip Ping/Pong and continue loop
                            continue;
                        },
                        Message::Close(c) => {
                            if let Some(frame) = c {
                                *self.close_code.write().unwrap() = Some(frame.code.into());
                                *self.close_reason.write().unwrap() = Some(frame.reason.to_string());
                            }
                            return Err(PyRuntimeError::new_err("Connection closed by server"));
                        },
                        _ => {
                            return Err(PyRuntimeError::new_err("Received unsupported message type"));
                        },
                    }
                }
            })
        })
    }

    /// Close the connection
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let stream = self.stream.clone();
        let stream_sync = self.stream_sync.clone();

        py.detach(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                if let Some(ref mut ws) = *guard {
                    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
                    if let Err(e) = ws.close(None::<CloseFrame>).await {
                        eprintln!("Error closing WebSocket: {}", e);
                    }
                }
                *guard = None;
            });
            *stream_sync.write().unwrap() = false;
            Ok(())
        })
    }

    /// Ping
    fn ping(&self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let stream = self.stream.clone();
        let data = data.unwrap_or_default();

        py.detach(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                ws.send(Message::Ping(data))
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("Ping failed: {}", e)))
            })
        })
    }

    /// Pong
    fn pong(&self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let stream = self.stream.clone();
        let data = data.unwrap_or_default();

        py.detach(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                ws.send(Message::Pong(data))
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("Pong failed: {}", e)))
            })
        })
    }

    /// Check if connection is open
    #[getter]
    fn open(&self) -> bool {
        *self.stream_sync.read().unwrap()
    }

    /// Check if connection is closed
    #[getter]
    fn closed(&self) -> bool {
        !*self.stream_sync.read().unwrap()
    }

    /// Local address
    #[getter]
    fn local_address(&self) -> Option<(String, u16)> {
        self.local_addr.read().unwrap().as_ref().and_then(|s| {
            s.rsplit_once(':').and_then(|(ip, port)| {
                port.parse().ok().map(|p| (ip.to_string(), p))
            })
        })
    }

    /// Remote address
    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.read().unwrap().as_ref().and_then(|s| {
            s.rsplit_once(':').and_then(|(ip, port)| {
                port.parse().ok().map(|p| (ip.to_string(), p))
            })
        })
    }

    /// Close code
    #[getter]
    fn close_code(&self) -> Option<u16> {
        *self.close_code.read().unwrap()
    }

    /// Close reason
    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.close_reason.read().unwrap().clone()
    }

    /// Subprotocol
    #[getter]
    fn subprotocol(&self) -> Option<String> {
        self.subprotocol.read().unwrap().clone()
    }

    /// Context manager - enter
    fn __enter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        slf.borrow(py).__connect(py)?;
        Ok(slf)
    }

    /// Context manager - exit
    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }

    /// Iterator support - return self
    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Iterator support - return next message
    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        match self.recv(py) {
            Ok(msg) => Ok(Some(msg)),
            Err(e) => {
                if e.is_instance_of::<PyRuntimeError>(py) && e.to_string().contains("Connection closed") {
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
