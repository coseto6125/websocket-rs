#![allow(non_local_definitions)]
#![allow(clippy::useless_conversion)]

use futures::prelude::*;
use parking_lot::RwLock;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream,
    WebSocketStream as TungsteniteWebSocketStream,
};

const DEFAULT_CONNECT_TIMEOUT: f64 = 30.0;
const DEFAULT_RECEIVE_TIMEOUT: f64 = 30.0;

type WebSocketStream = TungsteniteWebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// 全局 Runtime - 使用專用線程池
fn get_runtime() -> &'static Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .thread_name("ws-runtime")
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

#[pyclass]
struct WebSocket {
    url: String,
    stream: Arc<AsyncMutex<Option<WebSocketStream>>>,
    stream_sync: Arc<RwLock<bool>>, // 用於快速檢查連接狀態
    connect_timeout: f64,
    receive_timeout: f64,
}

#[pymethods]
impl WebSocket {
    #[new]
    #[pyo3(signature = (url, connect_timeout=None, receive_timeout=None))]
    fn new(url: String, connect_timeout: Option<f64>, receive_timeout: Option<f64>) -> Self {
        WebSocket {
            url,
            stream: Arc::new(AsyncMutex::new(None)),
            stream_sync: Arc::new(RwLock::new(false)),
            connect_timeout: connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            receive_timeout: receive_timeout.unwrap_or(DEFAULT_RECEIVE_TIMEOUT),
        }
    }

    /// 連接 WebSocket
    fn connect(&self, py: Python<'_>) -> PyResult<()> {
        let url = self.url.clone();
        let connect_timeout = self.connect_timeout;
        let stream = self.stream.clone();
        let stream_sync = self.stream_sync.clone();

        py.allow_threads(|| {
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
                    rt.block_on(async {
                        *stream.lock().await = Some(ws_stream);
                    });
                    *stream_sync.write() = true;
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

    /// 關閉連接
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let stream = self.stream.clone();
        let stream_sync = self.stream_sync.clone();

        py.allow_threads(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                if let Some(ref mut ws) = *guard {
                    let _ = ws.close(None).await;
                }
                *guard = None;
            });
            *stream_sync.write() = false;
            Ok(())
        })
    }

    /// 標準 send 方法 - 與 Python websockets 兼容
    fn send(&self, py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<()> {
        self.send_sync(py, message)
    }

    /// 標準 recv 方法 - 與 Python websockets 兼容
    fn recv(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.receive_sync(py)
    }

    /// receive 別名（為了兼容不同風格）
    fn receive(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.receive_sync(py)
    }

    /// 內部同步發送實現
    fn send_sync(&self, py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<()> {
        let msg = if let Ok(s) = message.downcast::<PyString>() {
            let cow = s.to_cow()?;
            match cow {
                Cow::Borrowed(borrowed) => Message::Text(borrowed.to_owned()),
                Cow::Owned(owned) => Message::Text(owned),
            }
        } else if let Ok(b) = message.downcast::<PyBytes>() {
            Message::Binary(b.as_bytes().to_vec())
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };

        let stream = self.stream.clone();

        py.allow_threads(|| {
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

    /// 內部同步接收實現
    fn receive_sync(&self, py: Python<'_>) -> PyResult<PyObject> {
        let stream = self.stream.clone();
        let receive_timeout = self.receive_timeout;

        let msg = py.allow_threads(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                timeout(Duration::from_secs_f64(receive_timeout), ws.next())
                    .await
                    .map_err(|_| {
                        PyTimeoutError::new_err(format!(
                            "Receive timed out ({} seconds)",
                            receive_timeout
                        ))
                    })?
                    .ok_or_else(|| PyRuntimeError::new_err("Connection closed"))?
                    .map_err(|e| PyRuntimeError::new_err(format!("Receive failed: {}", e)))
            })
        })?;

        match msg {
            Message::Text(text) => Ok(text.to_object(py)),
            Message::Binary(data) => Ok(PyBytes::new_bound(py, &data).to_object(py)),
            Message::Ping(_) | Message::Pong(_) => Ok(py.None()),
            Message::Close(_) => Err(PyRuntimeError::new_err("Connection closed by server")),
            _ => Err(PyRuntimeError::new_err("Received unsupported message type")),
        }
    }

    /// 批量發送 - 擴展 API
    fn send_batch(&self, py: Python<'_>, messages: Vec<String>) -> PyResult<()> {
        let stream = self.stream.clone();
        let msgs: Vec<Message> = messages.into_iter().map(Message::Text).collect();

        py.allow_threads(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                for msg in msgs {
                    ws.send(msg)
                        .await
                        .map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))?;
                }
                Ok(())
            })
        })
    }

    /// 批量接收 - 擴展 API
    fn receive_batch(&self, py: Python<'_>, count: usize) -> PyResult<Vec<PyObject>> {
        let stream = self.stream.clone();
        let receive_timeout = self.receive_timeout;

        let messages = py.allow_threads(|| {
            let rt = get_runtime();
            rt.block_on(async {
                let mut guard = stream.lock().await;
                let ws = guard
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

                let mut results = Vec::with_capacity(count);
                for _ in 0..count {
                    match timeout(Duration::from_secs_f64(receive_timeout), ws.next()).await {
                        Ok(Some(Ok(msg))) => results.push(msg),
                        Ok(Some(Err(e))) => {
                            return Err(PyRuntimeError::new_err(format!("Receive failed: {}", e)));
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                Ok(results)
            })
        })?;

        Ok(messages
            .into_iter()
            .filter_map(|msg| match msg {
                Message::Text(text) => Some(text.to_object(py)),
                Message::Binary(data) => Some(PyBytes::new_bound(py, &data).to_object(py)),
                _ => None,
            })
            .collect())
    }

    /// 檢查連接狀態
    #[getter]
    fn is_connected(&self) -> bool {
        *self.stream_sync.read()
    }

    /// Context manager 支援 - 進入
    fn __enter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        slf.borrow(py).connect(py)?;
        Ok(slf)
    }

    /// Context manager 支援 - 退出
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

    /// Async context manager 支援 - 進入
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        slf.borrow(py).connect(py)?;
        Ok(slf)
    }

    /// Async context manager 支援 - 退出
    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __aexit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }
}

#[pymodule]
fn websocket_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<WebSocket>()?;
    Ok(())
}
