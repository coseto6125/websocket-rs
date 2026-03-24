use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;
use tungstenite::WebSocket;

use crate::{
    DEFAULT_CLOSE_TIMEOUT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_RECEIVE_TIMEOUT, DEFAULT_TCP_NODELAY,
};

enum RecvResult {
    Text(String),
    Binary(Vec<u8>),
}

/// Type-erased stream for WebSocket (avoids generic type in pyclass)
enum WsStream {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}

impl Read for WsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            WsStream::Plain(s) => s.read(buf),
            WsStream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for WsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            WsStream::Plain(s) => s.write(buf),
            WsStream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            WsStream::Plain(s) => s.flush(),
            WsStream::Tls(s) => s.flush(),
        }
    }
}

impl WsStream {
    fn tcp_ref(&self) -> &TcpStream {
        match self {
            WsStream::Plain(s) => s,
            WsStream::Tls(s) => s.get_ref(),
        }
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

            // Build stream (plain or TLS) and do WS handshake
            let ws_stream = if is_tls {
                let connector = native_tls::TlsConnector::builder()
                    .build()
                    .map_err(|e| PyConnectionError::new_err(format!("TLS build failed: {}", e)))?;
                let tls = connector.connect(&host, tcp).map_err(|e| {
                    PyConnectionError::new_err(format!("TLS handshake failed: {}", e))
                })?;
                WsStream::Tls(Box::new(tls))
            } else {
                WsStream::Plain(tcp)
            };

            let (ws, _) = tungstenite::client(request, ws_stream).map_err(|e| {
                PyConnectionError::new_err(format!("WebSocket handshake failed: {}", e))
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
            Message::Text(s.to_str()?.to_owned())
        } else if let Ok(bytes) = message.extract::<Vec<u8>>() {
            Message::Binary(bytes)
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };

        py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            ws.send(msg)
                .map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))
        })
    }

    fn recv(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let result = py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;

            loop {
                let msg = ws.read().map_err(|e| match &e {
                    tungstenite::Error::Io(io_err)
                        if matches!(
                            io_err.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        PyTimeoutError::new_err(format!(
                            "Receive timed out ({} seconds)",
                            self.receive_timeout
                        ))
                    }
                    _ => PyRuntimeError::new_err(format!("Receive failed: {}", e)),
                })?;

                match msg {
                    Message::Text(text) => return Ok(RecvResult::Text(text)),
                    Message::Binary(data) => return Ok(RecvResult::Binary(data)),
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(frame) => {
                        if let Some(f) = frame {
                            self.close_code = Some(f.code.into());
                            self.close_reason = Some(f.reason.to_string());
                        }
                        return Err(PyRuntimeError::new_err("Connection closed by server"));
                    }
                    _ => return Err(PyRuntimeError::new_err("Received unsupported message type")),
                }
            }
        })?;

        match result {
            RecvResult::Text(s) => Ok(PyString::new(py, &s).into_any().unbind()),
            RecvResult::Binary(b) => Ok(PyBytes::new(py, &b).into_any().unbind()),
        }
    }

    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        let close_timeout = self.close_timeout;
        py.detach(|| {
            if let Some(mut ws) = self.ws.take() {
                // Set a timeout for the close handshake
                let tcp_ref = ws.get_ref().tcp_ref();
                let _ = tcp_ref.set_read_timeout(Some(Duration::from_secs_f64(close_timeout)));
                if let Err(e) = ws.close(None) {
                    // Read remaining frames until close confirmation or timeout
                    if !matches!(e, tungstenite::Error::ConnectionClosed) {
                        loop {
                            match ws.read() {
                                Ok(Message::Close(_)) | Err(_) => break,
                                _ => continue,
                            }
                        }
                    }
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
            ws.send(Message::Ping(data))
                .map_err(|e| PyRuntimeError::new_err(format!("Ping failed: {}", e)))
        })
    }

    fn pong(&mut self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let data = data.unwrap_or_default();
        py.detach(|| {
            let ws = self
                .ws
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket is not connected"))?;
            ws.send(Message::Pong(data))
                .map_err(|e| PyRuntimeError::new_err(format!("Pong failed: {}", e)))
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
