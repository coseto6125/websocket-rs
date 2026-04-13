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
use bytes::{Buf, Bytes, BytesMut};
use parking_lot::Mutex;
use pyo3::exceptions::{
    PyConnectionError, PyIndexError, PyRuntimeError, PyStopAsyncIteration, PyStopIteration,
    PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyModule, PySlice, PyString};
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
    buf: BytesMut,
    handshake_done: bool,
    handshake_fut: Option<Py<PyAny>>,
    expected_accept: String,
    pending_recv: VecDeque<Py<PyAny>>,
    backlog: VecDeque<Py<WSMessage>>,
    closed: bool,
    /// asyncio transport has passed its high-water mark — hold off on writes.
    paused: bool,
    /// Frames buffered while paused; drained on resume_writing.
    write_queue: VecDeque<Py<PyBytes>>,
    /// asyncio BufferedProtocol receive buffer. Python-side bytearray exposed
    /// to asyncio via memoryview so it writes directly into our memory without
    /// constructing a bytes object per callback.
    recv_ba: Option<Py<PyByteArray>>,
    /// Byte offset in recv_ba where asyncio will write the next chunk.
    recv_ba_write_pos: usize,
    /// Cached reference to the asyncio loop — avoids `asyncio.get_running_loop()`
    /// lookups on every recv() slow-path.
    loop_ref: Option<Py<PyAny>>,
    /// Negotiated subprotocol (Sec-WebSocket-Protocol response value), if any.
    subprotocol: Option<String>,
    /// Close-frame fields (populated after receiving a CLOSE opcode).
    close_code: Option<u16>,
    close_reason: Option<String>,
}

/// Pre-completed awaitable. Yields the stored result via StopIteration on first
/// `__next__`, bypassing asyncio.Future entirely. Used by recv() when a message
/// is already available in the backlog — saves one create_future + one set_result
/// per call.
#[pyclass(name = "_ReadyMessage", module = "websocket_rs.native_client", unsendable)]
struct ReadyMessage {
    result: Option<PyResult<Py<PyAny>>>,
}

#[pymethods]
impl ReadyMessage {
    fn __await__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&mut self, _py: Python<'_>) -> PyResult<()> {
        match self.result.take() {
            Some(Ok(val)) => Err(PyStopIteration::new_err((val,))),
            Some(Err(e)) => Err(e),
            None => Err(PyStopIteration::new_err(())),
        }
    }
}

fn ready_ok<'py>(py: Python<'py>, val: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let rm = Bound::new(
        py,
        ReadyMessage {
            result: Some(Ok(val)),
        },
    )?;
    Ok(rm.into_any())
}

fn ready_err<'py>(py: Python<'py>, err: PyErr) -> PyResult<Bound<'py, PyAny>> {
    let rm = Bound::new(
        py,
        ReadyMessage {
            result: Some(Err(err)),
        },
    )?;
    Ok(rm.into_any())
}

/// Zero-copy view over a received WebSocket frame payload.
///
/// Holds an Arc-shared slice into the underlying parse buffer — constructing
/// one is O(1) regardless of payload size. Exposes the Python buffer protocol
/// so ``memoryview(msg)``, ``struct.unpack_from``, ``msg[:N]`` slicing, and
/// ``bytes(msg)`` all work as expected. ``bytes(msg)`` is the only path that
/// materialises a copy.
#[pyclass(name = "WSMessage", module = "websocket_rs.native_client", frozen)]
pub struct WSMessage {
    data: Bytes,
}

#[pymethods]
impl WSMessage {
    fn __len__(&self) -> usize {
        self.data.len()
    }

