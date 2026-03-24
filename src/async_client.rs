use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use pyo3::exceptions::{
    PyConnectionError, PyRuntimeError, PyStopAsyncIteration, PyTimeoutError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::BoundObject;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream};

use crate::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT};

type MessageReceiver = Arc<AsyncMutex<mpsc::Receiver<Result<Message, String>>>>;

#[pyclass]
struct ReadyFuture {
    result: Option<Result<Py<PyAny>, PyErr>>,
}

static STOP_ITERATION: OnceLock<Py<PyAny>> = OnceLock::new();

fn get_stop_iteration(py: Python<'_>) -> &Py<PyAny> {
    STOP_ITERATION.get_or_init(|| {
        py.get_type::<pyo3::exceptions::PyStopIteration>()
            .into_any()
            .unbind()
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
                    let stop_iter = get_stop_iteration(py).bind(py);
                    let err = stop_iter.call1((val,))?;
                    Err(PyErr::from_value(err))
                }
                Err(e) => Err(e),
            }
        } else {
            Err(pyo3::exceptions::PyStopIteration::new_err(()))
        }
    }
}

static ASYNCIO: OnceLock<Py<PyModule>> = OnceLock::new();

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
        Ok(Message::Binary(data)) => Ok(PyBytes::new(py, &data).into_any().unbind()),
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
    let module = py.import(pyo3::intern!(py, "asyncio"))?;
    let module_perm = module.clone().unbind();
    ASYNCIO.set(module_perm).ok();
    Ok(module)
}

#[inline]
fn get_cached_event_loop<'py>(
    py: Python<'py>,
    cache: &Arc<RwLock<Option<Py<PyAny>>>>,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(loop_obj) = cache.read().as_ref() {
        return Ok(loop_obj.bind(py).clone());
    }
    let asyncio = get_asyncio(py)?;
    let event_loop = asyncio.call_method0(pyo3::intern!(py, "get_running_loop"))?;
    *cache.write() = Some(event_loop.clone().unbind());
    Ok(event_loop)
}

fn create_future<'py>(
    py: Python<'py>,
    event_loop: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    event_loop.call_method0(pyo3::intern!(py, "create_future"))
}

fn complete_future<'py>(
    py: Python<'py>,
    event_loop: &Bound<'py, PyAny>,
    future: &Bound<'py, PyAny>,
    result: Py<PyAny>,
) -> PyResult<()> {
    let set_result = future.getattr(pyo3::intern!(py, "set_result"))?;
    event_loop.call_method1(
        pyo3::intern!(py, "call_soon_threadsafe"),
        (set_result, result),
    )?;
    Ok(())
}

fn fail_future<'py>(
    py: Python<'py>,
    event_loop: &Bound<'py, PyAny>,
    future: &Bound<'py, PyAny>,
    exc: PyErr,
) -> PyResult<()> {
    let set_exc = future.getattr(pyo3::intern!(py, "set_exception"))?;
    event_loop.call_method1(pyo3::intern!(py, "call_soon_threadsafe"), (set_exc, exc))?;
    Ok(())
}

fn ready_fast<'py>(py: Python<'py>, result: impl IntoPyObject<'py>) -> PyResult<Bound<'py, PyAny>> {
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

fn ready_fast_err<'py>(py: Python<'py>, err: PyErr) -> PyResult<Bound<'py, PyAny>> {
    let future = Bound::new(
        py,
        ReadyFuture {
            result: Some(Err(err)),
        },
    )?;
    Ok(future.into_any())
}

#[derive(Debug)]
enum Command {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

/// Result of WebSocket connect — carries the stream type without Py<T> ownership issues
enum WsConnectResult {
    Direct(Box<tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>),
    Proxy(
        Box<
            tokio_tungstenite::WebSocketStream<
                tokio_native_tls::TlsStream<tokio_socks::tcp::Socks5Stream<tokio::net::TcpStream>>,
            >,
        >,
    ),
    ProxyPlain(
        Box<
            tokio_tungstenite::WebSocketStream<
                tokio_socks::tcp::Socks5Stream<tokio::net::TcpStream>,
            >,
        >,
    ),
}

// Background Task Handler (Supports both Direct and Proxy Streams)
async fn start_ws_task<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    slf_ptr: Py<AsyncClientConnection>,
    future_ptr: Py<PyAny>,
    event_loop_ptr: Py<PyAny>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (tx_cmd_val, mut rx_cmd) = mpsc::channel::<Command>(64);
    let (tx_msg, rx_msg_val) = mpsc::channel::<Result<Message, String>>(64);

