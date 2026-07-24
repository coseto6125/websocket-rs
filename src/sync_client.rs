use bytes::Bytes;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;
use tungstenite::WebSocket;

use crate::{
    DEFAULT_CLOSE_TIMEOUT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT, DEFAULT_TCP_NODELAY,
};

/// Cached rustls ClientConfig — built lazily on first TLS connect. Loads
/// the system trust store via rustls-native-certs, then additionally
/// trusts certs from `SSL_CERT_FILE` env var if set (matches OpenSSL's
/// behavior — native-tls honored this; users with private CAs / self-
/// signed certs in dev environments expect the env var to work).
pub(crate) fn build_rustls_client_config() -> PyResult<std::sync::Arc<rustls::ClientConfig>> {
    use std::sync::OnceLock;
    static CONFIG: OnceLock<std::sync::Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(c) = CONFIG.get() {
        return Ok(c.clone());
    }
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    if !native.errors.is_empty() {
        return Err(PyConnectionError::new_err(format!(
            "Failed to load native certs: {:?}",
            native.errors
        )));
    }
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    if let Ok(path) = std::env::var("SSL_CERT_FILE") {
        let pem = std::fs::read(&path).map_err(|e| {
            PyConnectionError::new_err(format!("SSL_CERT_FILE {} unreadable: {}", path, e))
        })?;
        for cert in rustls_pemfile::certs(&mut pem.as_slice()).flatten() {
            let _ = roots.add(cert);
        }
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let arc = std::sync::Arc::new(config);
    let _ = CONFIG.set(arc.clone());
    Ok(arc)
}

enum RecvResult {
    Text(String),
    Binary(Bytes),
}

mod send_pybytes_owner {
    use super::*;

    struct SendPyBytesOwner {
        _bytes: Py<PyBytes>,
        ptr: *const u8,
        len: usize,
    }

    // SAFETY: this owner is constructed only from an exact CPython `bytes`, whose
    // buffer is immutable and pointer-stable. `_bytes` is an owned reference cloned
    // while the GIL is held before `py.detach`; it keeps the allocation alive until
    // tungstenite drops the owner-backed `Bytes`. PyO3 permits `Py<PyBytes>` to cross
    // threads and defers its decref until the GIL can be acquired.
    unsafe impl Send for SendPyBytesOwner {}
    unsafe impl Sync for SendPyBytesOwner {}

    impl AsRef<[u8]> for SendPyBytesOwner {
        fn as_ref(&self) -> &[u8] {
            // SAFETY: `ptr` and `len` describe the immutable buffer owned by
            // `_bytes`, which remains alive for this owner's entire lifetime.
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    pub(super) fn new(bytes: &Bound<'_, PyBytes>) -> Bytes {
        let data = bytes.as_bytes();
        Bytes::from_owner(SendPyBytesOwner {
            _bytes: bytes.clone().unbind(),
            ptr: data.as_ptr(),
            len: data.len(),
        })
    }
}

/// Keeps EINTR handling below rustls, whose internal I/O loop otherwise retries
/// interrupted socket reads without giving Python a chance to raise a signal.
struct SignalAwareTcpStream {
    stream: TcpStream,
    read_deadline: Option<Instant>,
    enforce_read_deadline: bool,
}

impl SignalAwareTcpStream {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_deadline: None,
            enforce_read_deadline: false,
        }
    }

    fn apply_remaining_read_timeout(&self) -> io::Result<()> {
        let remaining = self
            .read_deadline
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "read deadline elapsed"))?;
        self.stream.set_read_timeout(Some(remaining))
    }
}

impl Read for SignalAwareTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.enforce_read_deadline {
                self.apply_remaining_read_timeout()?;
            }
            match self.stream.read(buf) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    check_python_signals()?;
                    self.enforce_read_deadline = true;
                }
                result => return result,
            }
        }
    }
}

impl Write for SignalAwareTcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        retry_interrupted(|| self.stream.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        retry_interrupted(|| self.stream.flush())
    }
}

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, SignalAwareTcpStream>;

/// Type-erased stream for WebSocket (avoids generic type in pyclass)
enum WsStream {
    Plain(SignalAwareTcpStream),
    Tls(Box<TlsStream>),
}