    fn __bytes__<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.data)
    }

    fn __repr__(&self) -> String {
        format!("WSMessage(len={})", self.data.len())
    }

    fn __getitem__<'py>(
        &self,
        py: Python<'py>,
        key: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Ok(idx) = key.extract::<isize>() {
            let n = self.data.len() as isize;
            let i = if idx < 0 { idx + n } else { idx };
            if i < 0 || i >= n {
                return Err(PyIndexError::new_err("WSMessage index out of range"));
            }
            return Ok(self.data[i as usize].into_pyobject(py)?.into_any());
        }
        if let Ok(slice) = key.cast::<PySlice>() {
            let indices = slice.indices(self.data.len() as isize)?;
            let (start, stop, step) = (indices.start, indices.stop, indices.step);
            if step == 1 {
                let (s, e) = (start.max(0) as usize, stop.max(0) as usize);
                let e = e.min(self.data.len());
                return Ok(PyBytes::new(py, &self.data[s..e]).into_any());
            }
            // Non-contiguous slice — materialise.
            let mut out = Vec::new();
            if step > 0 {
                let mut i = start;
                while i < stop {
                    out.push(self.data[i as usize]);
                    i += step;
                }
            } else {
                let mut i = start;
                while i > stop {
                    out.push(self.data[i as usize]);
                    i += step;
                }
            }
            return Ok(PyBytes::new(py, &out).into_any());
        }
        Err(PyTypeError::new_err(
            "WSMessage indices must be int or slice",
        ))
    }

    fn __eq__(&self, other: Bound<'_, PyAny>) -> PyResult<bool> {
        // bytes / bytearray / memoryview all compare as buffer
        if let Ok(pb) = other.cast::<PyBytes>() {
            return Ok(pb.as_bytes() == self.data.as_ref());
        }
        if let Ok(other_ws) = other.extract::<PyRef<WSMessage>>() {
            return Ok(other_ws.data == self.data);
        }
        // Fallback: try buffer protocol
        if let Ok(buf) = pyo3::buffer::PyBuffer::<u8>::get(&other) {
            if let Some(slice) = buf.as_slice(other.py()) {
                let as_u8: Vec<u8> = slice.iter().map(|c| c.get()).collect();
                return Ok(as_u8 == self.data.as_ref());
            }
        }
        Ok(false)
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        self.data.as_ref().hash(&mut h);
        h.finish()
    }

    // ---- Python buffer protocol ----

    /// Expose the underlying Bytes as a read-only Python buffer. Zero-copy.
    #[allow(clippy::missing_safety_doc)]
    unsafe fn __getbuffer__(
        slf: PyRef<'_, Self>,
        view: *mut pyo3::ffi::Py_buffer,
        flags: std::os::raw::c_int,
    ) -> PyResult<()> {
        let bytes = &slf.data;
        let ret = pyo3::ffi::PyBuffer_FillInfo(
            view,
            slf.as_ptr(),
            bytes.as_ptr() as *mut std::os::raw::c_void,
            bytes.len() as pyo3::ffi::Py_ssize_t,
            1, // readonly
            flags,
        );
        if ret == -1 {
            return Err(PyErr::fetch(slf.py()));
        }
        Ok(())
    }

    #[allow(clippy::missing_safety_doc)]
    unsafe fn __releasebuffer__(_slf: PyRef<'_, Self>, _view: *mut pyo3::ffi::Py_buffer) {
        // PyBuffer_FillInfo does not allocate; nothing to free.
    }
}

/// WebSocket client running as an asyncio.Protocol implementation in Rust.
///
/// Instances are produced by :func:`websocket_rs.native_client.connect` — direct
/// construction via ``NativeClient()`` is unsupported.
#[pyclass(name = "NativeClient", module = "websocket_rs.native_client", unsendable)]
pub struct NativeClient {
    state: Arc<Mutex<State>>,
}

fn build_handshake(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    subprotocols: &[String],
) -> (Vec<u8>, String) {
    let mut key_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key_bytes);
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let accept_src = format!("{}{}", key, MAGIC);
    let mut hasher = Sha1::new();
    hasher.update(accept_src.as_bytes());
    let expected = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());

    let mut req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    if !subprotocols.is_empty() {
        req.push_str("Sec-WebSocket-Protocol: ");
        req.push_str(&subprotocols.join(", "));
        req.push_str("\r\n");
    }
    // Skip the handful of headers we already manage ourselves; case-insensitive match.
    const RESERVED: &[&str] = &[
        "host",
        "upgrade",
        "connection",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
    ];
    for (k, v) in headers {
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(k)) {
            continue;
        }
        req.push_str(k);
        req.push_str(": ");
        req.push_str(v);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    (req.into_bytes(), expected)
}