    Python::attach(|py| {
        let mut ws_mut = slf_ptr.bind(py).borrow_mut();
        ws_mut.tx_cmd = Some(tx_cmd_val);
        ws_mut.rx_msg_internal = Some(Arc::new(AsyncMutex::new(rx_msg_val)));
        *ws_mut.stream_sync.write() = true;
    });

    tokio::spawn(async move {
        let (mut sink, mut stream) = ws_stream.split();
        loop {
            tokio::select! {
                cmd = rx_cmd.recv() => {
                    match cmd {
                        Some(cmd) => {
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
                        None => break,
                    }
                }
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                            // Filtered: don't wake recv/anext for control frames
                            continue;
                        }
                        Some(Ok(msg)) => {
                            if tx_msg.send(Ok(msg)).await.is_err() { break; }
                        }
                        Some(Err(e)) => {
                            let _ = tx_msg.send(Err(e.to_string())).await;
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
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

#[pyclass(name = "ClientConnection", module = "websocket_rs.async_client")]
pub struct AsyncClientConnection {
    url: String,
    headers: Option<HashMap<String, String>>,
    proxy: Option<String>,
    tx_cmd: Option<mpsc::Sender<Command>>,
    rx_msg_internal: Option<MessageReceiver>,
    stream_sync: Arc<RwLock<bool>>,
    connect_timeout: f64,
    receive_timeout: f64,
    event_loop: Arc<RwLock<Option<Py<PyAny>>>>,
    local_addr: Arc<RwLock<Option<(String, u16)>>>,
    remote_addr: Arc<RwLock<Option<(String, u16)>>>,
    subprotocol: Arc<RwLock<Option<String>>>,
    close_code: Arc<RwLock<Option<u16>>>,
    close_reason: Arc<RwLock<Option<String>>>,
}

#[pymethods]
impl AsyncClientConnection {
    #[new]
    #[pyo3(signature = (url, *, headers=None, proxy=None, connect_timeout=None, receive_timeout=None))]
    fn new(
        url: String,
        headers: Option<HashMap<String, String>>,
        proxy: Option<String>,
        connect_timeout: Option<f64>,
        receive_timeout: Option<f64>,
    ) -> PyResult<Self> {
        // Validate proxy scheme (only socks5:// supported)
        if let Some(ref p) = proxy {
            let scheme = p.split("://").next().unwrap_or("");
            if scheme != "socks5" {
                return Err(PyValueError::new_err(format!(
                    "Only socks5:// proxy is supported, got: {}://",
                    scheme
                )));
            }
        }

        // Validate headers
        if let Some(ref h) = headers {
            for (k, v) in h {
                HeaderName::from_str(k)
                    .map_err(|_| PyValueError::new_err(format!("Invalid header name: {}", k)))?;
                HeaderValue::from_str(v).map_err(|_| {
                    PyValueError::new_err(format!("Invalid header value for {}", k))
                })?;
            }
        }

        Ok(AsyncClientConnection {
            url,
            headers,
            proxy,
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
        })
    }

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

        let command = if let Ok(s) = message.cast::<pyo3::types::PyString>() {
            Command::Text(s.to_str()?.to_owned())
        } else if let Ok(b) = message.cast::<PyBytes>() {
            Command::Binary(b.as_bytes().to_vec())
        } else {
            return Err(PyRuntimeError::new_err("Message must be str or bytes"));
        };

        match tx_cloned.try_send(command) {
            Ok(_) => ready_fast(py, py.None()),
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
                                let _ = complete_future(py, event_loop, future, py.None());
                            } else {
                                let _ = fail_future(
                                    py,
                                    event_loop,
                                    future,
                                    PyRuntimeError::new_err("Failed to send message"),
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

    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let rx = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;

        // Fast path: try_lock + try_recv avoids spawning a tokio task
        if let Ok(mut guard) = rx.try_lock() {
            match guard.try_recv() {
                Ok(msg) => {
                    let result =
                        process_message(py, msg, &self.close_code, &self.close_reason, false);
                    match result {
                        Ok(val) => return ready_fast(py, val),
                        Err(e) => return ready_fast_err(py, e),
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return ready_fast_err(py, PyRuntimeError::new_err("Connection closed"));
                }
            }
        }

        // Slow path: clone Arcs only when we need to move them into the spawned task
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();
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
                            let result =
                                process_message(py, msg, &close_code, &close_reason, false);
                            match result {
                                Ok(val) => {
                                    let _ = complete_future(py, event_loop, future, val);
                                }
                                Err(e) => {
                                    let _ = fail_future(py, event_loop, future, e);
                                }
                            }
                        }
                        Ok(None) => {
                            let _ = fail_future(
                                py,
                                event_loop,
                                future,
                                PyRuntimeError::new_err("Connection closed"),
                            );
                        }
                        Err(_) => {
                            let _ = fail_future(
                                py,
                                event_loop,
                                future,
                                PyTimeoutError::new_err("Receive timed out"),
                            );
                        }
                    }
                });
            });
        });

        Ok(future)
    }

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