impl Read for WsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for WsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

impl WsStream {
    fn plain(stream: TcpStream) -> Self {
        Self::Plain(SignalAwareTcpStream::new(stream))
    }

    fn tls(connection: rustls::ClientConnection, stream: TcpStream) -> Self {
        Self::Tls(Box::new(TlsStream::new(
            connection,
            SignalAwareTcpStream::new(stream),
        )))
    }

    fn begin_read_deadline(&mut self, timeout: Duration) {
        let socket = self.socket_mut();
        socket.read_deadline = Some(Instant::now() + timeout);
        socket.enforce_read_deadline = false;
    }

    fn clear_read_deadline(&mut self, timeout: Duration) {
        let socket = self.socket_mut();
        if socket.enforce_read_deadline {
            let _ = socket.stream.set_read_timeout(Some(timeout));
        }
        socket.read_deadline = None;
        socket.enforce_read_deadline = false;
    }

    fn tcp_ref(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => &s.stream,
            Self::Tls(s) => &s.get_ref().stream,
        }
    }

    fn socket_mut(&mut self) -> &mut SignalAwareTcpStream {
        match self {
            Self::Plain(s) => s,
            Self::Tls(s) => s.get_mut(),
        }
    }
}

fn check_python_signals() -> io::Result<()> {
    // PEP 475 retries EINTR only after giving Python a chance to raise a
    // pending signal exception such as KeyboardInterrupt.
    Python::attach(|py| py.check_signals()).map_err(io::Error::from)
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => check_python_signals()?,
            result => return result,
        }
    }
}

fn is_python_error(error: &io::Error) -> bool {
    // PyO3 preserves a PyErr wrapped in io::Error, allowing the API boundary
    // to recover the original Python exception instead of replacing its type.
    error.get_ref().is_some_and(|inner| inner.is::<PyErr>())
}

fn map_websocket_error(context: &str, error: tungstenite::Error) -> PyErr {
    match error {
        tungstenite::Error::Io(error) if is_python_error(&error) => error.into(),
        error => PyRuntimeError::new_err(format!("{context} failed: {error}")),
    }
}

fn map_receive_error(error: tungstenite::Error, receive_timeout: f64) -> PyErr {
    match error {
        tungstenite::Error::Io(error) if is_python_error(&error) => error.into(),
        tungstenite::Error::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            PyTimeoutError::new_err(format!("Receive timed out ({receive_timeout} seconds)"))
        }
        error => PyRuntimeError::new_err(format!("Receive failed: {error}")),
    }
}

/// Sync client connection (pure sync, no async runtime overhead)
#[pyclass(name = "ClientConnection", module = "websocket_rs.sync.client")]
pub struct SyncClientConnection {
    url: String,
    ws: Option<WebSocket<WsStream>>,
    connect_timeout: f64,
    receive_timeout: f64,
    close_timeout: f64,
    tcp_nodelay: bool,
    local_addr: Option<(String, u16)>,
    remote_addr: Option<(String, u16)>,
    close_code: Option<u16>,
    close_reason: Option<String>,
}

#[pymethods]
impl SyncClientConnection {
    #[new]
    #[pyo3(signature = (url, connect_timeout=None, receive_timeout=None, close_timeout=None, tcp_nodelay=None))]
    fn new(
        url: String,
        connect_timeout: Option<f64>,
        receive_timeout: Option<f64>,
        close_timeout: Option<f64>,
        tcp_nodelay: Option<bool>,
    ) -> Self {
        SyncClientConnection {
            url,
            ws: None,
            connect_timeout: connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            receive_timeout: receive_timeout.unwrap_or(DEFAULT_RECEIVE_TIMEOUT),
            close_timeout: close_timeout.unwrap_or(DEFAULT_CLOSE_TIMEOUT),
            tcp_nodelay: tcp_nodelay.unwrap_or(DEFAULT_TCP_NODELAY),
            local_addr: None,
            remote_addr: None,
            close_code: None,
            close_reason: None,
        }
    }

