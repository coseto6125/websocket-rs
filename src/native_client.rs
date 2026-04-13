//! Native asyncio.Protocol WebSocket client.
//!
//! Runs entirely on the asyncio event loop thread — no tokio runtime involvement
//! post-handshake, no cross-thread wakeup (call_soon_threadsafe). Frame codec is
//! in Rust with AVX2-friendly masking.
//!
//! Scope for this MVP commit:
//! - ws:// plain TCP only (TLS / proxy land in follow-ups)
//! - Binary + Text messages; opcodes 0x1 / 0x2 / 0x8 (close)
//! - Client-side handshake (RFC 6455 §4.1)
//! - Fire-and-forget send(), async recv()
//!
//! Deliberately NOT in this commit: ping/pong, fragmented messages, permessage-deflate,
//! custom headers/subprotocols, receive_timeout. All can be layered on without
//! touching the hot path.
use std::collections::VecDeque;
use std::sync::Arc;

use base64::Engine;
use bytes::Bytes;
use parking_lot::Mutex;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyModule, PyString};
use rand::RngCore;
use sha1::{Digest, Sha1};

const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[inline]
fn apply_mask(buf: &mut [u8], mask: [u8; 4]) {
    // Auto-vectorized by rustc when built with -C target-feature=+avx2 (see .cargo/config.toml).
    let mask_u32 = u32::from_ne_bytes(mask);
    let (prefix, words, suffix) = unsafe { buf.align_to_mut::<u32>() };
    for (i, b) in prefix.iter_mut().enumerate() {
        *b ^= mask[i & 3];
    }
    let head = prefix.len() & 3;
    let rotated = if head > 0 {
        mask_u32.rotate_right(8 * head as u32)
    } else {
        mask_u32
    };
    for w in words.iter_mut() {
        *w ^= rotated;
    }
    let tail_mask = rotated.to_ne_bytes();
    for (i, b) in suffix.iter_mut().enumerate() {
        *b ^= tail_mask[i & 3];
    }
}

/// Minimum bytes needed before a frame header can be (potentially) fully parsed.
const MIN_HDR: usize = 2;

/// Parse a single server frame header (no mask — server->client frames are never masked).
/// Returns (opcode, payload_len, header_size) or None if not enough data.
fn parse_header(buf: &[u8]) -> Option<(u8, usize, usize)> {
    if buf.len() < MIN_HDR {
        return None;
    }
    let b0 = buf[0];
    let b1 = buf[1];
    let opcode = b0 & 0x0F;
    let plen_short = b1 & 0x7F;
    let (plen, hdr) = match plen_short {
        0..=125 => (plen_short as usize, 2usize),
        126 => {
            if buf.len() < 4 {
                return None;
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as usize, 4)
        }
        127 => {
            if buf.len() < 10 {
                return None;
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&buf[2..10]);
            (u64::from_be_bytes(arr) as usize, 10)
        }
        _ => unreachable!(),
    };
    // Server must NOT mask; we don't enforce here (picows/tungstenite both accept).
    Some((opcode, plen, hdr))
}

struct State {
    transport: Option<Py<PyAny>>,
    buf: Vec<u8>,
    handshake_done: bool,
    handshake_fut: Option<Py<PyAny>>,
    expected_accept: String,
    pending_recv: VecDeque<Py<PyAny>>,
    backlog: VecDeque<Bytes>,
    closed: bool,
}

/// WebSocket client running as an asyncio.Protocol implementation in Rust.
///
/// Instances are produced by :func:`websocket_rs.native_client.connect` — direct
/// construction via ``NativeClient()`` is unsupported.
#[pyclass(name = "NativeClient", module = "websocket_rs.native_client", unsendable)]
pub struct NativeClient {
    state: Arc<Mutex<State>>,
}

fn build_handshake(host: &str, port: u16, path: &str) -> (Vec<u8>, String) {
    let mut key_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key_bytes);
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let accept_src = format!("{}{}", key, MAGIC);
    let mut hasher = Sha1::new();
    hasher.update(accept_src.as_bytes());
    let expected = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    (req.into_bytes(), expected)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

#[pymethods]
impl NativeClient {
    // ---- asyncio.Protocol interface ----

    fn connection_made(&self, transport: Py<PyAny>) {
        self.state.lock().transport = Some(transport);
    }