                Python::attach(|py| {
                    let mut ws_mut = slf.bind(py).borrow_mut();
                    tx_option = ws_mut.tx_cmd.take();
                    rx_arc_option = ws_mut.rx_msg_internal.take();
                    stream_sync_arc = Some(ws_mut.stream_sync.clone());
                });

                if let Some(arc) = stream_sync_arc {
                    *arc.write() = false;
                }
                if let Some(tx) = tx_option {
                    let _ = tx.send(Command::Close).await;
                }

                if let Some(rx_arc) = rx_arc_option {
                    let _ = tokio::time::timeout(Duration::from_secs(10), async {
                        let mut rx = rx_arc.lock().await;
                        while let Some(msg) = rx.recv().await {
                            match msg {
                                Ok(Message::Close(_)) | Err(_) => break,
                                Ok(_) => continue,
                            }
                        }
                    })
                    .await;
                }

                Python::attach(|py| {
                    let future = future_ptr.bind(py);
                    let event_loop = event_loop_ptr.bind(py);
                    let _ = complete_future(py, event_loop, future, py.None());
                });
            });
        });

        Ok(future)
    }

    fn ping<'py>(&self, py: Python<'py>, data: Option<Vec<u8>>) -> PyResult<Bound<'py, PyAny>> {
        let tx_cloned = self
            .tx_cmd
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let data = data.unwrap_or_default();
        match tx_cloned.try_send(Command::Ping(data)) {
            Ok(_) => ready_fast(py, py.None()),
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
                                let _ = complete_future(py, event_loop, future, py.None());
                            } else {
                                let _ = fail_future(
                                    py,
                                    event_loop,
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

    fn pong<'py>(&self, py: Python<'py>, data: Option<Vec<u8>>) -> PyResult<Bound<'py, PyAny>> {
        let tx_cloned = self
            .tx_cmd
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let data = data.unwrap_or_default();
        match tx_cloned.try_send(Command::Pong(data)) {
            Ok(_) => ready_fast(py, py.None()),
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
                                let _ = complete_future(py, event_loop, future, py.None());
                            } else {
                                let _ = fail_future(
                                    py,
                                    event_loop,
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
        self.local_addr.read().clone()
    }
    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.read().clone()
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

    fn __aenter__<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // Extract all values while holding the GIL — nothing crosses the await boundary
        let (
            url,
            headers_opt,
            proxy_opt,
            connect_timeout,
            local_addr,
            remote_addr,
            event_loop_cache,
        ) = {
            let ws = slf.bind(py).borrow();
            (
                ws.url.clone(),
                ws.headers.clone(),
                ws.proxy.clone(),
                ws.connect_timeout,
                ws.local_addr.clone(),
                ws.remote_addr.clone(),
                ws.event_loop.clone(),
            )
        };

        let asyncio = get_asyncio(py)?;
        let event_loop = asyncio.call_method0(pyo3::intern!(py, "get_running_loop"))?;
        *event_loop_cache.write() = Some(event_loop.clone().unbind());

        let future = create_future(py, &event_loop)?;
        let future_ptr = future.clone().unbind();
        let event_loop_ptr = event_loop.unbind();
        let slf_ptr = slf.clone_ref(py);

        py.detach(|| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                let connect_fut = async {
                    let mut request = url
                        .clone()
                        .into_client_request()
                        .map_err(|e| e.to_string())?;

                    // Inject custom headers (already validated in new())
                    if let Some(ref headers) = headers_opt {
                        for (k, v) in headers {
                            if let (Ok(k_hdr), Ok(v_hdr)) =
                                (HeaderName::from_str(k), HeaderValue::from_str(v))
                            {
                                request.headers_mut().insert(k_hdr, v_hdr);
                            }
                        }
                    }

                    if let Some(ref proxy_str) = proxy_opt {
                        // ── SOCKS5 proxy path ──
                        let target_url = url::Url::parse(&url).map_err(|e| e.to_string())?;
                        let host = target_url
                            .host_str()
                            .ok_or("Invalid target host")?
                            .to_string();
                        let port = target_url
                            .port_or_known_default()
                            .ok_or("Invalid target port")?;

                        let proxy_url = url::Url::parse(proxy_str).map_err(|e| e.to_string())?;
                        let proxy_host = proxy_url
                            .host_str()
                            .ok_or("Invalid proxy host")?
                            .to_string();
                        let proxy_port = proxy_url
                            .port_or_known_default()
                            .ok_or("Invalid proxy port")?;

                        let socks_stream = tokio_socks::tcp::Socks5Stream::connect(
                            (proxy_host.as_str(), proxy_port),
                            (host.as_str(), port),
                        )
                        .await
                        .map_err(|e| format!("SOCKS5 proxy connection failed: {}", e))?;

                        if target_url.scheme() == "wss" {
                            let cx = native_tls::TlsConnector::builder()
                                .build()
                                .map_err(|e| format!("TLS connector build failed: {}", e))?;
                            let cx = tokio_native_tls::TlsConnector::from(cx);
                            let tls_stream =
                                cx.connect(&host, socks_stream).await.map_err(|e| {
                                    format!("TLS handshake through proxy failed: {}", e)
                                })?;

                            let (ws_stream, _) =
                                tokio_tungstenite::client_async(request, tls_stream)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            return Ok(WsConnectResult::Proxy(Box::new(ws_stream)));
                        } else {
                            let (ws_stream, _) =
                                tokio_tungstenite::client_async(request, socks_stream)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            return Ok(WsConnectResult::ProxyPlain(Box::new(ws_stream)));
                        }
                    }

                    // ── Direct connection path ──
                    let (ws_stream, _) = connect_async(request).await.map_err(|e| e.to_string())?;

                    // Extract addr info from direct connection
                    match ws_stream.get_ref() {
                        MaybeTlsStream::Plain(s) => {
                            if let Ok(addr) = s.local_addr() {
                                *local_addr.write() = Some((addr.ip().to_string(), addr.port()));
                            }
                            if let Ok(addr) = s.peer_addr() {
                                *remote_addr.write() = Some((addr.ip().to_string(), addr.port()));
                            }
                        }
                        MaybeTlsStream::NativeTls(s) => {
                            if let Ok(addr) = s.get_ref().get_ref().get_ref().local_addr() {
                                *local_addr.write() = Some((addr.ip().to_string(), addr.port()));
                            }
                            if let Ok(addr) = s.get_ref().get_ref().get_ref().peer_addr() {
                                *remote_addr.write() = Some((addr.ip().to_string(), addr.port()));
                            }
                        }
                        _ => {}
                    }
                    Ok::<WsConnectResult, String>(WsConnectResult::Direct(Box::new(ws_stream)))
                };

                // Timeout wraps the entire connect future
                match timeout(Duration::from_secs_f64(connect_timeout), connect_fut).await {
                    Ok(Ok(ws_result)) => {
                        // Success: start background task with the stream
                        match ws_result {
                            WsConnectResult::Direct(ws_stream) => {
                                start_ws_task(*ws_stream, slf_ptr, future_ptr, event_loop_ptr)
                                    .await;
                            }
                            WsConnectResult::Proxy(ws_stream) => {
                                start_ws_task(*ws_stream, slf_ptr, future_ptr, event_loop_ptr)
                                    .await;
                            }
                            WsConnectResult::ProxyPlain(ws_stream) => {
                                start_ws_task(*ws_stream, slf_ptr, future_ptr, event_loop_ptr)
                                    .await;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        // Connection error — drop Py<T> safely under GIL
                        Python::attach(|py| {
                            drop(slf_ptr);
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if let Err(e) =
                                fail_future(py, event_loop, future, PyConnectionError::new_err(e))
                            {
                                eprintln!("Failed to set connection error on future: {:?}", e);
                            }
                        });
                    }
                    Err(_) => {
                        // Timeout — drop Py<T> safely under GIL
                        Python::attach(|py| {
                            drop(slf_ptr);
                            let future = future_ptr.bind(py);
                            let event_loop = event_loop_ptr.bind(py);
                            if let Err(e) = fail_future(
                                py,
                                event_loop,
                                future,
                                PyTimeoutError::new_err(format!(
                                    "Connection timed out ({:.1}s)",
                                    connect_timeout
                                )),
                            ) {
                                eprintln!("Failed to set timeout error on future: {:?}", e);
                            }
                        });
                    }
                }
            });
        });

        Ok(future)
    }

    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __aexit__<'py>(
        slf: Py<Self>,
        py: Python<'py>,
        _exc_type: Option<&Bound<'py, PyAny>>,
        _exc_value: Option<&Bound<'py, PyAny>>,
        _traceback: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        *slf.bind(py).borrow().event_loop.write() = None;
        AsyncClientConnection::close(slf, py)
    }

    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let rx = self
            .rx_msg_internal
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?
            .clone();
        let receive_timeout = self.receive_timeout;

        // Fast path: borrow from self, no Arc clone
        if let Ok(mut guard) = rx.try_lock() {
            match guard.try_recv() {
                Ok(msg) => {
                    let result =
                        process_message(py, msg, &self.close_code, &self.close_reason, true);
                    match result {
                        Ok(val) => return ready_fast(py, val),
                        Err(e) => return ready_fast_err(py, e),
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return ready_fast_err(py, PyStopAsyncIteration::new_err("Connection closed"))
                }
            }
        }

        // Slow path: clone only when moving into spawned task
        let close_code = self.close_code.clone();
        let close_reason = self.close_reason.clone();
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
                                    let _ = complete_future(py, event_loop, future, val);
                                }
                                Err(e) => {
                                    let _ = fail_future(py, event_loop, future, e);
                                }
                            }
                        }
                        Ok(None) => {
                            let _ = fail_future(
                                py,
                                event_loop,
                                future,
                                PyStopAsyncIteration::new_err("Connection closed"),
                            );
                        }
                        Err(_) => {
                            let _ = fail_future(
                                py,
                                event_loop,
                                future,
                                PyTimeoutError::new_err("Receive timed out"),
                            );
                        }
                    }
                });
            });
        });

        Ok(future)
    }
}

#[pyfunction]
#[pyo3(signature = (uri, *, headers=None, proxy=None, **_kwargs))]
pub fn connect<'py>(
    py: Python<'py>,
    uri: String,
    headers: Option<HashMap<String, String>>,
    proxy: Option<String>,
    _kwargs: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let ws = AsyncClientConnection::new(uri, headers, proxy, None, None)?;
    let ws_cell = Py::new(py, ws)?;
    AsyncClientConnection::__aenter__(ws_cell, py)
}

pub fn register_async_client(py: Python<'_>, parent_module: &Bound<'_, PyModule>) -> PyResult<()> {
    let async_client_module = PyModule::new(py, "async_client")?;
    async_client_module.add_class::<AsyncClientConnection>()?;
    async_client_module.add_function(wrap_pyfunction!(connect, &async_client_module)?)?;
    parent_module.add_submodule(&async_client_module)?;
    Ok(())
}