    /// Internal connect implementation
    // SAFETY: py.detach() releases the GIL while this closure runs with &mut self.
    // This is safe because PyO3's PyRefMut guarantees exclusive borrow — no other
    // Python thread can access this object while we hold the mutable reference.
    fn __connect(&mut self, py: Python<'_>) -> PyResult<()> {
        let url = self.url.clone();
        let connect_timeout = self.connect_timeout;
        let receive_timeout = self.receive_timeout;
        let tcp_nodelay = self.tcp_nodelay;

        py.detach(|| {
            // Parse URL and resolve address
            let request = url
                .clone()
                .into_client_request()
                .map_err(|e| PyConnectionError::new_err(format!("Invalid URL: {}", e)))?;
            let uri = request.uri().clone();
            let host = uri
                .host()
                .ok_or_else(|| PyConnectionError::new_err("Missing host in URL"))?
                .to_string();
            let is_tls = uri.scheme_str() == Some("wss");
            let port = uri.port_u16().unwrap_or(if is_tls { 443 } else { 80 });
            let addr = (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|e| PyConnectionError::new_err(format!("DNS resolution failed: {}", e)))?
                .next()
                .ok_or_else(|| PyConnectionError::new_err("No address resolved"))?;

            // TCP connect with timeout
            let connect_dur = Duration::from_secs_f64(connect_timeout);
            let tcp = TcpStream::connect_timeout(&addr, connect_dur).map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    PyTimeoutError::new_err(format!(
                        "Connection timed out ({:.1}s)",
                        connect_timeout
                    ))
                } else {
                    PyConnectionError::new_err(format!("Connection failed: {}", e))
                }
            })?;
            let _ = tcp.set_nodelay(tcp_nodelay);

            // Set temp timeout to bound TLS + WS handshake
            let _ = tcp.set_read_timeout(Some(connect_dur));
            let _ = tcp.set_write_timeout(Some(connect_dur));