    fn data_received(&self, py: Python<'_>, data: &[u8]) -> PyResult<()> {
        // Fast path: append and process
        let mut state = self.state.lock();
        state.buf.extend_from_slice(data);

        if !state.handshake_done {
            let Some(end) = find_header_end(&state.buf) else {
                return Ok(());
            };
            let headers = String::from_utf8_lossy(&state.buf[..end]);
            // Case-insensitive search for the accept key token.
            let expected = state.expected_accept.clone();
            let matched = headers
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("sec-websocket-accept:")
                    && l.contains(&expected));
            state.buf.drain(..end);
            if !matched {
                // Fail handshake future
                if let Some(fut) = state.handshake_fut.take() {
                    let fut_b = fut.bind(py);
                    let exc = PyConnectionError::new_err("WebSocket handshake failed");
                    let _ = fut_b.call_method1("set_exception", (exc,));
                }
                return Ok(());
            }
            state.handshake_done = true;
            if let Some(fut) = state.handshake_fut.take() {
                let fut_b = fut.bind(py);
                if !fut_b
                    .call_method0("done")?
                    .extract::<bool>()
                    .unwrap_or(false)
                {
                    let _ = fut_b.call_method1("set_result", (py.None(),));
                }
            }
        }

        // Parse any frames in buffer.
        loop {
            let Some((opcode, plen, hdr)) = parse_header(&state.buf) else {
                break;
            };
            if state.buf.len() < hdr + plen {
                break;
            }
            // Drain header
            let total = hdr + plen;
            match opcode {
                0x1 | 0x2 => {
                    // Take payload as Bytes (cheap Arc) to avoid copying again later.
                    let payload: Bytes = Bytes::copy_from_slice(&state.buf[hdr..total]);
                    state.buf.drain(..total);
                    Self::deliver_payload(py, &mut state, payload)?;
                }
                0x8 => {
                    state.buf.drain(..total);
                    state.closed = true;
                    Self::fail_all_pending(py, &mut state, "Connection closed by peer");
                    if let Some(t) = state.transport.as_ref() {
                        let tb = t.bind(py);
                        let _ = tb.call_method0("close");
                    }
                    break;
                }
                // 0x9 ping / 0xA pong: ignored for MVP (server-side echoes not expected)
                0x0 | 0x9 | 0xA => {
                    state.buf.drain(..total);
                }
                _ => {
                    state.buf.drain(..total);
                }
            }
        }
        Ok(())
    }

    fn connection_lost(&self, py: Python<'_>, _exc: Py<PyAny>) {
        let mut state = self.state.lock();
        state.closed = true;
        Self::fail_all_pending(py, &mut state, "Connection lost");
        state.transport = None;
    }

    // ---- User-facing API ----

    /// Encode a single binary frame and write it directly to the transport.
    /// Zero-copy: the encoded frame is materialised straight into Python memory.
    fn send(&self, py: Python<'_>, message: Bound<'_, PyAny>) -> PyResult<()> {
        let state = self.state.lock();
        if state.closed {
            return Err(PyRuntimeError::new_err("WebSocket is closed"));
        }
        if !state.handshake_done {
            return Err(PyRuntimeError::new_err("WebSocket handshake not complete"));
        }
        let transport = state
            .transport
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("No transport"))?
            .clone_ref(py);
        drop(state);

        // Borrow payload as slice — single memcpy into the PyBytes output below.
        let (payload, opcode): (&[u8], u8) = if let Ok(pb) = message.cast::<PyBytes>() {
            (pb.as_bytes(), 0x2)
        } else if let Ok(s) = message.cast::<PyString>() {
            (s.to_str()?.as_bytes(), 0x1)
        } else {
            return Err(PyValueError::new_err("message must be str or bytes"));
        };

        let plen = payload.len();
        let header_len = 2 + match plen {
            0..=125 => 0,
            126..=65535 => 2,
            _ => 8,
        } + 4; // 4-byte mask
        let total = header_len + plen;

        let mut mask_key = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut mask_key);

        // Allocate PyBytes and fill directly — one memcpy total.
        let out = PyBytes::new_with(py, total, |buf| {
            buf[0] = 0x80 | opcode; // FIN=1
            let mut pos = 2;
            if plen <= 125 {
                buf[1] = 0x80 | plen as u8;
            } else if plen <= 65535 {
                buf[1] = 0x80 | 126;
                buf[2..4].copy_from_slice(&(plen as u16).to_be_bytes());
                pos = 4;
            } else {
                buf[1] = 0x80 | 127;
                buf[2..10].copy_from_slice(&(plen as u64).to_be_bytes());
                pos = 10;
            }
            buf[pos..pos + 4].copy_from_slice(&mask_key);
            pos += 4;
            buf[pos..pos + plen].copy_from_slice(payload);
            apply_mask(&mut buf[pos..pos + plen], mask_key);
            Ok(())
        })?;

        transport.bind(py).call_method1("write", (out,))?;
        Ok(())
    }

    /// Returns an asyncio.Future that completes with the next received frame payload.
    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut state = self.state.lock();
        if let Some(payload) = state.backlog.pop_front() {
            drop(state);
            let fut = create_loop_future(py)?;
            fut.call_method1("set_result", (PyBytes::new(py, &payload),))?;
            return Ok(fut);
        }
        if state.closed {
            drop(state);
            let fut = create_loop_future(py)?;
            fut.call_method1(
                "set_exception",
                (PyConnectionError::new_err("Connection closed"),),
            )?;
            return Ok(fut);
        }
        let fut = create_loop_future(py)?;
        state.pending_recv.push_back(fut.clone().unbind());
        Ok(fut)
    }

    #[getter]
    fn is_open(&self) -> bool {
        let s = self.state.lock();
        s.handshake_done && !s.closed && s.transport.is_some()
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let mut state = self.state.lock();
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        if let Some(t) = state.transport.as_ref() {
            // Send close frame (best-effort) then close transport.
            let close_frame: [u8; 6] = [
                0x88, 0x80, // FIN | opcode=8, masked, length=0
                0, 0, 0, 0, // mask key (payload empty so mask value immaterial)
            ];
            let tb = t.bind(py);
            let _ = tb.call_method1("write", (PyBytes::new(py, &close_frame),));
            let _ = tb.call_method0("close");
        }
        Self::fail_all_pending(py, &mut state, "Connection closed by client");
        Ok(())
    }
}

