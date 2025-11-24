use futures_util::{SinkExt, StreamExt};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyStopAsyncIteration, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::sync::OnceLock;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream};

use crate::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT};

// Type alias to simplify complex types
type MessageReceiver = Arc<AsyncMutex<mpsc::Receiver<Result<Message, String>>>>;

// Cache asyncio parts to avoid repeated imports
static ASYNCIO: OnceLock<Py<PyModule>> = OnceLock::new();

fn get_asyncio(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    if let Some(module) = ASYNCIO.get() {
        return Ok(module.bind(py).clone());
    }
    let module = py.import("asyncio")?;
    let module_perm = module.clone().unbind();
    ASYNCIO.set(module_perm).ok();
    Ok(module)
}

fn create_sys_future<'py>(
    _py: Python<'py>,
    loop_obj: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    loop_obj.call_method0("create_future")
}

fn set_future_result<'py>(
    _py: Python<'py>,
    loop_obj: &Bound<'py, PyAny>,
    future: &Bound<'py, PyAny>,
    result: Py<PyAny>,
) -> PyResult<()> {
    let set_result = future.getattr("set_result")?;
    loop_obj.call_method1("call_soon_threadsafe", (set_result, result))?;
    Ok(())
}

fn set_future_exception(
    _py: Python<'_>,
    loop_obj: &Bound<'_, PyAny>,
    future: &Bound<'_, PyAny>,
    exc: PyErr,
) -> PyResult<()> {
    let set_exception = future.getattr("set_exception")?;
    loop_obj.call_method1("call_soon_threadsafe", (set_exception, exc))?;
    Ok(())
}

fn create_completed_future<'py>(py: Python<'py>, result: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = get_asyncio(py)?;
    let loop_obj = asyncio.call_method0("get_event_loop")?;
    let future = loop_obj.call_method0("create_future")?;
    future.call_method1("set_result", (result,))?;
    Ok(future)
}

fn create_failed_future<'py>(py: Python<'py>, exc: PyErr) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = get_asyncio(py)?;
    let loop_obj = asyncio.call_method0("get_event_loop")?;
    let future = loop_obj.call_method0("create_future")?;
    future.call_method1("set_exception", (exc,))?;
    Ok(future)
}

// Optimized version: Create a completed future with minimal overhead
fn create_completed_future_fast<'py>(
    py: Python<'py>,
    result: impl IntoPyObject<'py>,
) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = get_asyncio(py)?;
    let future = asyncio
        .call_method0("get_event_loop")?
        .call_method0("create_future")?;
    future.call_method1("set_result", (result,))?;
    Ok(future)
}