            // Build stream (plain or TLS) and do WS handshake. TLS via
            // rustls (pure Rust, no OpenSSL — avoids global-state conflict
            // with Python's _ssl when both libraries co-exist in process).
            let ws_stream = if is_tls {
                let config = build_rustls_client_config()?;
                let server_name =
                    rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| {
                        PyConnectionError::new_err(format!("Invalid server name: {}", e))
                    })?;
                let conn = rustls::ClientConnection::new(config, server_name)
                    .map_err(|e| PyConnectionError::new_err(format!("TLS init failed: {}", e)))?;
                WsStream::tls(conn, tcp)
            } else {
                WsStream::plain(tcp)
            };

            let (ws, _) = tungstenite::client(request, ws_stream).map_err(|error| match error {
                tungstenite::HandshakeError::Failure(tungstenite::Error::Io(error))
                    if is_python_error(&error) =>
                {
                    error.into()
                }
                error => PyConnectionError::new_err(format!("WebSocket handshake failed: {error}")),
            })?;

            // Reset timeouts: read = receive_timeout, write = unlimited
            let tcp_ref = ws.get_ref().tcp_ref();
            tcp_ref
                .set_read_timeout(Some(Duration::from_secs_f64(receive_timeout)))
                .map_err(|e| PyRuntimeError::new_err(format!("Set timeout failed: {}", e)))?;
            let _ = tcp_ref.set_write_timeout(None);

            if let Ok(a) = tcp_ref.local_addr() {
                self.local_addr = Some((a.ip().to_string(), a.port()));
            }
            if let Ok(a) = tcp_ref.peer_addr() {
                self.remote_addr = Some((a.ip().to_string(), a.port()));
            }

            self.ws = Some(ws);
            Ok(())
        })
    }

    fn send<'py>(&mut self, py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<()> {
        let msg = if let Ok(s) = message.cast::<PyString>() {
            Message::Text(s.to_str()?.into())
        } else if let Ok(bytes) = message.cast_exact::<PyBytes>() {
            Message::Binary(send_pybytes_owner::new(bytes))
        } else if let Ok(bytes) = message.extract::<Vec<u8>>() {
            Message::Binary(bytes.into())
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };

        py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.send(msg)
                .map_err(|error| map_websocket_error("Send", error))
        })
    }

    fn recv(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let receive_timeout = self.receive_timeout;
        let result = py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.get_mut()
                .begin_read_deadline(Duration::from_secs_f64(receive_timeout));
            let mut close_details = None;
            let result = loop {
                let msg = match ws.read() {
                    Ok(msg) => msg,
                    Err(error) => break Err(map_receive_error(error, receive_timeout)),
                };

                match msg {
                    Message::Text(text) => break Ok(RecvResult::Text(text.to_string())),
                    Message::Binary(data) => break Ok(RecvResult::Binary(data)),
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(frame) => {
                        if let Some(f) = frame {
                            close_details = Some((f.code.into(), f.reason.to_string()));
                        }
                        break Err(PyRuntimeError::new_err("Connection closed by server"));
                    }
                    _ => {
                        break Err(PyRuntimeError::new_err("Received unsupported message type"));
                    }
                }
            };
            ws.get_mut()
                .clear_read_deadline(Duration::from_secs_f64(receive_timeout));
            if let Some((code, reason)) = close_details {
                self.close_code = Some(code);
                self.close_reason = Some(reason);
            }
            result
        })?;

        match result {
            RecvResult::Text(s) => Ok(PyString::new(py, &s).into_any().unbind()),
            RecvResult::Binary(b) => Ok(PyBytes::new(py, b.as_ref()).into_any().unbind()),
        }
    }

    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        let close_timeout = self.close_timeout;
        py.detach(|| {
            if let Some(mut ws) = self.ws.take() {
                // Set a timeout for the close handshake
                let close_duration = Duration::from_secs_f64(close_timeout);
                let _ = ws
                    .get_ref()
                    .tcp_ref()
                    .set_read_timeout(Some(close_duration));
                ws.get_mut().begin_read_deadline(close_duration);
                match ws.close(None) {
                    Err(tungstenite::Error::Io(error)) if is_python_error(&error) => {
                        return Err(error.into());
                    }
                    Err(error) if !matches!(error, tungstenite::Error::ConnectionClosed) => {
                        // Read remaining frames until close confirmation or timeout
                        loop {
                            match ws.read() {
                                Ok(Message::Close(_)) => break,
                                Err(tungstenite::Error::Io(error)) if is_python_error(&error) => {
                                    return Err(error.into());
                                }
                                Err(_) => break,
                                _ => continue,
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }

    fn ping(&mut self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let data = data.unwrap_or_default();
        py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;
            ws.send(Message::Ping(data.into()))
                .map_err(|error| map_websocket_error("Ping", error))
        })
    }

    fn pong(&mut self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let data = data.unwrap_or_default();
        py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;
            ws.send(Message::Pong(data.into()))
                .map_err(|error| map_websocket_error("Pong", error))
        })
    }

    #[getter]
    fn open(&self) -> bool {
        self.ws.is_some()
    }
    #[getter]
    fn closed(&self) -> bool {
        self.ws.is_none()
    }
    #[getter]
    fn local_address(&self) -> Option<(String, u16)> {
        self.local_addr.clone()
    }
    #[getter]
    fn remote_address(&self) -> Option<(String, u16)> {
        self.remote_addr.clone()
    }
    #[getter]
    fn close_code(&self) -> Option<u16> {
        self.close_code
    }
    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.close_reason.clone()
    }

    fn __enter__<'py>(
        mut slf: PyRefMut<'py, Self>,
        py: Python<'py>,
    ) -> PyResult<PyRefMut<'py, Self>> {
        slf.__connect(py)?;
        Ok(slf)
    }

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

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

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

#[pyfunction]
#[pyo3(signature = (uri, connect_timeout=None, receive_timeout=None, close_timeout=None, tcp_nodelay=None, **_kwargs))]
pub fn connect(
    uri: String,
    connect_timeout: Option<f64>,
    receive_timeout: Option<f64>,
    close_timeout: Option<f64>,
    tcp_nodelay: Option<bool>,
    _kwargs: Option<&Bound<'_, PyAny>>,
) -> PyResult<SyncClientConnection> {
    Ok(SyncClientConnection::new(
        uri,
        connect_timeout,
        receive_timeout,
        close_timeout,
        tcp_nodelay,
    ))
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
