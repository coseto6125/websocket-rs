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
use flate2::read::DeflateDecoder;
use flate2::{Compress, Compression, FlushCompress};
use parking_lot::Mutex;
use pyo3::exceptions::{
    PyConnectionError, PyIndexError, PyRuntimeError, PyStopAsyncIteration, PyStopIteration,
    PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyModule, PySlice, PyString};
use rand::RngCore;
use sha1::{Digest, Sha1};
use std::io::Read as _;

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
/// Returns (fin, opcode, payload_len, header_size) or None if not enough data.
fn parse_header(buf: &[u8]) -> Option<(bool, bool, u8, usize, usize)> {
    if buf.len() < MIN_HDR {
        return None;
    }
    let b0 = buf[0];
    let b1 = buf[1];
    let fin = (b0 & 0x80) != 0;
    let rsv1 = (b0 & 0x40) != 0;
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
    // Server must NOT mask; we don't enforce (most server libs accept anyway).
    Some((fin, rsv1, opcode, plen, hdr))
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
    /// Optional per-recv timeout (seconds). Applied via asyncio.wait_for wrapper
    /// only when the slow path would block — backlog fast-path skips it.
    receive_timeout: Option<f64>,
    /// Fragmented-message reassembly: accumulates continuation frame payloads
    /// until FIN=1 arrives. First frame's opcode is stashed here.
    fragment_buf: Option<BytesMut>,
    fragment_opcode: u8,
    /// True when the current fragmented message used RSV1 (compressed) in the
    /// first frame — per RFC 7692 the flag is set only on the first frame.
    fragment_rsv1: bool,
    /// permessage-deflate context, lazily initialised after negotiation.
    deflate: Option<DeflateCtx>,
}

/// permessage-deflate per-connection state. We always negotiate
/// client_no_context_takeover / server_no_context_takeover so streaming state
/// never persists across messages — the DEFLATE allocators get reset after
/// each message, trading a few % compression ratio for simpler, race-free code.
/// Marker struct — presence of Option<DeflateCtx>::Some means permessage-deflate
/// is negotiated. No per-connection state: Compress/Decompress are instantiated
/// fresh per message (no_context_takeover semantics either way).
struct DeflateCtx;

/// Pre-completed awaitable. Yields the stored result via StopIteration on first
/// `__next__`, bypassing asyncio.Future entirely. Used by recv() when a message
/// is already available in the backlog — saves one create_future + one set_result
/// per call.
#[pyclass(
    name = "_ReadyMessage",
    module = "websocket_rs.native_client",
    unsendable
)]
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
#[pyclass(
    name = "NativeClient",
    module = "websocket_rs.native_client",
    unsendable
)]
pub struct NativeClient {
    state: Arc<Mutex<State>>,
}