// Helpers (non-pymethod)
impl NativeClient {
    fn deliver_payload(py: Python<'_>, state: &mut State, payload: Bytes) -> PyResult<()> {
        if let Some(fut) = state.pending_recv.pop_front() {
            let fb = fut.bind(py);
            if !fb.call_method0("done")?.extract::<bool>().unwrap_or(false) {
                fb.call_method1("set_result", (PyBytes::new(py, &payload),))?;
            }
        } else {
            state.backlog.push_back(payload);
        }
        Ok(())
    }

    fn fail_all_pending(py: Python<'_>, state: &mut State, msg: &str) {
        while let Some(fut) = state.pending_recv.pop_front() {
            let fb = fut.bind(py);
            if !fb
                .call_method0("done")
                .and_then(|d| d.extract::<bool>())
                .unwrap_or(true)
            {
                let _ = fb.call_method1(
                    "set_exception",
                    (PyConnectionError::new_err(msg.to_string()),),
                );
            }
        }
    }
}

fn create_loop_future(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let asyncio = py.import("asyncio")?;
    let loop_ = asyncio.call_method0("get_running_loop")?;
    loop_.call_method0("create_future")
}

/// Connect to a ws:// or wss:// URI and return a NativeClient once the handshake completes.
///
/// TLS is delegated to asyncio — we pass an ``ssl.SSLContext`` through to
/// ``loop.create_connection``, so the protocol sees decrypted bytes. If a
/// custom context is needed (self-signed, client cert), pass it via ``ssl_context``.
#[pyfunction]
#[pyo3(signature = (uri, *, ssl_context=None))]
fn connect<'py>(
    py: Python<'py>,
    uri: String,
    ssl_context: Option<Py<PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let (scheme, host, port, path) = parse_ws_uri(&uri)?;
    let is_tls = scheme == "wss";
    let client = NativeClient {
        state: Arc::new(Mutex::new(State {
            transport: None,
            buf: Vec::with_capacity(4096),
            handshake_done: false,
            handshake_fut: None,
            expected_accept: String::new(),
            pending_recv: VecDeque::new(),
            backlog: VecDeque::new(),
            closed: false,
        })),
    };
    let state_arc = client.state.clone();
    let client_obj = Py::new(py, client)?;

    // Build handshake bytes + expected accept
    let (req_bytes, expected) = build_handshake(&host, port, &path);
    state_arc.lock().expected_accept = expected;

    // Create the handshake future and park it
    let asyncio = py.import("asyncio")?;
    let loop_ = asyncio.call_method0("get_running_loop")?;
    let handshake_fut = loop_.call_method0("create_future")?;
    state_arc.lock().handshake_fut = Some(handshake_fut.clone().unbind());

    // Launch the low-level create_connection + post-connection handshake send as a task
    let protocol_factory = {
        let client_clone = client_obj.clone_ref(py);
        pyo3::types::PyCFunction::new_closure(
            py,
            None,
            None,
            move |_args, _kwargs| -> PyResult<Py<PyAny>> {
                Python::attach(|py| Ok(client_clone.clone_ref(py).into_any()))
            },
        )?
    };

    // Resolve SSL context if wss:// (user-supplied overrides default).
    let ssl_arg: Py<PyAny> = if is_tls {
        match ssl_context {
            Some(ctx) => ctx,
            None => py
                .import("ssl")?
                .call_method0("create_default_context")?
                .unbind(),
        }
    } else {
        py.None()
    };

    // loop.create_connection(protocol_factory, host, port, ssl=..., server_hostname=...) -> coro
    let kwargs = pyo3::types::PyDict::new(py);
    if is_tls {
        kwargs.set_item("ssl", ssl_arg)?;
        kwargs.set_item("server_hostname", host.clone())?;
    }
    let create_conn_coro = loop_.call_method(
        "create_connection",
        (protocol_factory, host.clone(), port),
        Some(&kwargs),
    )?;

    // Chain: await create_connection, then transport.write(request), then await handshake_fut, then return client
    // We implement this by wrapping in an async Python function crafted from asyncio.
    // Simplest: schedule a Python helper that we build inline via an async def compiled once.
    let helper = get_connect_helper(py)?;
    helper.call1((create_conn_coro, req_bytes, handshake_fut, client_obj))
}

