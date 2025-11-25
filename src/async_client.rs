use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyStopAsyncIteration, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::BoundObject;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream};

use crate::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT};

// Type alias to simplify complex types
type MessageReceiver = Arc<AsyncMutex<mpsc::Receiver<Result<Message, String>>>>;

/// A custom Future that completes immediately.
/// This avoids the overhead of asyncio.get_event_loop().create_future()
/// when the result is already available (Optimistic Send/Recv).
///
/// Supports both success and error paths for maximum fast-path coverage.
#[pyclass]
struct ReadyFuture {
    result: Option<Result<Py<PyAny>, PyErr>>,
}

// Cache StopIteration type to avoid repeated lookups
static STOP_ITERATION: OnceLock<Py<PyAny>> = OnceLock::new();

fn get_stop_iteration(py: Python<'_>) -> &Py<PyAny> {
    STOP_ITERATION.get_or_init(|| {
        py.get_type::<pyo3::exceptions::PyStopIteration>().into_any().unbind()
    })
}

#[pymethods]
impl ReadyFuture {
    fn __await__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&mut self, py: Python) -> PyResult<Py<PyAny>> {
        if let Some(res) = self.result.take() {
            match res {
                Ok(val) => {
                    // Success path: Use cached StopIteration type
                    let stop_iter = get_stop_iteration(py).bind(py);
                    let err = stop_iter.call1((val,))?;
                    Err(PyErr::from_value(err))
                }
                Err(e) => {
                    // Error path: Directly raise the exception
                    Err(e)
                }
            }
        } else {
            Err(pyo3::exceptions::PyStopIteration::new_err(()))
        }
    }
}

// Cache asyncio parts to avoid repeated imports
static ASYNCIO: OnceLock<Py<PyModule>> = OnceLock::new();

/// Process a received WebSocket message into a Python object.
/// This function is marked #[inline] to ensure zero-cost abstraction.
/// 
/// - `async_iter`: If true, uses PyStopAsyncIteration for Close messages (for __anext__),
///   otherwise uses PyRuntimeError (for recv)
#[inline]
fn process_message(
    py: Python,
    msg: Result<Message, String>,
    close_code: &Arc<RwLock<Option<u16>>>,
    close_reason: &Arc<RwLock<Option<String>>>,
    async_iter: bool,
) -> PyResult<Py<PyAny>> {
    match msg {
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
                *close_code.write() = Some(frame.code.into());
                *close_reason.write() = Some(frame.reason.to_string());
            }
            if async_iter {
                Err(PyStopAsyncIteration::new_err("Connection closed by server"))
            } else {
                Err(PyRuntimeError::new_err("Connection closed by server"))
            }
        }
        Ok(_) => Err(PyRuntimeError::new_err("Received unsupported message type")),
        Err(e) => Err(PyRuntimeError::new_err(format!("Receive failed: {}", e))),
    }
}

fn get_asyncio(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    if let Some(module) = ASYNCIO.get() {
        return Ok(module.bind(py).clone());
    }
    let module = py.import("asyncio")?;
    let module_perm = module.clone().unbind();
    ASYNCIO.set(module_perm).ok();
    Ok(module)
}

/// Get event loop from cache, with fallback to dynamic query
#[inline]
fn get_cached_event_loop<'py>(
    py: Python<'py>,
    cache: &Arc<RwLock<Option<Py<PyAny>>>>,
) -> PyResult<Bound<'py, PyAny>> {
    // Fast path: Use cache
    if let Some(loop_obj) = cache.read().as_ref() {
        return Ok(loop_obj.bind(py).clone());
    }

    // Slow path: Fallback to dynamic query (use get_running_loop for safety)
    let asyncio = get_asyncio(py)?;
    asyncio.call_method0("get_running_loop")
}

fn create_future<'py>(
    _py: Python<'py>,
    event_loop: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    event_loop.call_method0("create_future")
}

fn complete_future<'py>(
    _py: Python<'py>,
    event_loop: &Bound<'py, PyAny>,
    future: &Bound<'py, PyAny>,
    result: Py<PyAny>,
) -> PyResult<()> {
    let set_result = future.getattr("set_result")?;
    event_loop.call_method1("call_soon_threadsafe", (set_result, result))?;
    Ok(())
}

fn fail_future(
    _py: Python<'_>,
    event_loop: &Bound<'_, PyAny>,
    future: &Bound<'_, PyAny>,
    exc: PyErr,
) -> PyResult<()> {
    let set_exc = future.getattr("set_exception")?;
    event_loop.call_method1("call_soon_threadsafe", (set_exc, exc))?;
    Ok(())
}