fn build_handshake(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    subprotocols: &[String],
    compression: bool,
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
    if compression {
        // no_context_takeover on both sides keeps decompressor state per-message,
        // matching the DeflateCtx::reset calls in process_buffered_frames.
        req.push_str(
            "Sec-WebSocket-Extensions: permessage-deflate; \
             client_no_context_takeover; server_no_context_takeover\r\n",
        );
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

/// Decompress a permessage-deflate payload. Per RFC 7692 §7.2.2 the client MUST
/// append 00 00 FF FF before feeding to a raw-DEFLATE decoder.
///
/// Uses a fresh Decompress per call — matches server_no_context_takeover and
/// sidesteps a real miniz_oxide bug where `reset(false)` leaves residual
/// internal state that corrupts subsequent decompression of large inputs.
fn decompress_message(state: &mut State, compressed: &[u8]) -> PyResult<Vec<u8>> {
    if state.deflate.is_none() {
        return Err(PyRuntimeError::new_err(
            "received compressed frame but permessage-deflate is not enabled",
        ));
    }
    let mut with_marker = Vec::with_capacity(compressed.len() + 4);
    with_marker.extend_from_slice(compressed);
    with_marker.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);

    // `read::DeflateDecoder` wraps a reader and treats the stream as raw
    // DEFLATE. read_to_end handles the grow-retry dance that decompress_vec
    // needs to be hand-coded for. Consistently decodes regardless of the
    // compressed/uncompressed size ratio.
    let mut decoder = DeflateDecoder::new(with_marker.as_slice());
    let mut out = Vec::with_capacity(compressed.len() * 4 + 128);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| PyRuntimeError::new_err(format!("deflate decode error: {e}")))?;
    Ok(out)
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
        // buf. Servers that deliver one frame per write hit this path and save a
        // memcpy per callback.
        if state.handshake_done && state.buf.is_empty() && state.fragment_buf.is_none() {
            let mut off = 0usize;
            while let Some((fin, rsv1, opcode, plen, hdr)) = parse_header(&data[off..]) {
                if data.len() - off < hdr + plen {
                    break;
                }
                // Fragmented frames (fin=false, or opcode=0x0 continuation) and
                // compressed frames (rsv1=true) need buffered handling — bail
                // to the slow path where the DeflateCtx lives.
                if !fin || opcode == 0x0 || rsv1 {
                    break;
                }
                let total = hdr + plen;
                let slice = &data[off + hdr..off + total];
                match opcode {
                    0x1 | 0x2 => {
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
                drop(state);
                return self.process_buffered_frames(py);
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
            let matched = headers.lines().any(|l| {
                l.to_ascii_lowercase().starts_with("sec-websocket-accept:") && l.contains(&expected)
            });
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

        // Borrow payload as slice — single memcpy into the PyBytes output below.
        let (raw_payload, opcode): (&[u8], u8) = if let Ok(pb) = message.cast::<PyBytes>() {
            (pb.as_bytes(), 0x2)
        } else if let Ok(s) = message.cast::<PyString>() {
            (s.to_str()?.as_bytes(), 0x1)
        } else {
            return Err(PyValueError::new_err("message must be str or bytes"));
        };

        // permessage-deflate: if negotiated, compress via a fresh DeflateEncoder
        // (no_context_takeover means we'd reset state after every message anyway
        // — starting fresh is simpler than stateful Compress::reset). Sync-flush
        // produces a stream ending in 00 00 FF FF which we then strip per
        // RFC 7692 §7.2.1. RSV1 gets set in the frame header below.
        // permessage-deflate compression via raw Compress. A fresh instance
        // each message matches client_no_context_takeover semantics and sidesteps
        // the reset() pitfalls in miniz_oxide. We reserve enough output capacity
        // up front so compress_vec finishes in a single call.
        let compressed = state.deflate.is_some();
        let deflate_buf: Vec<u8> = if compressed {
            let mut comp = Compress::new(Compression::default(), false);
            // Worst case: small overhead on random data; highly compressible
            // data is much smaller. +64 covers header + sync marker + slack.
            let mut out: Vec<u8> = Vec::with_capacity(raw_payload.len() + 64);
            // Drive compression until all input is consumed AND the Sync marker
            // has been emitted. With ample output capacity this loop finishes
            // in one or two iterations.
            let mut cursor = 0usize;
            loop {
                let need = if cursor < raw_payload.len() { 128 } else { 32 };
                if out.capacity() - out.len() < need {
                    out.reserve(raw_payload.len().max(1024));
                }
                let in_before = comp.total_in();
                comp.compress_vec(&raw_payload[cursor..], &mut out, FlushCompress::Sync)
                    .map_err(|e| PyRuntimeError::new_err(format!("deflate error: {e}")))?;
                cursor += (comp.total_in() - in_before) as usize;
                if cursor >= raw_payload.len() && out.ends_with(&[0x00, 0x00, 0xFF, 0xFF]) {
                    break;
                }
                if cursor >= raw_payload.len() && (comp.total_out() - in_before) == 0 {
                    // Defensive: no progress after input exhausted.
                    break;
                }
            }
            if out.ends_with(&[0x00, 0x00, 0xFF, 0xFF]) {
                out.truncate(out.len() - 4);
            }
            out
        } else {
            Vec::new()
        };
        let payload: &[u8] = if compressed {
            &deflate_buf
        } else {
            raw_payload
        };
        drop(state);

        let plen = payload.len();
        let header_len =
            2 + match plen {
                0..=125 => 0,
                126..=65535 => 2,
                _ => 8,
            } + 4; // 4-byte mask
        let total = header_len + plen;

        let mut mask_key = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut mask_key);

        // Allocate PyBytes and fill directly — one memcpy total.
        let out = PyBytes::new_with(py, total, |buf| {
            // FIN=1; set RSV1 (0x40) when the payload is deflated.
            buf[0] = 0x80 | opcode | if compressed { 0x40 } else { 0x00 };
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
        let timeout = state.receive_timeout;
        drop(state);
        let fut = loop_ref.bind(py).call_method0("create_future")?;
        self.state
            .lock()
            .pending_recv
            .push_back(fut.clone().unbind());
        // If a per-recv timeout is set, wrap in asyncio.wait_for so recv() raises
        // TimeoutError after `receive_timeout` seconds.
        if let Some(t) = timeout {
            let asyncio = py.import("asyncio")?;
            return asyncio.call_method1("wait_for", (fut, t));
        }
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
        let timeout = state.receive_timeout;
        drop(state);
        let fut = loop_ref.bind(py).call_method0("create_future")?;
        self.state
            .lock()
            .pending_recv
            .push_back(fut.clone().unbind());
        if let Some(t) = timeout {
            let asyncio = py.import("asyncio")?;
            return asyncio.call_method1("wait_for", (fut, t));
        }
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
            let mut deflate_accepted = false;
            for line in headers_str.lines() {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("sec-websocket-accept:") && line.contains(&expected) {
                    matched = true;
                } else if lower.starts_with("sec-websocket-protocol:") {
                    if let Some((_, rest)) = line.split_once(':') {
                        subprotocol = Some(rest.trim().to_string());
                    }
                } else if lower.starts_with("sec-websocket-extensions:")
                    && lower.contains("permessage-deflate")
                {
                    deflate_accepted = true;
                }
            }
            state.buf.advance(end);
            state.subprotocol = subprotocol;
            // Server didn't echo permessage-deflate → disable our compressor.
            if !deflate_accepted {
                state.deflate = None;
            }
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
            let Some((fin, rsv1, opcode, plen, hdr)) = parse_header(&state.buf) else {
                break;
            };
            if state.buf.len() < hdr + plen {
                break;
            }
            let total = hdr + plen;
            match opcode {
                0x1 | 0x2 => {
                    // Data frame (text / binary). FIN=1 and no pending fragment =
                    // complete message. Otherwise start accumulating.
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen).freeze();
                    let is_compressed = rsv1;
                    if fin && state.fragment_buf.is_none() {
                        let final_payload = if is_compressed {
                            match decompress_message(&mut state, &payload) {
                                Ok(p) => Bytes::from(p),
                                Err(e) => return Err(e),
                            }
                        } else {
                            payload
                        };
                        let msg = Py::new(
                            py,
                            WSMessage {
                                data: final_payload,
                            },
                        )?;
                        Self::deliver_message(py, &mut state, msg)?;
                    } else {
                        // Fragmented message: remember whether RSV1 was on the
                        // first frame; continuation frames don't carry it.
                        let mut acc = BytesMut::with_capacity(plen);
                        acc.extend_from_slice(&payload);
                        state.fragment_buf = Some(acc);
                        state.fragment_opcode = opcode;
                        state.fragment_rsv1 = is_compressed;
                        if fin {
                            let raw = state.fragment_buf.take().unwrap().freeze();
                            let compressed_flag = state.fragment_rsv1;
                            state.fragment_opcode = 0;
                            state.fragment_rsv1 = false;
                            let out = if compressed_flag {
                                Bytes::from(decompress_message(&mut state, &raw)?)
                            } else {
                                raw
                            };
                            let msg = Py::new(py, WSMessage { data: out })?;
                            Self::deliver_message(py, &mut state, msg)?;
                        }
                    }
                }
                0x0 => {
                    // Continuation frame — append to fragment_buf; deliver on FIN.
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen);
                    if let Some(acc) = state.fragment_buf.as_mut() {
                        acc.extend_from_slice(&payload);
                    }
                    if fin {
                        if let Some(acc) = state.fragment_buf.take() {
                            let compressed_flag = state.fragment_rsv1;
                            state.fragment_opcode = 0;
                            state.fragment_rsv1 = false;
                            let raw = acc.freeze();
                            let out = if compressed_flag {
                                Bytes::from(decompress_message(&mut state, &raw)?)
                            } else {
                                raw
                            };
                            let msg = Py::new(py, WSMessage { data: out })?;
                            Self::deliver_message(py, &mut state, msg)?;
                        }
                    }
                }
                0x8 => {
                    // Close: body is [u16 code | reason (utf-8)] per RFC 6455 §5.5.1.
                    state.buf.advance(hdr);
                    let payload = state.buf.split_to(plen);
                    if payload.len() >= 2 {
                        state.close_code = Some(u16::from_be_bytes([payload[0], payload[1]]));
                        if payload.len() > 2 {
                            state.close_reason =
                                Some(String::from_utf8_lossy(&payload[2..]).into_owned());
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
                        let _ = t
                            .bind(py)
                            .call_method1("write", (PyBytes::new(py, &frame),));
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
#[pyo3(signature = (uri, *, headers=None, subprotocols=None, ssl_context=None, connect_timeout=None, receive_timeout=None, proxy=None, compression=false))]
#[allow(clippy::too_many_arguments)]
fn connect<'py>(
    py: Python<'py>,
    uri: String,
    headers: Option<Vec<(String, String)>>,
    subprotocols: Option<Vec<String>>,
    ssl_context: Option<Py<PyAny>>,
    connect_timeout: Option<f64>,
    receive_timeout: Option<f64>,
    proxy: Option<String>,
    compression: bool,
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
            receive_timeout,
            fragment_buf: None,
            fragment_opcode: 0,
            fragment_rsv1: false,
            deflate: if compression { Some(DeflateCtx) } else { None },
        })),
    };
    let state_arc = client.state.clone();
    let client_obj = Py::new(py, client)?;

    // Build handshake bytes + expected accept
    let headers_vec = headers.unwrap_or_default();
    let subprotocols_vec = subprotocols.unwrap_or_default();
    let (req_bytes, expected) = build_handshake(
        &host,
        port,
        &path,
        &headers_vec,
        &subprotocols_vec,
        compression,
    );
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

    // If a proxy is configured, SOCKS5 negotiation happens Python-side inside
    // run_in_executor (see _connect_helper). Otherwise create_connection takes
    // host/port directly.
    let helper = get_connect_helper(py)?;
    let timeout_obj = match connect_timeout {
        Some(t) => t.into_pyobject(py)?.into_any(),
        None => py.None().into_bound(py),
    };
    let proxy_obj = match proxy {
        Some(p) => p.into_pyobject(py)?.into_any().unbind(),
        None => py.None(),
    };
    let ssl_obj: Py<PyAny> = if is_tls { ssl_arg } else { py.None() };
    helper.call1((
        loop_,
        protocol_factory,
        host.clone(),
        port,
        is_tls,
        ssl_obj,
        proxy_obj,
        req_bytes,
        handshake_fut,
        client_obj,
        timeout_obj,
    ))
}

fn parse_ws_uri(uri: &str) -> PyResult<(&'static str, String, u16, String)> {
    let (scheme, rest, default_port): (&str, &str, u16) =
        if let Some(r) = uri.strip_prefix("wss://") {
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
import socket as _socket


def _socks5_connect_blocking(proxy_host, proxy_port, user, password, target_host, target_port):
    """Blocking SOCKS5 CONNECT. Designed to run inside loop.run_in_executor so it
    never blocks the asyncio event loop. Returns a connected, non-blocking socket
    tunnelled through the proxy to (target_host, target_port)."""
    s = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    try:
        s.connect((proxy_host, proxy_port))
        s.setsockopt(_socket.IPPROTO_TCP, _socket.TCP_NODELAY, 1)
        methods = b"\x00" if not user else b"\x00\x02"
        s.sendall(b"\x05" + bytes([len(methods)]) + methods)
        reply = s.recv(2)
        if len(reply) < 2 or reply[0] != 0x05:
            raise ConnectionError("SOCKS5 proxy rejected greeting")
        method = reply[1]
        if method == 0x02:
            if not user:
                raise ConnectionError("SOCKS5 proxy requires auth but none supplied")
            ub, pb = user.encode(), password.encode()
            s.sendall(b"\x01" + bytes([len(ub)]) + ub + bytes([len(pb)]) + pb)
            ar = s.recv(2)
            if len(ar) < 2 or ar[1] != 0x00:
                raise ConnectionError("SOCKS5 auth failed")
        elif method != 0x00:
            raise ConnectionError(f"SOCKS5 proxy selected unsupported method {method}")
        host_b = target_host.encode("idna")
        req = b"\x05\x01\x00\x03" + bytes([len(host_b)]) + host_b + int(target_port).to_bytes(2, "big")
        s.sendall(req)
        hdr = s.recv(4)
        if len(hdr) < 4 or hdr[1] != 0x00:
            raise ConnectionError(f"SOCKS5 CONNECT failed: status={hdr[1] if len(hdr) >= 2 else '?'}")
        atyp = hdr[3]
        if atyp == 0x01:
            s.recv(4)
        elif atyp == 0x03:
            nlen = s.recv(1)[0]
            s.recv(nlen)
        elif atyp == 0x04:
            s.recv(16)
        else:
            raise ConnectionError(f"SOCKS5 returned unsupported ATYP {atyp}")
        s.recv(2)
        s.setblocking(False)
        return s
    except Exception:
        s.close()
        raise


def _parse_proxy_uri(proxy):
    # socks5://[user:password@]host:port
    from urllib.parse import urlsplit, unquote
    parts = urlsplit(proxy)
    if parts.scheme not in ("socks5", "socks5h"):
        raise ValueError(f"Only socks5:// proxies are supported (got {parts.scheme})")
    user = unquote(parts.username) if parts.username else None
    password = unquote(parts.password) if parts.password else ""
    if not parts.hostname or not parts.port:
        raise ValueError("SOCKS5 proxy URI must include host and port")
    return parts.hostname, parts.port, user, password


async def _connect_helper(loop, protocol_factory, host, port, is_tls, ssl_ctx,
                          proxy, req_bytes, handshake_fut, client, connect_timeout):
    async def _do():
        kwargs = {}
        if is_tls:
            kwargs["ssl"] = ssl_ctx
            kwargs["server_hostname"] = host
        if proxy:
            proxy_host, proxy_port, user, password = _parse_proxy_uri(proxy)
            sock = await loop.run_in_executor(
                None, _socks5_connect_blocking,
                proxy_host, proxy_port, user, password, host, port,
            )
            # Hand the already-connected socket to asyncio. TLS (if any) runs
            # on top of it; asyncio will perform the TLS handshake itself.
            kwargs["sock"] = sock
            transport, _proto = await loop.create_connection(protocol_factory, **kwargs)
        else:
            transport, _proto = await loop.create_connection(
                protocol_factory, host, port, **kwargs
            )
            try:
                s = transport.get_extra_info("socket")
                if s is not None:
                    s.setsockopt(_socket.IPPROTO_TCP, _socket.TCP_NODELAY, 1)
            except Exception:
                pass
        transport.write(bytes(req_bytes))
        await handshake_fut
        return client
    if connect_timeout is not None:
        return await _asyncio.wait_for(_do(), timeout=connect_timeout)
    return await _do()
"#;
    let module = PyModule::from_code(
        py,
        std::ffi::CString::new(code)?.as_c_str(),
        c"helper.py",
        c"helper",
    )?;
    let helper = module.getattr("_connect_helper")?;
    let _ = CACHE.set(helper.clone().unbind());
    Ok(helper)
}

pub fn register_native_client(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(py, "native_client")?;
    m.add_class::<NativeClient>()?;
    m.add_class::<WSMessage>()?;
    m.add_function(wrap_pyfunction!(connect, &m)?)?;
    parent.add_submodule(&m)?;
    // Also register in sys.modules so `from websocket_rs.native_client import ...` works.
    let sys_modules = py.import("sys")?.getattr("modules")?;
    sys_modules.set_item("websocket_rs.native_client", &m)?;
    Ok(())
}