/// Commands sent to the background actor
#[derive(Debug)]
enum Command {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

/// Async client connection
#[pyclass(name = "ClientConnection", module = "websocket_rs.async_client")]
pub struct AsyncClientConnection {
    url: String,
    // Communication with the background task
    tx_cmd: Option<mpsc::Sender<Command>>,
    rx_msg_internal: Option<MessageReceiver>,
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
impl AsyncClientConnection {
    #[new]
    #[pyo3(signature = (url, connect_timeout=None, receive_timeout=None))]
    fn new(url: String, connect_timeout: Option<f64>, receive_timeout: Option<f64>) -> Self {
        AsyncClientConnection {
            url,
            tx_cmd: None,
            rx_msg_internal: None,
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

    /// Send a message (async)
    fn send<'py>(
        &self,
        py: Python<'py>,
        message: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let tx_cloned = self
            .tx_cmd
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();

        let command = if let Ok(text) = message.extract::<String>() {
            Command::Text(text)
        } else if let Ok(bytes) = message.extract::<Vec<u8>>() {
            Command::Binary(bytes)
        } else {
            return Err(PyRuntimeError::new_err("Message must be str or bytes"));
        };
        // Optimistic Send: Try to send synchronously first
        match tx_cloned.try_send(command) {
            Ok(_) => {
                // Fast path: Use optimized future creation
                create_completed_future_fast(py, py.None())
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                // Channel full, fallback to async wait (Backpressure)
                let asyncio = get_asyncio(py)?;
                let loop_obj = asyncio.call_method0("get_event_loop")?;
                let future = create_sys_future(py, &loop_obj)?;

                let future_ptr = future.clone().unbind();
                let loop_ptr = loop_obj.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);

                            if res.is_ok() {
                                let _ = set_future_result(py, loop_obj, future, py.None());
                            } else {
                                let _ = set_future_exception(
                                    py,
                                    loop_obj,
                                    future,
                                    PyRuntimeError::new_err("Failed to send message (actor died)"),
                                );
                            }
                        });
                    });
                });

                Ok(future)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PyRuntimeError::new_err("WebSocket is not connected"))
            }
        }
    }

    /// Receive a message (async)
    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let message_rx_arc = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();

        // Optimistic Recv: Try to receive synchronously first
        if let Ok(mut guard) = message_rx_arc.try_lock() {
            match guard.try_recv() {
                Ok(msg) => {
                    // Message available! Process immediately
                    let result = match msg {
                        Ok(Message::Text(text)) => Ok(text.into_pyobject(py)?.into_any().unbind()),
                        Ok(Message::Binary(data)) => {
                            let bytes = PyBytes::new_with(py, data.len(), |b| {
                                b.copy_from_slice(&data);
                                Ok(())
                            })?;
                            Ok(bytes.into_any().unbind())
                        }
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Ok(py.None()),
                        Ok(Message::Close(c)) => {
                            if let Some(frame) = c {
                                *close_code.write().unwrap() = Some(frame.code.into());
                                *close_reason.write().unwrap() = Some(frame.reason.to_string());
                            }
                            Err(PyRuntimeError::new_err("Connection closed by server"))
                        }
                        Ok(_) => Err(PyRuntimeError::new_err("Received unsupported message type")),
                        Err(e) => Err(PyRuntimeError::new_err(format!("Receive failed: {}", e))),
                    };

                    match result {
                        Ok(val) => {
                            // Fast path: Use optimized future creation
                            return create_completed_future_fast(py, val);
                        }
                        Err(e) => {
                            return create_failed_future(py, e);
                        }
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // Channel empty, proceed to async wait
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return create_failed_future(py, PyRuntimeError::new_err("Connection closed"));
                }
            }
        }

        // Slow Path: Async Wait
        let asyncio = get_asyncio(py)?;
        let loop_obj = asyncio.call_method0("get_event_loop")?;
        let future = create_sys_future(py, &loop_obj)?;

        let future_ptr = future.clone().unbind();
        let loop_ptr = loop_obj.unbind();

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let mut rx = message_rx_arc.lock().await;

                let msg_result = timeout(Duration::from_secs_f64(receive_timeout), rx.recv()).await;

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let loop_obj = loop_ptr.bind(py);

                    match msg_result {
                        Ok(Some(msg)) => {
                            let result = match msg {
                                Ok(Message::Text(text)) => {
                                    Ok(text.into_pyobject(py).unwrap().into_any().unbind())
                                }
                                Ok(Message::Binary(data)) => {
                                    let bytes = PyBytes::new_with(py, data.len(), |b| {
                                        b.copy_from_slice(&data);
                                        Ok(())
                                    })
                                    .unwrap();
                                    Ok(bytes.into_any().unbind())
                                }
                                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Ok(py.None()),
                                Ok(Message::Close(c)) => {
                                    if let Some(frame) = c {
                                        *close_code.write().unwrap() = Some(frame.code.into());
                                        *close_reason.write().unwrap() =
                                            Some(frame.reason.to_string());
                                    }
                                    Err(PyRuntimeError::new_err("Connection closed by server"))
                                }
                                Ok(_) => Err(PyRuntimeError::new_err(
                                    "Received unsupported message type",
                                )),
                                Err(e) => {
                                    Err(PyRuntimeError::new_err(format!("Receive failed: {}", e)))
                                }
                            };

                            match result {
                                Ok(val) => {
                                    let _ = set_future_result(py, loop_obj, future, val);
                                }
                                Err(e) => {
                                    let _ = set_future_exception(py, loop_obj, future, e);
                                }
                            }
                        }
                        Ok(None) => {
                            let _ = set_future_exception(
                                py,
                                loop_obj,
                                future,
                                PyRuntimeError::new_err("Connection closed"),
                            );
                        }
                        Err(_) => {
                            let _ = set_future_exception(
                                py,
                                loop_obj,
                                future,
                                PyTimeoutError::new_err(format!(
                                    "Receive timed out ({} seconds)",
                                    receive_timeout
                                )),
                            );
                        }
                    }
                });
            });
        });

        Ok(future)
    }

    /// Close the connection (async)
    fn close<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let asyncio = get_asyncio(py)?;
        let loop_obj = asyncio.call_method0("get_event_loop")?;
        let future = create_sys_future(py, &loop_obj)?;

        let future_ptr = future.clone().unbind();
        let loop_ptr = loop_obj.unbind();

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let mut tx_option = None;
                let mut rx_arc_option = None;
                let mut stream_sync_arc = None;

                // Acquire GIL to take ownership of fields and set stream_sync
                Python::attach(|py| {
                    let mut ws_mut = slf.bind(py).borrow_mut();
                    tx_option = ws_mut.tx_cmd.take(); // Take ownership
                    rx_arc_option = ws_mut.rx_msg_internal.take(); // Take ownership
                    stream_sync_arc = Some(ws_mut.stream_sync.clone()); // Clone Arc for later mutation
                });

                // Set stream_sync to false
                if let Some(arc) = stream_sync_arc {
                    *arc.write().unwrap() = false;
                }

                // 1. Send Close command (if tx exists)
                if let Some(tx) = tx_option {
                    let _ = tx.send(Command::Close).await;
                }

                // 2. Wait for actor to close (if rx exists)
                if let Some(rx_arc) = rx_arc_option {
                    let mut rx = rx_arc.lock().await;
                    while let Some(msg) = rx.recv().await {
                        match msg {
                            Ok(Message::Close(_)) => break,
                            Ok(_) => continue,
                            Err(_) => break,
                        }
                    }
                }

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let loop_obj = loop_ptr.bind(py);
                    let _ = set_future_result(py, loop_obj, future, py.None());
                });
            });
        });

        Ok(future)
    }

    /// Ping (async)
    fn ping<'py>(&self, py: Python<'py>, data: Option<Vec<u8>>) -> PyResult<Bound<'py, PyAny>> {
        let tx_cloned = self
            .tx_cmd
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let data = data.unwrap_or_default();

        // Optimistic Send
        match tx_cloned.try_send(Command::Ping(data)) {
            Ok(_) => {
                let future = create_completed_future(py, py.None())?;
                Ok(future)
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                let asyncio = get_asyncio(py)?;
                let loop_obj = asyncio.call_method0("get_event_loop")?;
                let future = create_sys_future(py, &loop_obj)?;

                let future_ptr = future.clone().unbind();
                let loop_ptr = loop_obj.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);
                            if res.is_ok() {
                                let _ = set_future_result(py, loop_obj, future, py.None());
                            } else {
                                let _ = set_future_exception(
                                    py,
                                    loop_obj,
                                    future,
                                    PyRuntimeError::new_err("Failed to send ping"),
                                );
                            }
                        });
                    });
                });

                Ok(future)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PyRuntimeError::new_err("WebSocket is not connected"))
            }
        }
    }

    /// Pong (async)
    fn pong<'py>(&self, py: Python<'py>, data: Option<Vec<u8>>) -> PyResult<Bound<'py, PyAny>> {
        let tx_cloned = self
            .tx_cmd
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let data = data.unwrap_or_default();

        // Optimistic Send
        match tx_cloned.try_send(Command::Pong(data)) {
            Ok(_) => {
                let future = create_completed_future(py, py.None())?;
                Ok(future)
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                let asyncio = get_asyncio(py)?;
                let loop_obj = asyncio.call_method0("get_event_loop")?;
                let future = create_sys_future(py, &loop_obj)?;

                let future_ptr = future.clone().unbind();
                let loop_ptr = loop_obj.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);
                            if res.is_ok() {
                                let _ = set_future_result(py, loop_obj, future, py.None());
                            } else {
                                let _ = set_future_exception(
                                    py,
                                    loop_obj,
                                    future,
                                    PyRuntimeError::new_err("Failed to send pong"),
                                );
                            }
                        });
                    });
                });

                Ok(future)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PyRuntimeError::new_err("WebSocket is not connected"))
            }
        }
    }

    // ... getters ...
    #[getter]
    fn open(&self) -> bool {
        *self.stream_sync.read().unwrap()
    }

    #[getter]
    fn closed(&self) -> bool {
        !*self.stream_sync.read().unwrap()
    }

    #[getter]
    fn local_address(&self) -> Option<(String, u16)> {
        self.local_addr.read().unwrap().as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.read().unwrap().as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    #[getter]
    fn close_code(&self) -> Option<u16> {
        *self.close_code.read().unwrap()
    }

    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.close_reason.read().unwrap().clone()
    }

    #[getter]
    fn subprotocol(&self) -> Option<String> {
        self.subprotocol.read().unwrap().clone()
    }

    /// Async context manager - enter
    fn __aenter__<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (url, connect_timeout, _stream_sync, local_addr, remote_addr) = {
            let ws = slf.bind(py).borrow();
            (
                ws.url.clone(),
                ws.connect_timeout,
                ws.stream_sync.clone(),
                ws.local_addr.clone(),
                ws.remote_addr.clone(),
            )
        };

        let asyncio = get_asyncio(py)?;
        let loop_obj = asyncio.call_method0("get_event_loop")?;
        let future = create_sys_future(py, &loop_obj)?;

        let future_ptr = future.clone().unbind();
        let loop_ptr = loop_obj.unbind();
        let slf_ptr = slf.clone_ref(py);

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let result = timeout(
                    Duration::from_secs_f64(connect_timeout),
                    connect_async(&url),
                )
                .await;

                match result {
                    Ok(Ok((ws_stream, _))) => {
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

                        // Create channels with larger buffer to reduce backpressure
                        let (tx_cmd_val, mut rx_cmd) = mpsc::channel::<Command>(256);
                        let (tx_msg, rx_msg_val) = mpsc::channel::<Result<Message, String>>(256);

                        // Update the fields on the AsyncClientConnection instance
                        Python::attach(|py| {
                            let mut ws_mut = slf_ptr.bind(py).borrow_mut();
                            ws_mut.tx_cmd = Some(tx_cmd_val);
                            ws_mut.rx_msg_internal = Some(Arc::new(AsyncMutex::new(rx_msg_val)));
                            *ws_mut.stream_sync.write().unwrap() = true; // Use ws_mut for stream_sync as well
                        });

                        // Spawn background actor
                        tokio::spawn(async move {
                            let (sink, stream) = ws_stream.split();

                            let mut sink = sink;
                            let mut stream = stream;

                            loop {
                                tokio::select! {
                                    cmd = rx_cmd.recv() => {
                                        match cmd {
                                            Some(cmd) => {
                                                // 處理第一個命令
                                                let mut close_requested = false;
                                                match cmd {
                                                    Command::Text(t) => { let _ = sink.send(Message::Text(t)).await; }
                                                    Command::Binary(b) => { let _ = sink.send(Message::Binary(b)).await; }
                                                    Command::Ping(d) => { let _ = sink.send(Message::Ping(d)).await; }
                                                    Command::Pong(d) => { let _ = sink.send(Message::Pong(d)).await; }
                                                    Command::Close => {
                                                        let _ = sink.close().await;
                                                        close_requested = true;
                                                    }
                                                }

                                                if close_requested {
                                                    // 如果是關閉命令,繼續讀取直到對方關閉或出錯
                                                    while let Some(msg) = stream.next().await {
                                                        match msg {
                                                            Ok(Message::Close(_)) => break,
                                                            Ok(_) => continue,
                                                            Err(_) => break,
                                                        }
                                                    }
                                                    break;
                                                }
                                            }
                                            None => break, // Channel closed
                                        }
                                    }
                                    msg = stream.next() => {
                                        match msg {
                                            Some(Ok(msg)) => {
                                                if tx_msg.send(Ok(msg)).await.is_err() {
                                                    break; // Receiver dropped
                                                }
                                            }
                                            Some(Err(e)) => {
                                                let _ = tx_msg.send(Err(e.to_string())).await;
                                                break;
                                            }
                                            None => break, // Stream ended
                                        }
                                    }
                                }
                            }
                            // Ensure sink is closed if we exit loop
                            let _ = sink.close().await;
                        });

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);
                            let _ = set_future_result(py, loop_obj, future, slf_ptr.into_any());
                        });
                    }
                    Ok(Err(e)) => {
                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);
                            let _ = set_future_exception(py, loop_obj, future, PyConnectionError::new_err(e.to_string()));
                        });
                    },
                    Err(_) => {
                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let loop_obj = loop_ptr.bind(py);
                            let _ = set_future_exception(py, loop_obj, future, PyTimeoutError::new_err(format!("Connection timed out ({} seconds)", connect_timeout)));
                        });
                    },
                }
            });
        });

        Ok(future)
    }

    /// Async context manager - exit
    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __aexit__<'py>(
        slf: Py<Self>, // Capture slf for passing to close
        py: Python<'py>,
        _exc_type: Option<&Bound<'py, PyAny>>,
        _exc_value: Option<&Bound<'py, PyAny>>,
        _traceback: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        AsyncClientConnection::close(slf, py) // Call close with slf
    }

    /// Async Iterator support - return self
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Async Iterator support - return next message
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let message_rx_arc = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();
        // Slow Path: Async Wait
        let asyncio = get_asyncio(py)?;
        let loop_obj = asyncio.call_method0("get_event_loop")?;
        let future = create_sys_future(py, &loop_obj)?;

        let future_ptr = future.clone().unbind();
        let loop_ptr = loop_obj.unbind();

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let mut rx = message_rx_arc.lock().await;

                let msg_result = timeout(Duration::from_secs_f64(receive_timeout), rx.recv()).await;

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let loop_obj = loop_ptr.bind(py);

                    match msg_result {
                        Ok(Some(msg)) => {
                            let result = match msg {
                                Ok(Message::Text(text)) => {
                                    Ok(text.into_pyobject(py).unwrap().into_any().unbind())
                                }
                                Ok(Message::Binary(data)) => {
                                    let bytes = PyBytes::new_with(py, data.len(), |b| {
                                        b.copy_from_slice(&data);
                                        Ok(())
                                    })
                                    .unwrap();
                                    Ok(bytes.into_any().unbind())
                                }
                                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Ok(py.None()), // Should ideally skip these
                                Ok(Message::Close(c)) => {
                                    if let Some(frame) = c {
                                        *close_code.write().unwrap() = Some(frame.code.into());
                                        *close_reason.write().unwrap() =
                                            Some(frame.reason.to_string());
                                    }
                                    Err(PyStopAsyncIteration::new_err(
                                        "Connection closed by server",
                                    ))
                                }
                                Ok(_) => Err(PyRuntimeError::new_err(
                                    "Received unsupported message type",
                                )),
                                Err(e) => {
                                    Err(PyRuntimeError::new_err(format!("Receive failed: {}", e)))
                                }
                            };

                            match result {
                                Ok(val) => {
                                    let _ = set_future_result(py, loop_obj, future, val);
                                }
                                Err(e) => {
                                    let _ = set_future_exception(py, loop_obj, future, e);
                                }
                            }
                        }
                        Ok(None) => {
                            let _ = set_future_exception(
                                py,
                                loop_obj,
                                future,
                                PyStopAsyncIteration::new_err("Connection closed"),
                            );
                        }
                        Err(_) => {
                            let _ = set_future_exception(
                                py,
                                loop_obj,
                                future,
                                PyTimeoutError::new_err(format!(
                                    "Receive timed out ({} seconds)",
                                    receive_timeout
                                )),
                            );
                        }
                    }
                });
            });
        });

        Ok(future)
    }
}

/// Connect to a WebSocket server (async)
#[pyfunction]
#[pyo3(signature = (uri, **_kwargs))]
pub fn connect<'py>(
    py: Python<'py>,
    uri: String,
    _kwargs: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let ws = AsyncClientConnection::new(uri, None, None);
    let ws_cell = Py::new(py, ws)?;

    // Call __aenter__ to connect
    AsyncClientConnection::__aenter__(ws_cell, py)
}

pub fn register_async_client(py: Python<'_>, parent_module: &Bound<'_, PyModule>) -> PyResult<()> {
    let async_client_module = PyModule::new(py, "async_client")?;

    async_client_module.add_class::<AsyncClientConnection>()?;
    async_client_module.add_function(wrap_pyfunction!(connect, &async_client_module)?)?;

    parent_module.add_submodule(&async_client_module)?;

    Ok(())
}