fn parse_ws_uri(uri: &str) -> PyResult<(&'static str, String, u16, String)> {
    let (scheme, rest, default_port): (&str, &str, u16) = if let Some(r) = uri.strip_prefix("wss://")
    {
        ("wss", r, 443)
    } else if let Some(r) = uri.strip_prefix("ws://") {
        ("ws", r, 80)
    } else {
        return Err(PyValueError::new_err("URI must start with ws:// or wss://"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rfind(':') {
        Some(i) => (
            authority[..i].to_string(),
            authority[i + 1..]
                .parse()
                .map_err(|_| PyValueError::new_err("Invalid port"))?,
        ),
        None => (authority.to_string(), default_port),
    };
    Ok((scheme, host, port, path.to_string()))
}

/// Cached Python helper that orchestrates create_connection -> send handshake -> await accept -> return client.
fn get_connect_helper(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Py<PyAny>> = OnceLock::new();
    if let Some(h) = CACHE.get() {
        return Ok(h.bind(py).clone());
    }
    let code = r#"
async def _connect_helper(create_conn_coro, req_bytes, handshake_fut, client):
    transport, _proto = await create_conn_coro
    try:
        sock = transport.get_extra_info("socket")
        if sock is not None:
            import socket as _s
            sock.setsockopt(_s.IPPROTO_TCP, _s.TCP_NODELAY, 1)
    except Exception:
        pass
    transport.write(bytes(req_bytes))
    await handshake_fut
    return client
"#;
    let module = PyModule::from_code(py, std::ffi::CString::new(code)?.as_c_str(), c"helper.py", c"helper")?;
    let helper = module.getattr("_connect_helper")?;
    let _ = CACHE.set(helper.clone().unbind());
    Ok(helper)
}

pub fn register_native_client(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(py, "native_client")?;
    m.add_class::<NativeClient>()?;
    m.add_function(wrap_pyfunction!(connect, &m)?)?;
    parent.add_submodule(&m)?;
    // Also register in sys.modules so `from websocket_rs.native_client import ...` works.
    let sys_modules = py.import("sys")?.getattr("modules")?;
    sys_modules.set_item("websocket_rs.native_client", &m)?;
    Ok(())
}