/// Encode a masked control frame (ping=0x9 / pong=0xA). Payload ≤125 bytes per RFC.
fn encode_control_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let plen = payload.len().min(125);
    let mut out = Vec::with_capacity(2 + 4 + plen);
    let mut mask = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut mask);
    out.push(0x80 | opcode);
    out.push(0x80 | plen as u8);
    out.extend_from_slice(&mask);
    out.extend_from_slice(&payload[..plen]);
    let start = out.len() - plen;
    apply_mask(&mut out[start..], mask);
    out
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
        let mut state = self.state.lock();

        // Fast path: if our internal buf is empty and the handshake is already done,
        // parse frames straight out of `data` and only copy the tail (if any) back into
        // buf. Servers like picows that deliver one frame per write hit this path and
        // save a memcpy per callback.
        if state.handshake_done && state.buf.is_empty() {
            let mut off = 0usize;
            while let Some((opcode, plen, hdr)) = parse_header(&data[off..]) {
                if data.len() - off < hdr + plen {
                    break;
                }
                let total = hdr + plen;
                let slice = &data[off + hdr..off + total];
                match opcode {
                    0x1 | 0x2 => {
                        // One memcpy into a fresh Bytes (no shared-buffer split available
                        // since we don't own `data`), zero-copy from there to the Python
                        // layer via WSMessage's buffer protocol.
                        let payload = Bytes::copy_from_slice(slice);
                        let msg = Py::new(py, WSMessage { data: payload })?;
                        Self::deliver_message(py, &mut state, msg)?;
                    }
                    0x8 => {
                        state.closed = true;
                        Self::fail_all_pending(py, &mut state, "Connection closed by peer");
                        if let Some(t) = state.transport.as_ref() {
                            let tb = t.bind(py);
                            let _ = tb.call_method0("close");
                        }
                        return Ok(());
                    }
                    _ => {}
                }
                off += total;
            }
            if off < data.len() {
                state.buf.extend_from_slice(&data[off..]);
            }
            return Ok(());
        }

        // Slow path: handshake in progress or buf already holds partial frame data.
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
            state.buf.advance(end);
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

        drop(state);
        self.process_buffered_frames(py)
    }

    // ---- asyncio.BufferedProtocol interface (preferred path on uvloop) ----
    //
    // uvloop and CPython's asyncio both prefer BufferedProtocol when get_buffer /
    // buffer_updated are present — they write bytes directly into a memoryview we
    // hand out, skipping the `bytes` object allocation that data_received incurs.

    /// Return a writable memoryview into our internal bytearray where asyncio
    /// should place the next chunk of bytes. Grows the bytearray if necessary.
    fn get_buffer<'py>(&self, py: Python<'py>, size_hint: isize) -> PyResult<Bound<'py, PyAny>> {
        let want = size_hint.max(16384) as usize + 1024;
        let mut state = self.state.lock();

        // Lazy-init the bytearray on first call.
        let existing_len = if let Some(ba) = state.recv_ba.as_ref() {
            ba.bind(py).len()
        } else {
            let ba = PyByteArray::new_with(py, 64 * 1024, |_| Ok(()))?;
            state.recv_ba = Some(ba.unbind());
            64 * 1024
        };

        let needed = state.recv_ba_write_pos + want;
        if existing_len < needed {
            // Extend bytearray with zeros. Use Python-side `extend`.
            let ba = state.recv_ba.as_ref().unwrap().bind(py);
            let zeros = PyByteArray::new_with(py, needed - existing_len, |_| Ok(()))?;
            ba.call_method1("extend", (zeros,))?;
        }

        let ba_ref = state.recv_ba.as_ref().unwrap().clone_ref(py);
        let start = state.recv_ba_write_pos as isize;
        drop(state);
        let ba_bound = ba_ref.bind(py);
        let mv = py
            .import("builtins")?
            .getattr("memoryview")?
            .call1((ba_bound,))?;
        let slice = PySlice::new(py, start, isize::MAX, 1);
        mv.get_item(slice)
    }

    /// Called by asyncio after it writes `nbytes` into the memoryview we handed out.
    fn buffer_updated(&self, py: Python<'_>, nbytes: isize) -> PyResult<()> {
        let nbytes = nbytes as usize;
        if nbytes == 0 {
            return Ok(());
        }
        let mut state = self.state.lock();
        let write_start = state.recv_ba_write_pos;
        state.recv_ba_write_pos += nbytes;

        // Copy the newly-written bytes from the bytearray into state.buf so the
        // existing parse logic (which owns a Vec<u8>) can run unchanged. This still
        // avoids the per-callback `bytes` object that vanilla data_received
        // materialises in asyncio-land.
        let ba_ref = state.recv_ba.as_ref().unwrap().clone_ref(py);
        let ba_bound = ba_ref.bind(py);
        // PyByteArray::as_bytes is unsafe: the data pointer is valid while we hold the GIL
        // and don't mutate the bytearray concurrently; both hold here.
        let slice = unsafe { ba_bound.as_bytes() };
        state
            .buf
            .extend_from_slice(&slice[write_start..write_start + nbytes]);

        // If the bytearray's consumed bytes are large, reset write_pos back to 0 so
        // we don't grow it unboundedly.
        if state.recv_ba_write_pos > 1 << 20 {
            state.recv_ba_write_pos = 0;
        }
        drop(state);

        // Reuse the same parse path data_received uses. We pass an empty slice since
        // the data is already in state.buf.
        self.process_buffered_frames(py)
    }

    /// Called by asyncio transport when its send buffer crosses the high-water mark.
    /// We stop draining into the transport; subsequent send() calls buffer internally.
    fn pause_writing(&self) {
        self.state.lock().paused = true;
    }

    /// Called when the transport drains below the low-water mark. Flush whatever we
    /// queued while paused, then clear the flag.
    fn resume_writing(&self, py: Python<'_>) -> PyResult<()> {
        let mut state = self.state.lock();
        state.paused = false;
        // Drain queued frames. Each one is a fully-encoded PyBytes.
        let transport = match state.transport.as_ref() {
            Some(t) => t.clone_ref(py),
            None => return Ok(()),
        };
        let mut queue = std::mem::take(&mut state.write_queue);
        drop(state);
        let tb = transport.bind(py);
        while let Some(pb) = queue.pop_front() {
            tb.call_method1("write", (pb,))?;
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

        // If the transport has paused us, buffer the encoded frame until resume_writing.
        // Re-lock briefly to check the flag and push if needed.
        {
            let mut state = self.state.lock();
            if state.paused {
                state.write_queue.push_back(out.unbind());
                return Ok(());
            }
        }
        transport.bind(py).call_method1("write", (out,))?;
        Ok(())
    }

    /// Returns an asyncio.Future that completes with the next received frame payload.
    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut state = self.state.lock();
        // Fast path: message already in backlog — bypass asyncio.Future entirely.
        if let Some(payload) = state.backlog.pop_front() {
            drop(state);
            return ready_ok(py, payload.into_any());
        }
        // Closed path: same — ReadyMessage carrying an exception short-circuits
        // awaits without a Future alloc.
        if state.closed {
            drop(state);
            return ready_err(py, PyConnectionError::new_err("Connection closed"));
        }
        // Slow path: message not yet arrived. Use asyncio.Future so data_received
        // can complete it across the callback boundary.
        let loop_ref = state
            .loop_ref
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Event loop not bound"))?
            .clone_ref(py);
        drop(state);
        let fut = loop_ref.bind(py).call_method0("create_future")?;
        self.state
            .lock()
            .pending_recv
            .push_back(fut.clone().unbind());
        Ok(fut)
    }

    #[getter]
    fn is_open(&self) -> bool {
        let s = self.state.lock();
        s.handshake_done && !s.closed && s.transport.is_some()
    }

    // ---- Async iteration: `async for msg in ws` ----
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Async iterator step. Returns the next WSMessage; raises StopAsyncIteration
    /// when the connection is closed (vs recv() which raises ConnectionError).
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut state = self.state.lock();
        if let Some(payload) = state.backlog.pop_front() {
            drop(state);
            return ready_ok(py, payload.into_any());
        }
        if state.closed {
            drop(state);
            return ready_err(py, PyStopAsyncIteration::new_err("Connection closed"));
        }
        let loop_ref = state
            .loop_ref
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Event loop not bound"))?
            .clone_ref(py);
        drop(state);
        let fut = loop_ref.bind(py).call_method0("create_future")?;
        self.state
            .lock()
            .pending_recv
            .push_back(fut.clone().unbind());
        Ok(fut)
    }

    // ---- Async context manager: `async with connect(...) as ws:` ----
    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        ready_ok(py, slf.into_any())
    }

    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Py<PyAny>>,
        _exc_value: Option<Py<PyAny>>,
        _traceback: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.close(py)?;
        ready_ok(py, py.None())
    }

    #[getter]
    fn subprotocol(&self) -> Option<String> {
        self.state.lock().subprotocol.clone()
    }

    #[getter]
    fn close_code(&self) -> Option<u16> {
        self.state.lock().close_code
    }

    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.state.lock().close_reason.clone()
    }

    /// Send a ping frame. Payload must be ≤125 bytes (control-frame limit).
    #[pyo3(signature = (data=None))]
    fn ping(&self, py: Python<'_>, data: Option<Vec<u8>>) -> PyResult<()> {
        let payload = data.unwrap_or_default();
        if payload.len() > 125 {
            return Err(PyValueError::new_err(
                "ping payload exceeds 125 bytes (WS control-frame limit)",
            ));
        }
        let state = self.state.lock();
        if state.closed {
            return Err(PyRuntimeError::new_err("WebSocket is closed"));
        }
        let transport = state
            .transport
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("No transport"))?
            .clone_ref(py);
        drop(state);
        let frame = encode_control_frame(0x9, &payload);
        transport
            .bind(py)
            .call_method1("write", (PyBytes::new(py, &frame),))?;
        Ok(())
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
    /// Parse and dispatch all complete frames currently sitting in state.buf.
    /// Also completes the HTTP/101 handshake on first invocation.
    fn process_buffered_frames(&self, py: Python<'_>) -> PyResult<()> {
        let mut state = self.state.lock();

        if !state.handshake_done {
            let Some(end) = find_header_end(&state.buf) else {
                return Ok(());
            };
            let headers_str = String::from_utf8_lossy(&state.buf[..end]).to_string();
            let expected = state.expected_accept.clone();
            let mut matched = false;
            let mut subprotocol: Option<String> = None;
            for line in headers_str.lines() {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("sec-websocket-accept:") && line.contains(&expected) {
                    matched = true;
                } else if let Some(rest) = line
                    .splitn(2, ':')
                    .nth(1)
                    .filter(|_| lower.starts_with("sec-websocket-protocol:"))
                {
                    subprotocol = Some(rest.trim().to_string());
                }
            }
            state.buf.advance(end);
            state.subprotocol = subprotocol;
            if !matched {
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

        loop {
            let Some((opcode, plen, hdr)) = parse_header(&state.buf) else {
                break;
            };
            if state.buf.len() < hdr + plen {
                break;
            }
            let total = hdr + plen;
            match opcode {
                0x1 | 0x2 => {
                    // Zero-copy: advance past header, then split_to() hands us an
                    // Arc-backed Bytes for the payload range. No memcpy at all —
                    // the PyBytes alloc that used to cost one per frame is gone.
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen).freeze();
                    let msg = Py::new(py, WSMessage { data: payload })?;
                    Self::deliver_message(py, &mut state, msg)?;
                }
                0x8 => {
                    // Close: body is [u16 code | reason (utf-8)] per RFC 6455 §5.5.1.
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen);
                    if payload.len() >= 2 {
                        state.close_code = Some(u16::from_be_bytes([payload[0], payload[1]]));
                        if payload.len() > 2 {
                            state.close_reason = Some(
                                String::from_utf8_lossy(&payload[2..]).into_owned(),
                            );
                        }
                    }
                    state.closed = true;
                    Self::fail_all_pending(py, &mut state, "Connection closed by peer");
                    if let Some(t) = state.transport.as_ref() {
                        let tb = t.bind(py);
                        let _ = tb.call_method0("close");
                    }
                    break;
                }
                0x9 => {
                    // Ping: echo payload back as a Pong frame (RFC 6455 §5.5.2).
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen).freeze();
                    let transport = state.transport.as_ref().map(|t| t.clone_ref(py));
                    if let Some(t) = transport {
                        let frame = encode_control_frame(0xA, &payload);
                        let _ = t.bind(py).call_method1("write", (PyBytes::new(py, &frame),));
                    }
                }
                0xA => {
                    // Pong — silently consumed; could dispatch to a ping-waiter in future.
                    state.buf.advance(total);
                }
                _ => {
                    state.buf.advance(total);
                }
            }
        }
        Ok(())
    }

    fn deliver_message(py: Python<'_>, state: &mut State, msg: Py<WSMessage>) -> PyResult<()> {
        if let Some(fut) = state.pending_recv.pop_front() {
            let fb = fut.bind(py);
            if !fb.call_method0("done")?.extract::<bool>().unwrap_or(false) {
                fb.call_method1("set_result", (msg,))?;
            }
        } else {
            state.backlog.push_back(msg);
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


/// Connect to a ws:// or wss:// URI and return a NativeClient once the handshake completes.
///
/// TLS is delegated to asyncio — we pass an ``ssl.SSLContext`` through to
/// ``loop.create_connection``, so the protocol sees decrypted bytes. If a
/// custom context is needed (self-signed, client cert), pass it via ``ssl_context``.
#[pyfunction]
#[pyo3(signature = (uri, *, headers=None, subprotocols=None, ssl_context=None, connect_timeout=None))]
fn connect<'py>(
    py: Python<'py>,
    uri: String,
    headers: Option<Vec<(String, String)>>,
    subprotocols: Option<Vec<String>>,
    ssl_context: Option<Py<PyAny>>,
    connect_timeout: Option<f64>,
) -> PyResult<Bound<'py, PyAny>> {
    let (scheme, host, port, path) = parse_ws_uri(&uri)?;
    let is_tls = scheme == "wss";
    let client = NativeClient {
        state: Arc::new(Mutex::new(State {
            transport: None,
            buf: BytesMut::with_capacity(16384),
            handshake_done: false,
            handshake_fut: None,
            expected_accept: String::new(),
            pending_recv: VecDeque::new(),
            backlog: VecDeque::new(),
            closed: false,
            paused: false,
            write_queue: VecDeque::new(),
            recv_ba: None,
            recv_ba_write_pos: 0,
            loop_ref: None,
            subprotocol: None,
            close_code: None,
            close_reason: None,
        })),
    };
    let state_arc = client.state.clone();
    let client_obj = Py::new(py, client)?;

    // Build handshake bytes + expected accept
    let headers_vec = headers.unwrap_or_default();
    let subprotocols_vec = subprotocols.unwrap_or_default();
    let (req_bytes, expected) =
        build_handshake(&host, port, &path, &headers_vec, &subprotocols_vec);
    state_arc.lock().expected_accept = expected;

    // Create the handshake future and park it
    let asyncio = py.import("asyncio")?;
    let loop_ = asyncio.call_method0("get_running_loop")?;
    let handshake_fut = loop_.call_method0("create_future")?;
    {
        let mut st = state_arc.lock();
        st.handshake_fut = Some(handshake_fut.clone().unbind());
        st.loop_ref = Some(loop_.clone().unbind());
    }

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
    let timeout_obj = match connect_timeout {
        Some(t) => t.into_pyobject(py)?.into_any(),
        None => py.None().into_bound(py),
    };
    helper.call1((
        create_conn_coro,
        req_bytes,
        handshake_fut,
        client_obj,
        timeout_obj,
    ))
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
import asyncio as _asyncio

async def _connect_helper(create_conn_coro, req_bytes, handshake_fut, client, connect_timeout):
    async def _do():
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
    if connect_timeout is not None:
        return await _asyncio.wait_for(_do(), timeout=connect_timeout)
    return await _do()
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