fn ready_ok<'py>(py: Python<'py>, result: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = get_asyncio(py)?;
    let event_loop = asyncio.call_method0("get_running_loop")?;
    let future = event_loop.call_method0("create_future")?;
    future.call_method1("set_result", (result,))?;
    Ok(future)
}

fn ready_err<'py>(py: Python<'py>, exc: PyErr) -> PyResult<Bound<'py, PyAny>> {
    let asyncio = get_asyncio(py)?;
    let event_loop = asyncio.call_method0("get_running_loop")?;
    let future = event_loop.call_method0("create_future")?;
    future.call_method1("set_exception", (exc,))?;
    Ok(future)
}

// Fast path: Create completed future with minimal overhead (success)
fn ready_fast<'py>(
    py: Python<'py>,
    result: impl IntoPyObject<'py>,
) -> PyResult<Bound<'py, PyAny>> {
    // Use custom ReadyFuture to bypass asyncio overhead
    let obj = result
        .into_pyobject(py)
        .map_err(|_| PyRuntimeError::new_err("Conversion failed"))?;

    let future = Bound::new(
        py,
        ReadyFuture {
            result: Some(Ok(obj.into_any().unbind())),
        },
    )?;
    Ok(future.into_any())
}

// Fast path: Create completed future with error (error path optimization)
fn ready_fast_err<'py>(
    py: Python<'py>,
    err: PyErr,
) -> PyResult<Bound<'py, PyAny>> {
    // Use custom ReadyFuture to bypass asyncio overhead for errors
    let future = Bound::new(
        py,
        ReadyFuture {
            result: Some(Err(err)),
        },
    )?;
    Ok(future.into_any())
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
    // Event loop cache (per-connection optimization)
    event_loop: Arc<RwLock<Option<Py<PyAny>>>>,
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
            event_loop: Arc::new(RwLock::new(None)),
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
                ready_fast(py, py.None())
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                // Channel full, fallback to async wait (Backpressure)
                let event_loop = get_cached_event_loop(py, &self.event_loop)?;
                let future = create_future(py, &event_loop)?;

                let future_ptr = future.clone().unbind();
                let event_loop_ptr = event_loop.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);

                            if res.is_ok() {
                                if let Err(e) = complete_future(py, event_loop, future, py.None()) {
                                    eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                                }
                            } else {
                                if let Err(e) = fail_future(
                                    py,
                                    event_loop,
                                    future,
                                    PyRuntimeError::new_err("Failed to send message (actor died)"),
                                ) {
                                    eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                                }
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
        let rx = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();

        // Optimistic Recv: Try to receive synchronously first
        if let Ok(mut guard) = rx.try_lock() {
            match guard.try_recv() {
                Ok(msg) => {
                    // Message available! Process immediately
                    let result = process_message(py, msg, &close_code, &close_reason, false);
                    
                    match result {
                        Ok(val) => {
                            // Fast path: Use optimized future creation
                            return ready_fast(py, val);
                        }
                        Err(e) => {
                            // Fast error path: Use optimized error future
                            return ready_fast_err(py, e);
                        }
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // Channel empty, proceed to async wait
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Fast error path: Connection closed
                    return ready_fast_err(py, PyRuntimeError::new_err("Connection closed"));
                }
            }
        }

        // Slow Path: Async Wait
        let event_loop = get_cached_event_loop(py, &self.event_loop)?;
        let future = create_future(py, &event_loop)?;

        let future_ptr = future.clone().unbind();
        let event_loop_ptr = event_loop.unbind();

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let mut rx = rx.lock().await;

                let msg_result = timeout(Duration::from_secs_f64(receive_timeout), rx.recv()).await;

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let event_loop = event_loop_ptr.bind(py);

                    match msg_result {
                        Ok(Some(msg)) => {
                            let result = process_message(py, msg, &close_code, &close_reason, false);

                            match result {
                                Ok(val) => {
                                    if let Err(e) = complete_future(py, event_loop, future, val) {
                                        eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    if let Err(err) = fail_future(py, event_loop, future, e) {
                                        eprintln!("CRITICAL: Failed to set future exception: {:?}", err);
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            if let Err(e) = fail_future(
                                py,
                                event_loop,
                                future,
                                PyRuntimeError::new_err("Connection closed"),
                            ) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                            }
                        }
                        Err(_) => {
                            if let Err(e) = fail_future(
                                py,
                                event_loop,
                                future,
                                PyTimeoutError::new_err(format!(
                                    "Receive timed out ({} seconds)",
                                    receive_timeout
                                )),
                            ) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                            }
                        }
                    }
                });
            });
        });

        Ok(future)
    }

    /// Close the connection (async)
    fn close<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let event_loop_cache = slf.bind(py).borrow().event_loop.clone();
        let event_loop = get_cached_event_loop(py, &event_loop_cache)?;
        let future = create_future(py, &event_loop)?;

        let future_ptr = future.clone().unbind();
        let event_loop_ptr = event_loop.unbind();

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
                    *arc.write() = false;
                }

                // 1. Send Close command (if tx exists)
                if let Some(tx) = tx_option {
                    let _ = tx.send(Command::Close).await;
                }

                // 2. Wait for actor to close with 10s timeout (if rx exists)
                if let Some(rx_arc) = rx_arc_option {
                    let _ = tokio::time::timeout(Duration::from_secs(10), async {
                        let mut rx = rx_arc.lock().await;
                        while let Some(msg) = rx.recv().await {
                            match msg {
                                Ok(Message::Close(_)) => break,
                                Ok(_) => continue,
                                Err(_) => break,
                            }
                        }
                    }).await;
                }

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let event_loop = event_loop_ptr.bind(py);
                    if let Err(e) = complete_future(py, event_loop, future, py.None()) {
                        eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                    }
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
                let future = ready_ok(py, py.None())?;
                Ok(future)
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                let event_loop = get_cached_event_loop(py, &self.event_loop)?;
                let future = create_future(py, &event_loop)?;

                let future_ptr = future.clone().unbind();
                let event_loop_ptr = event_loop.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if res.is_ok() {
                                if let Err(e) = complete_future(py, event_loop, future, py.None()) {
                                    eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                                }
                            } else {
                                if let Err(e) = fail_future(
                                    py,
                                    event_loop,
                                    future,
                                    PyRuntimeError::new_err("Failed to send ping"),
                                ) {
                                    eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                                }
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
                let future = ready_ok(py, py.None())?;
                Ok(future)
            }
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                let event_loop = get_cached_event_loop(py, &self.event_loop)?;
                let future = create_future(py, &event_loop)?;

                let future_ptr = future.clone().unbind();
                let event_loop_ptr = event_loop.unbind();

                py.detach(|| {
                    pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                        let res = tx_cloned.send(cmd).await;

                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if res.is_ok() {
                                if let Err(e) = complete_future(py, event_loop, future, py.None()) {
                                    eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                                }
                            } else {
                                if let Err(e) = fail_future(
                                    py,
                                    event_loop,
                                    future,
                                    PyRuntimeError::new_err("Failed to send pong"),
                                ) {
                                    eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                                }
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
        *self.stream_sync.read()
    }

    #[getter]
    fn closed(&self) -> bool {
        !*self.stream_sync.read()
    }

    #[getter]
    fn local_address(&self) -> Option<(String, u16)> {
        self.local_addr.read().as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.read().as_ref().and_then(|s| {
            s.rsplit_once(':')
                .and_then(|(ip, port)| port.parse().ok().map(|p| (ip.to_string(), p)))
        })
    }

    #[getter]
    fn close_code(&self) -> Option<u16> {
        *self.close_code.read()
    }

    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.close_reason.read().clone()
    }

    #[getter]
    fn subprotocol(&self) -> Option<String> {
        self.subprotocol.read().clone()
    }

    /// Async context manager - enter
    fn __aenter__<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (url, connect_timeout, _stream_sync, local_addr, remote_addr, event_loop_cache) = {
            let ws = slf.bind(py).borrow();
            (
                ws.url.clone(),
                ws.connect_timeout,
                ws.stream_sync.clone(),
                ws.local_addr.clone(),
                ws.remote_addr.clone(),
                ws.event_loop.clone(),
            )
        };

        let asyncio = get_asyncio(py)?;
        let event_loop = asyncio.call_method0("get_running_loop")?;

        // Cache event loop for this connection
        *event_loop_cache.write() = Some(event_loop.clone().unbind());

        let future = create_future(py, &event_loop)?;

        let future_ptr = future.clone().unbind();
        let event_loop_ptr = event_loop.unbind();
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
                                    *local_addr.write() = Some(addr.to_string());
                                }
                                if let Ok(addr) = s.peer_addr() {
                                    *remote_addr.write() = Some(addr.to_string());
                                }
                            }
                            MaybeTlsStream::NativeTls(s) => {
                                if let Ok(addr) = s.get_ref().get_ref().get_ref().local_addr() {
                                    *local_addr.write() = Some(addr.to_string());
                                }
                                if let Ok(addr) = s.get_ref().get_ref().get_ref().peer_addr() {
                                    *remote_addr.write() = Some(addr.to_string());
                                }
                            }
                            _ => {}
                        }

                        // Create channels with optimal buffer size
                        let (tx_cmd_val, mut rx_cmd) = mpsc::channel::<Command>(64);
                        let (tx_msg, rx_msg_val) = mpsc::channel::<Result<Message, String>>(64);

                        // Update the fields on the AsyncClientConnection instance
                        Python::attach(|py| {
                            let mut ws_mut = slf_ptr.bind(py).borrow_mut();
                            ws_mut.tx_cmd = Some(tx_cmd_val);
                            ws_mut.rx_msg_internal = Some(Arc::new(AsyncMutex::new(rx_msg_val)));
                            *ws_mut.stream_sync.write() = true;
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
                                                    Command::Text(t) => { let _ = sink.send(Message::Text(Utf8Bytes::from(t))).await; }
                                                    Command::Binary(b) => { let _ = sink.send(Message::Binary(Bytes::from(b))).await; }
                                                    Command::Ping(d) => { let _ = sink.send(Message::Ping(Bytes::from(d))).await; }
                                                    Command::Pong(d) => { let _ = sink.send(Message::Pong(Bytes::from(d))).await; }
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
                            let event_loop = event_loop_ptr.bind(py);
                            if let Err(e) = complete_future(py, event_loop, future, slf_ptr.into_any()) {
                                eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if let Err(err) = fail_future(py, event_loop, future, PyConnectionError::new_err(e.to_string())) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", err);
                            }
                        });
                    },
                    Err(_) => {
                        Python::attach(|py| {
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if let Err(e) = fail_future(py, event_loop, future, PyTimeoutError::new_err(format!("Connection timed out ({} seconds)", connect_timeout))) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                            }
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
        // Clear event loop cache
        *slf.bind(py).borrow().event_loop.write() = None;

        AsyncClientConnection::close(slf, py) // Call close with slf
    }

    /// Async Iterator support - return self
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Async Iterator support - return next message
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let rx = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();


        // Optimistic Recv: Try to receive synchronously first
        if let Ok(mut guard) = rx.try_lock() {
            match guard.try_recv() {
                Ok(msg) => {
                    // Message available! Process immediately
                    let result = process_message(py, msg, &close_code, &close_reason, true);
                    
                    match result {
                        Ok(val) => {
                            // Fast path: Use optimized future creation
                            return ready_fast(py, val);
                        }
                        Err(e) => {
                            // Fast error path: Use optimized error future
                            return ready_fast_err(py, e);
                        }
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // Channel empty, proceed to async wait
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Fast error path: Connection closed
                    return ready_fast_err(
                        py,
                        PyStopAsyncIteration::new_err("Connection closed"),
                    );
                }
            }
        }

        // Slow Path: Async Wait
        let event_loop = get_cached_event_loop(py, &self.event_loop)?;
        let future = create_future(py, &event_loop)?;

        let future_ptr = future.clone().unbind();
        let event_loop_ptr = event_loop.unbind();

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let mut rx = rx.lock().await;

                let msg_result = timeout(Duration::from_secs_f64(receive_timeout), rx.recv()).await;

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let event_loop = event_loop_ptr.bind(py);

                    match msg_result {
                        Ok(Some(msg)) => {
                            let result = process_message(py, msg, &close_code, &close_reason, true);

                            match result {
                                Ok(val) => {
                                    if let Err(e) = complete_future(py, event_loop, future, val) {
                                        eprintln!("CRITICAL: Failed to complete future: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    if let Err(err) = fail_future(py, event_loop, future, e) {
                                        eprintln!("CRITICAL: Failed to set future exception: {:?}", err);
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            if let Err(e) = fail_future(
                                py,
                                event_loop,
                                future,
                                PyStopAsyncIteration::new_err("Connection closed"),
                            ) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                            }
                        }
                        Err(_) => {
                            if let Err(e) = fail_future(
                                py,
                                event_loop,
                                future,
                                PyTimeoutError::new_err(format!(
                                    "Receive timed out ({} seconds)",
                                    receive_timeout
                                )),
                            ) {
                                eprintln!("CRITICAL: Failed to set future exception: {:?}", e);
                            }
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
