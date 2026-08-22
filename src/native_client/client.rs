//! PyO3 bindings: NativeClient / NativeClientBuffered, the State they
//! share, send-side control-frame encoding, and zero-copy receive helpers.
use std::collections::VecDeque;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use flate2::{Compress, Compression, FlushCompress};
use pyo3::exceptions::{
    PyConnectionError, PyIndexError, PyRuntimeError, PyStopAsyncIteration, PyStopIteration,
    PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PySlice, PyString};
use rand::RngExt;
use std::cell::RefCell;

use super::codec::*;
use super::protocol::*;

pub(crate) struct State {
    pub(crate) transport: Option<Py<PyAny>>,
    pub(crate) buf: BytesMut,
    pub(crate) handshake_done: bool,
    pub(crate) handshake_fut: Option<Py<PyAny>>,
    pub(crate) expected_accept: String,
    pub(crate) pending_recv: VecDeque<Py<PyAny>>,
    pub(crate) backlog: VecDeque<Py<WSMessage>>,
    /// Optional synchronous callback invoked after data_received finishes
    /// parsing — bypasses the Future/await round-trip. Frames are buffered in
    /// `pending_callback_msgs` during parse and dispatched after the parse
    /// loop releases its borrow on State (user callbacks may re-enter via
    /// `ws.send()` etc).
    pub(crate) on_message: Option<Py<PyAny>>,
    pub(crate) pending_callback_msgs: VecDeque<Py<WSMessage>>,
    pub(crate) closed: bool,
    /// asyncio transport has passed its high-water mark — hold off on writes.
    pub(crate) paused: bool,
    /// True when we know asyncio's internal write buffer is empty — lets the
    /// native_sendmsg fast path skip the `transport.get_write_buffer_size()`
    /// Python call. Set after successful native sends and on resume_writing;
    /// cleared whenever we route a write through asyncio.
    pub(crate) buf_known_empty: bool,
    /// Pool of pre-generated mask keys (each entry packs 4 mask bytes as u32).
    /// Refilled in batches of 256 to amortise the rand call. Pop from the back.
    pub(crate) mask_pool: Vec<u32>,
    /// Reusable scratch buffer for send-side frame assembly. Avoids a per-send
    /// `Vec::with_capacity()` allocation in the hot pipelined loop. Mirrors
    /// picows' `_write_buffer` MemoryBuffer.
    pub(crate) send_buf: Vec<u8>,
    /// Reusable receive buffer exposed to uvloop via the BufferedProtocol
    /// `get_buffer` / `buffer_updated` pair. uvloop writes kernel data here
    /// directly, skipping the per-recv `bytes` object allocation that the
    /// plain `data_received` path incurs. Sized to one large frame; grows on
    /// demand if a single recv would overrun. Mirrors picows' `_read_buffer`.
    pub(crate) recv_buf: Vec<u8>,
    /// Write cursor into `recv_buf`. `recv_buf[..recv_pos]` contains data
    /// uvloop has delivered but we haven't fully consumed (i.e. a partial
    /// frame at the tail). `get_buffer` exposes `recv_buf[recv_pos..]` so
    /// kernel writes append; `buffer_updated` advances `recv_pos`, parses
    /// complete frames in place, then compacts the leftover to offset 0.
    pub(crate) recv_pos: usize,
    /// If the previous parse pass ended on a partial frame, holds the total
    /// byte count needed before the next parse pass can yield anything.
    /// Lets `buffer_updated` skip the parse loop entirely for chunks that
    /// can't possibly produce a frame — relevant under TLS where a single
    /// large WS frame is delivered as ~128 × 16 KB asyncio callbacks.
    pub(crate) next_frame_needed: Option<usize>,
    /// Frames buffered while paused; drained on resume_writing.
    pub(crate) write_queue: VecDeque<Py<PyBytes>>,
    /// Cached reference to the asyncio loop — avoids `asyncio.get_running_loop()`
    /// lookups on every recv() slow-path.
    pub(crate) loop_ref: Option<Py<PyAny>>,
    /// `transport.write`, `loop.create_future`, `asyncio.wait_for` cached once
    /// at connect. Hot paths call through these instead of doing attribute
    /// lookup / re-importing `asyncio` per call.
    pub(crate) transport_write: Option<Py<PyAny>>,
    /// `transport.get_write_buffer_size` bound method, cached for the
    /// native-send fast path (we only bypass asyncio when the internal buffer
    /// is already drained).
    pub(crate) transport_get_buf_size: Option<Py<PyAny>>,
    /// Raw socket fd for plain-TCP connections. `None` when the transport is
    /// TLS-wrapped (SSL state machine would be bypassed by raw send) or when
    /// the runtime refused to hand us the underlying socket.
    pub(crate) raw_fd: Option<i32>,
    pub(crate) create_future: Option<Py<PyAny>>,
    pub(crate) wait_for: Option<Py<PyAny>>,
    /// Negotiated subprotocol (Sec-WebSocket-Protocol response value), if any.
    pub(crate) subprotocol: Option<String>,
    /// Close-frame fields (populated after receiving a CLOSE opcode).
    pub(crate) close_code: Option<u16>,
    pub(crate) close_reason: Option<String>,
    /// Optional per-recv timeout (seconds). Applied via asyncio.wait_for wrapper
    /// only when the slow path would block — backlog fast-path skips it.
    pub(crate) receive_timeout: Option<f64>,
    /// Fragmented-message reassembly: accumulates continuation frame payloads
    /// until FIN=1 arrives. First frame's opcode is stashed here.
    pub(crate) fragment_buf: Option<BytesMut>,
    pub(crate) fragment_opcode: u8,
    /// True when the current fragmented message used RSV1 (compressed) in the
    /// first frame — per RFC 7692 the flag is set only on the first frame.
    pub(crate) fragment_rsv1: bool,
    /// permessage-deflate context, lazily initialised after negotiation.
    pub(crate) deflate: Option<DeflateCtx>,
}

impl State {
    #[inline]
    fn protocol_core(&mut self) -> ProtocolCore<'_> {
        ProtocolCore {
            buf: &mut self.buf,
            handshake_done: &mut self.handshake_done,
            expected_accept: &self.expected_accept,
            compression_enabled: self.deflate.is_some(),
            fragment_buf: &mut self.fragment_buf,
            fragment_opcode: &mut self.fragment_opcode,
            fragment_rsv1: &mut self.fragment_rsv1,
        }
    }
}

/// permessage-deflate per-connection state. We always negotiate
/// client_no_context_takeover / server_no_context_takeover so streaming state
/// never persists across messages — the DEFLATE allocators get reset after
/// Pre-completed awaitable. Yields the stored result via StopIteration on first
/// `__next__`, bypassing asyncio.Future entirely. Used by recv() when a message
/// is already available in the backlog — saves one create_future + one set_result
/// per call.
#[pyclass(
    name = "_ReadyMessage",
    module = "websocket_rs.native_client",
    unsendable
)]
pub(crate) struct ReadyMessage {
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

pub(crate) fn ready_ok<'py>(py: Python<'py>, val: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let rm = Bound::new(
        py,
        ReadyMessage {
            result: Some(Ok(val)),
        },
    )?;
    Ok(rm.into_any())
}

pub(crate) fn ready_err<'py>(py: Python<'py>, err: PyErr) -> PyResult<Bound<'py, PyAny>> {
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
/// Owner that keeps a `Py<PyBytes>` alive so a slice into its buffer can be
/// safely returned as `Bytes`. PyBytes is immutable in CPython so the buffer
/// pointer is stable for the object's lifetime.
pub(crate) struct PyBytesOwner {
    _bytes: Py<PyBytes>,
    ptr: *const u8,
    len: usize,
}

// SAFETY: `Py<PyBytes>` is itself Send+Sync per pyo3's contract (refcount ops
// take the GIL). PyBytes is immutable in CPython so the buffer pointer and
// length are stable for the lifetime of the held reference. The Bytes owner
// keeps that reference alive, so no thread can observe a freed pointer.
unsafe impl Send for PyBytesOwner {}
unsafe impl Sync for PyBytesOwner {}

impl AsRef<[u8]> for PyBytesOwner {
    fn as_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

/// Build a zero-copy `Bytes` over `data[start..end]`, keeping `pb` alive as the
/// owner so the slice remains valid.
///
/// Caller must ensure `start <= end <= data.len()` and that `data` actually
/// points into `pb`'s buffer (not a derived/temporary slice).
pub(crate) fn pybytes_zero_copy_slice<'py>(
    _py: Python<'py>,
    pb: &Bound<'py, PyBytes>,
    data: &[u8],
    start: usize,
    end: usize,
) -> Bytes {
    debug_assert!(start <= end, "start ({start}) > end ({end})");
    debug_assert!(end <= data.len(), "end ({end}) > data.len ({})", data.len());
    let ptr = unsafe { data.as_ptr().add(start) };
    let len = end - start;
    let owner = PyBytesOwner {
        _bytes: pb.clone().unbind(),
        ptr,
        len,
    };
    Bytes::from_owner(owner)
}

#[pyclass(name = "WSMessage", module = "websocket_rs.native_client", frozen)]
pub(crate) struct WSMessage {
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

    fn __hash__(&self, py: Python<'_>) -> PyResult<isize> {
        PyBytes::new(py, &self.data).hash()
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
    subclass,
    unsendable
)]
pub(crate) struct NativeClient {
    // Arc + RefCell is intentional: pyclass(unsendable) ensures single-thread
    // access, and we share ownership with PyCFunction closures that capture
    // the state. Send/Sync isn't required since unsendable enforces it via PyO3.
    #[allow(clippy::arc_with_non_send_sync)]
    pub(crate) state: Arc<RefCell<State>>,
}

/// Subclass of `NativeClient` that adds `get_buffer` / `buffer_updated`
/// pymethods. asyncio's transport layer detects these and switches to the
/// BufferedProtocol fast path (no per-recv `bytes` allocation).
///
/// We instantiate this for plain-TCP (`ws://`) connections where the
/// BufferedProtocol path measurably wins (64 KB pipelined: +15% mean vs
/// picows). For TLS (`wss://`) we instantiate the bare `NativeClient`
/// instead — asyncio's SSLProtocol delivers ≤16 KB record-sized chunks
/// and the per-callback PyMemoryView_FromMemory + RefCell churn becomes
/// net-negative (measured: 2 MB TLS +12% with BufferedProtocol off).
#[pyclass(
    name = "NativeClientBuffered",
    module = "websocket_rs.native_client",
    extends = NativeClient,
    unsendable
)]
pub(crate) struct NativeClientBuffered;

#[pymethods]
impl NativeClientBuffered {
    fn get_buffer<'py>(
        self_: PyRef<'_, Self>,
        py: Python<'py>,
        size_hint: isize,
    ) -> PyResult<Bound<'py, PyAny>> {
        self_.as_super().get_buffer_impl(py, size_hint)
    }

    fn buffer_updated(self_: PyRef<'_, Self>, py: Python<'_>, nbytes: usize) -> PyResult<()> {
        self_.as_super().buffer_updated_impl(py, nbytes)
    }
}

/// Best-effort single `send()` syscall. Non-blocking via MSG_DONTWAIT;
/// MSG_NOSIGNAL avoids SIGPIPE on abrupt peer close. Returns the number of
/// bytes actually written or -1 on any error. The caller handles partial
/// writes / errors by falling back to asyncio's transport.write.
#[cfg(unix)]
pub(crate) fn native_send(fd: std::os::unix::io::RawFd, buf: &[u8]) -> isize {
    // MSG_NOSIGNAL is Linux-specific; macOS achieves the same via SO_NOSIGPIPE
    // on the socket (asyncio already sets that when creating the transport on
    // macOS). MSG_DONTWAIT is honoured on both.
    let flags: libc::c_int = {
        #[cfg(target_os = "linux")]
        {
            libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT
        }
        #[cfg(not(target_os = "linux"))]
        {
            libc::MSG_DONTWAIT
        }
    };
    unsafe { libc::send(fd, buf.as_ptr() as *const _, buf.len(), flags) }
}

#[cfg(not(unix))]
pub(crate) fn native_send(_fd: i32, _buf: &[u8]) -> isize {
    -1 // Windows: fall back to transport.write
}

/// Encode a masked control frame (ping=0x9 / pong=0xA). Payload ≤125 bytes per RFC.
pub(crate) fn encode_control_frame(state: &mut State, opcode: u8, payload: &[u8]) -> Vec<u8> {
    let plen = payload.len().min(125);
    let mask = next_mask_key(state);
    let mut out = vec![0u8; 2 + 4 + plen];
    out[0] = 0x80 | opcode;
    out[1] = 0x80 | plen as u8;
    out[2..6].copy_from_slice(&mask);
    copy_masked(&mut out[6..], &payload[..plen], mask);
    out
}

/// Pull a 4-byte WebSocket mask key from the per-connection pool, refilling
/// in batches of 256 to amortise the rand call.
#[inline]
pub(crate) fn next_mask_key(state: &mut State) -> [u8; 4] {
    if state.mask_pool.is_empty() {
        let mut buf = [0u32; 256];
        rand::rng().fill(&mut buf[..]);
        state.mask_pool.extend_from_slice(&buf);
    }
    state.mask_pool.pop().unwrap().to_ne_bytes()
}

#[pymethods]
impl NativeClient {
    // ---- asyncio.Protocol interface ----

    fn connection_made(&self, py: Python<'_>, transport: Py<PyAny>) -> PyResult<()> {
        let tb = transport.bind(py);
        let write = tb.getattr(pyo3::intern!(py, "write"))?.unbind();
        let get_buf_size = tb
            .getattr(pyo3::intern!(py, "get_write_buffer_size"))
            .ok()
            .map(|m| m.unbind());

        // Borrow the socket fd for the native-send fast path, BUT only when
        // the transport is plain TCP. If an SSL object is present the send path
        // must go through the TLS layer, so we leave raw_fd = None.
        let ssl_obj = tb.call_method1("get_extra_info", ("ssl_object",))?;
        let fd = if ssl_obj.is_none() {
            let sock = tb.call_method1("get_extra_info", ("socket",))?;
            if sock.is_none() {
                None
            } else {
                sock.call_method0("fileno")?
                    .extract::<i32>()
                    .ok()
                    .filter(|&f| f >= 0)
            }
        } else {
            None
        };

        // Tune the socket: asyncio sets TCP_NODELAY by default, but TCP_QUICKACK
        // must be set explicitly on Linux to disable delayed-ACK. Without it,
        // pipelined throughput at medium frame sizes (8-32 KiB) is throttled by
        // the 40 ms ACK delay timer. Mirrors picows' connection_made
        // (picows.pyx:956-958).
        #[cfg(target_os = "linux")]
        if let Some(f) = fd {
            unsafe {
                let on: libc::c_int = 1;
                libc::setsockopt(
                    f,
                    libc::IPPROTO_TCP,
                    libc::TCP_NODELAY,
                    &on as *const _ as *const _,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
                libc::setsockopt(
                    f,
                    libc::IPPROTO_TCP,
                    libc::TCP_QUICKACK,
                    &on as *const _ as *const _,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }

        let mut s = self.state.borrow_mut();
        s.transport = Some(transport);
        s.transport_write = Some(write);
        s.transport_get_buf_size = get_buf_size;
        s.raw_fd = fd;
        Ok(())
    }

    fn data_received<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<()> {
        // Try PyBytes zero-copy fast path (asyncio.Protocol gives PyBytes which
        // is immutable, so wrapping its buffer as a Bytes owner avoids the
        // per-frame memcpy). Fall back to slice extraction otherwise.
        let result = if let Ok(pb) = data.cast::<PyBytes>() {
            let bytes = pb.as_bytes();
            self.data_received_inner_pybytes(py, pb, bytes)
        } else {
            let buf = pyo3::buffer::PyBuffer::<u8>::get(data)?;
            // Reject non-contiguous or multi-dimensional buffers — treating
            // them as flat slices via from_raw_parts would mis-read strided
            // numpy views and similar layouts.
            if !buf.is_c_contiguous() || buf.dimensions() != 1 {
                return Err(PyTypeError::new_err(
                    "data_received expected a 1-D C-contiguous buffer",
                ));
            }
            let slice: &[u8] =
                unsafe { std::slice::from_raw_parts(buf.buf_ptr() as *const u8, buf.item_count()) };
            self.data_received_inner(py, slice)
        };
        self.flush_pending_callbacks(py)?;
        result
    }

    /// Called by asyncio transport when its send buffer crosses the high-water mark.
    /// We stop draining into the transport; subsequent send() calls buffer internally.
    fn pause_writing(&self) {
        let mut s = self.state.borrow_mut();
        s.paused = true;
        s.buf_known_empty = false;
    }

    /// Called when the transport drains below the low-water mark. Flush whatever we
    /// queued while paused, then clear the flag.
    fn resume_writing(&self, py: Python<'_>) -> PyResult<()> {
        let mut state = self.state.borrow_mut();
        state.paused = false;
        // After resume, asyncio's buffer is below low-water but not necessarily
        // empty — leave buf_known_empty alone (it'll be set true again when the
        // next native_send sees an empty buffer via get_write_buffer_size, or
        // explicitly tracked here). Conservative: leave false.
        state.buf_known_empty = false;
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

    /// The peer half-closed (TCP FIN, or TLS close_notify). No further frame can
    /// arrive, so let the transport close itself and let `connection_lost` do the
    /// cleanup: returning true would mean "the protocol keeps this open", which is
    /// wrong for a WebSocket client and is ignored under TLS anyway.
    ///
    /// asyncio.Protocol supplies a default, but this class is a pyclass and is not
    /// a subclass of it, so the method has to exist here. Both call sites are
    /// unconditional and unguarded (`sslproto._call_eof_received`,
    /// `_SelectorSocketTransport._read_ready__data_received`), and the TLS one
    /// routes a missing attribute into `_fatal_error`.
    fn eof_received(&self) -> bool {
        false
    }

    fn connection_lost(&self, py: Python<'_>, _exc: Py<PyAny>) {
        let pending = {
            let mut state = self.state.borrow_mut();
            state.closed = true;
            state.transport = None;
            std::mem::take(&mut state.pending_recv)
        };
        Self::fail_pending(py, pending, "Connection lost");
    }

    // ---- User-facing API ----

    /// Encode a single binary frame and write it directly to the transport.
    /// Zero-copy: the encoded frame is materialised straight into Python memory.
    fn send(&self, py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<()> {
        // Hold a single borrow_mut throughout. asyncio's transport.write /
        // get_write_buffer_size are internal Python calls that don't re-enter
        // our methods, so it's safe. Saves ~5 separate borrow ops per send.
        let mut st = self.state.borrow_mut();
        if st.closed {
            return Err(PyRuntimeError::new_err("WebSocket is closed"));
        }
        if !st.handshake_done {
            return Err(PyRuntimeError::new_err("WebSocket handshake not complete"));
        }
        let write = st
            .transport_write
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("No transport"))?
            .clone_ref(py);
        let raw_fd = st.raw_fd;

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
        let compressed = st.deflate.is_some();
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
                let out_before = comp.total_out();
                comp.compress_vec(&raw_payload[cursor..], &mut out, FlushCompress::Sync)
                    .map_err(|e| PyRuntimeError::new_err(format!("deflate error: {e}")))?;
                cursor += (comp.total_in() - in_before) as usize;
                if cursor >= raw_payload.len() && out.ends_with(&[0x00, 0x00, 0xFF, 0xFF]) {
                    break;
                }
                if cursor >= raw_payload.len() && (comp.total_out() - out_before) == 0 {
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

        let plen = payload.len();
        let header_len =
            2 + match plen {
                0..=125 => 0,
                126..=65535 => 2,
                _ => 8,
            } + 4; // 4-byte mask

        let mask_key = next_mask_key(&mut st);

        // Encode header onto the stack (max 14 bytes: 2 + 8 length + 4 mask).
        let mut header_buf = [0u8; 14];
        header_buf[0] = 0x80 | opcode | if compressed { 0x40 } else { 0x00 };
        let mut pos = 2;
        if plen <= 125 {
            header_buf[1] = 0x80 | plen as u8;
        } else if plen <= 65535 {
            header_buf[1] = 0x80 | 126;
            header_buf[2..4].copy_from_slice(&(plen as u16).to_be_bytes());
            pos = 4;
        } else {
            header_buf[1] = 0x80 | 127;
            header_buf[2..10].copy_from_slice(&(plen as u64).to_be_bytes());
            pos = 10;
        }
        header_buf[pos..pos + 4].copy_from_slice(&mask_key);
        let header = &header_buf[..pos + 4];
        debug_assert_eq!(header.len(), header_len);

        if st.paused {
            let out = self.build_merged_frame(py, header, payload, mask_key)?;
            st.write_queue.push_back(out.unbind());
            return Ok(());
        }

        // Native send fast path. The buf_known_empty cache lets us skip the
        // Python call to get_write_buffer_size() in the steady state.
        if let Some(fd) = raw_fd {
            let drained = if st.buf_known_empty {
                true
            } else {
                // Lazy: clone the get_buf_size method only on cache miss.
                let truth = match st.transport_get_buf_size.as_ref() {
                    Some(m) => m
                        .bind(py)
                        .call0()
                        .and_then(|v| v.extract::<isize>())
                        .map(|n| n == 0)
                        .unwrap_or(false),
                    None => false,
                };
                if truth {
                    st.buf_known_empty = true;
                }
                truth
            };
            if drained {
                let total = header.len() + plen;
                // Grow-only scratch buffer: `resize` zeroes only the bytes added
                // when a larger frame arrives, and the steady state reuses the
                // same allocation without touching it. `send_buf[..total]` is
                // the live frame; anything past it is stale and never sent.
                if st.send_buf.len() < total {
                    st.send_buf.resize(total, 0);
                }
                let (hdr_dst, payload_dst) = st.send_buf[..total].split_at_mut(header.len());
                hdr_dst.copy_from_slice(header);
                copy_masked(payload_dst, payload, mask_key);
                let written = native_send(fd, &st.send_buf[..total]);
                if written == total as isize {
                    return Ok(());
                }
                if written > 0 {
                    let n = written as usize;
                    let tail = PyBytes::new(py, &st.send_buf[n..total]);
                    st.buf_known_empty = false;
                    drop(st);
                    write.bind(py).call1((tail,))?;
                    return Ok(());
                }
            }
        }
        let out = self.build_merged_frame(py, header, payload, mask_key)?;
        st.buf_known_empty = false;
        drop(st);
        write.bind(py).call1((out,))?;
        Ok(())
    }

    /// Returns an asyncio.Future that completes with the next received frame payload.
    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut state = self.state.borrow_mut();
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
        // Slow path: use cached loop.create_future + optional asyncio.wait_for.
        let create_future = state
            .create_future
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Event loop not bound"))?
            .clone_ref(py);
        let wait_for_cached = state.wait_for.as_ref().map(|w| w.clone_ref(py));
        let timeout = state.receive_timeout;
        drop(state);
        let fut = create_future.bind(py).call0()?;
        self.state
            .borrow_mut()
            .pending_recv
            .push_back(fut.clone().unbind());
        if let (Some(t), Some(wait_for)) = (timeout, wait_for_cached) {
            return wait_for.bind(py).call1((fut, t));
        }
        Ok(fut)
    }

    #[getter]
    fn is_open(&self) -> bool {
        let s = self.state.borrow();
        s.handshake_done && !s.closed && s.transport.is_some()
    }

    // ---- Async iteration: `async for msg in ws` ----
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Async iterator step. Returns the next WSMessage; raises StopAsyncIteration
    /// when the connection is closed (vs recv() which raises ConnectionError).
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut state = self.state.borrow_mut();
        if let Some(payload) = state.backlog.pop_front() {
            drop(state);
            return ready_ok(py, payload.into_any());
        }
        if state.closed {
            drop(state);
            return ready_err(py, PyStopAsyncIteration::new_err("Connection closed"));
        }
        let create_future = state
            .create_future
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Event loop not bound"))?
            .clone_ref(py);
        let wait_for_cached = state.wait_for.as_ref().map(|w| w.clone_ref(py));
        let timeout = state.receive_timeout;
        drop(state);
        let fut = create_future.bind(py).call0()?;
        self.state
            .borrow_mut()
            .pending_recv
            .push_back(fut.clone().unbind());
        if let (Some(t), Some(wait_for)) = (timeout, wait_for_cached) {
            return wait_for.bind(py).call1((fut, t));
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
        self.state.borrow().subprotocol.clone()
    }

    #[getter]
    fn close_code(&self) -> Option<u16> {
        self.state.borrow().close_code
    }

    #[getter]
    fn close_reason(&self) -> Option<String> {
        self.state.borrow().close_reason.clone()
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
        let mut state = self.state.borrow_mut();
        if state.closed {
            return Err(PyRuntimeError::new_err("WebSocket is closed"));
        }
        let transport = state
            .transport
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("No transport"))?
            .clone_ref(py);
        let frame = encode_control_frame(&mut state, OP_PING, &payload);
        drop(state);
        transport
            .bind(py)
            .call_method1("write", (PyBytes::new(py, &frame),))?;
        Ok(())
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let mut state = self.state.borrow_mut();
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        // Pull Py refs out and drop them at the end of this call so the event
        // loop sees the transport's refcount go to zero promptly. Some loop
        // implementations (rloop 0.2) wedge on subsequent connects if these
        // references linger.
        let transport = state.transport.take();
        state.transport_write = None;
        state.loop_ref = None;
        state.on_message = None;
        state.create_future = None;
        state.wait_for = None;
        let write_queue = std::mem::take(&mut state.write_queue);
        let pending = std::mem::take(&mut state.pending_recv);
        drop(state);
        Self::fail_pending(py, pending, "Connection closed by client");
        // All mutex-guarded references are gone; drop pending writes and then
        // issue the close frame + transport.close() on the surviving transport ref.
        drop(write_queue);
        if let Some(t) = transport {
            let close_frame: [u8; 6] = [
                0x88, 0x80, // FIN | opcode=8, masked, length=0
                0, 0, 0, 0, // mask key (payload empty so mask value immaterial)
            ];
            let tb = t.bind(py);
            let _ = tb.call_method1("write", (PyBytes::new(py, &close_frame),));
            let _ = tb.call_method0("close");
        }
        Ok(())
    }
}

// Helpers (non-pymethod)
impl NativeClient {
    /// Parse `data` (a window into `recv_buf`) in place. Returns the number
    /// of bytes consumed; the caller compacts the rest. The fast path
    /// (handshake done, no fragment in flight, no deflate state, no carry-
    /// over `buf`) parses straight from `data` without copying. The
    /// slow path (handshake-in-progress / fragmented / compressed) routes
    /// through `data_received_inner` which uses `buf` as its working
    /// area — in that case we report the full window as consumed.
    /// Returns `(consumed, next_frame_needed)`. `next_frame_needed` is
    /// `Some(N)` when parse stopped on a partial frame: `recv_pos` (after
    /// caller's compaction) must reach `N` before the next parse pass can
    /// complete that frame. Caller writes it onto State once, outside any
    /// per-frame borrow churn.
    /// True when frames arriving now are frame-aligned: handshake done,
    /// nothing parked in `buf`, no fragment assembly in flight. Only then
    /// may a caller scan incoming bytes directly instead of parking them.
    fn fast_path_eligible(&self) -> bool {
        let st = self.state.borrow();
        st.handshake_done && st.buf.is_empty() && st.fragment_buf.is_none()
    }

    /// Single pass over frame-aligned `data` — THE opcode dispatch shared by
    /// every receive path: deliver TEXT/BINARY per `mode`, queue one masked
    /// pong per unfragmented PING, stop at the first peer CLOSE.
    ///
    /// Borrow discipline: queued pongs are written and peer-close effects
    /// applied only AFTER the State borrow is released, because transport
    /// writes and close bookkeeping may re-enter the client (send(), recv()
    /// resolution). Flushing pings before applying the close also keeps the
    /// wire order "pong, then close" for a batch ending in CLOSE, matching
    /// the ProtocolCore slow path's event order.
    fn scan_frame_aligned(
        &self,
        py: Python<'_>,
        data: &[u8],
        mode: PayloadMode<'_, '_>,
    ) -> PyResult<ScanOutcome> {
        let mut state = self.state.borrow_mut();
        let mut close_effects = None;
        let mut pongs: Vec<(Py<PyAny>, Vec<u8>)> = Vec::new();
        let outcome = walk_frames(data, |frame| -> PyResult<VisitOutcome> {
            match frame.opcode {
                OP_TEXT | OP_BINARY => {
                    let payload = match mode {
                        PayloadMode::Copy => Bytes::copy_from_slice(frame.payload),
                        PayloadMode::ZeroCopy { pb } => {
                            // PyBytes is immutable, so the pointer stays
                            // valid as long as the PyBytesOwner refcount
                            // keeps it alive.
                            pybytes_zero_copy_slice(
                                py,
                                pb,
                                data,
                                frame.payload_start,
                                frame.payload_start + frame.payload.len(),
                            )
                        }
                    };
                    let msg = Py::new(py, WSMessage { data: payload })?;
                    Self::deliver_message(py, &mut state, msg)?;
                }
                OP_PING => {
                    let transport = state.transport.as_ref().map(|t| t.clone_ref(py));
                    if let Some(transport) = transport {
                        let pong_frame = encode_control_frame(&mut state, OP_PONG, frame.payload);
                        pongs.push((transport, pong_frame));
                    }
                }
                OP_CLOSE => {
                    let (code, reason) = parse_close_payload(frame.payload);
                    close_effects = Some(Self::begin_peer_close(py, &mut state, code, reason));
                    return Ok(VisitOutcome::Stop);
                }
                _ => {}
            }
            Ok(VisitOutcome::Continue)
        })?;
        drop(state);
        // Answer queued pings once the State borrow is released.
        for (transport, pong_frame) in pongs {
            let _ = transport
                .bind(py)
                .call_method1("write", (PyBytes::new(py, &pong_frame),));
        }
        if let ScanOutcome::Stopped { .. } = outcome {
            if let Some((pending, transport)) = close_effects {
                Self::apply_peer_close(py, pending, transport);
            }
        }
        Ok(outcome)
    }

    /// Shared epilogue of the callback-driven receive paths: park any
    /// unconsumed tail in State.buf and re-drain it through the slow path.
    fn park_tail_and_drain(
        &self,
        py: Python<'_>,
        outcome: ScanOutcome,
        data: &[u8],
    ) -> PyResult<()> {
        let consumed = match outcome {
            ScanOutcome::Stopped { .. } => return Ok(()),
            ScanOutcome::Exhausted { consumed } if consumed == data.len() => return Ok(()),
            ScanOutcome::Exhausted { consumed }
            | ScanOutcome::Partial { consumed, .. }
            | ScanOutcome::Fallback { consumed } => consumed,
        };
        if consumed < data.len() {
            self.state
                .borrow_mut()
                .buf
                .extend_from_slice(&data[consumed..]);
            return self.process_buffered_frames(py);
        }
        Ok(())
    }

    /// Parse `data` (a window into `recv_buf`) in place; the caller compacts
    /// the remainder. Frame-aligned windows take the shared fast-path scan;
    /// anything else routes through `data_received_inner`, which reports the
    /// full window as consumed because it parked the bytes itself.
    /// Returns `(consumed, next_frame_needed)`; `Some(N)` means the caller's
    /// `recv_pos` must reach `N` before the next parse pass can finish the
    /// partial frame.
    fn parse_recv_data(&self, py: Python<'_>, data: &[u8]) -> PyResult<(usize, Option<usize>)> {
        if !self.fast_path_eligible() {
            self.data_received_inner(py, data)?;
            return Ok((data.len(), None));
        }
        let outcome = self.scan_frame_aligned(py, data, PayloadMode::Copy)?;
        match outcome {
            ScanOutcome::Exhausted { consumed } | ScanOutcome::Stopped { consumed } => {
                Ok((consumed, None))
            }
            ScanOutcome::Partial { consumed, needed } => Ok((consumed, Some(needed))),
            ScanOutcome::Fallback { consumed } => {
                self.data_received_inner(py, &data[consumed..])?;
                Ok((data.len(), None))
            }
        }
    }

    fn data_received_inner_pybytes<'py>(
        &self,
        py: Python<'py>,
        pb: &Bound<'py, PyBytes>,
        data: &[u8],
    ) -> PyResult<()> {
        if self.fast_path_eligible() {
            let outcome = self.scan_frame_aligned(py, data, PayloadMode::ZeroCopy { pb })?;
            return self.park_tail_and_drain(py, outcome, data);
        }
        self.state.borrow_mut().buf.extend_from_slice(data);
        self.process_buffered_frames(py)
    }

    fn data_received_inner(&self, py: Python<'_>, data: &[u8]) -> PyResult<()> {
        if self.fast_path_eligible() {
            let outcome = self.scan_frame_aligned(py, data, PayloadMode::Copy)?;
            return self.park_tail_and_drain(py, outcome, data);
        }
        // Slow path: handshake in progress or buf already holds partial frame
        // data (fragment / compression). process_buffered_frames has the full
        // handshake parse including subprotocol + extension negotiation.
        self.state.borrow_mut().buf.extend_from_slice(data);
        self.process_buffered_frames(py)
    }

    /// Drain pending_callback_msgs and invoke the user callback for each.
    /// Must be called with no outstanding borrow on State.
    fn flush_pending_callbacks(&self, py: Python<'_>) -> PyResult<()> {
        loop {
            // Pop one message at a time; the user callback may push new frames
            // (e.g. by triggering re-entrant data_received) — unlikely on
            // single-thread asyncio but cheap to handle.
            let (cb, msg) = {
                let mut st = self.state.borrow_mut();
                if st.pending_callback_msgs.is_empty() {
                    return Ok(());
                }
                let msg = match st.pending_callback_msgs.pop_front() {
                    Some(m) => m,
                    None => return Ok(()),
                };
                let cb = match st.on_message.as_ref() {
                    Some(c) => c.clone_ref(py),
                    None => return Ok(()),
                };
                (cb, msg)
            };
            cb.bind(py).call1((msg,))?;
        }
    }

    /// Build a single PyBytes containing header + masked payload — used on the
    /// slow path (paused transport, no raw fd, or sendmsg fallback).
    fn build_merged_frame<'py>(
        &self,
        py: Python<'py>,
        header: &[u8],
        payload: &[u8],
        mask_key: [u8; 4],
    ) -> PyResult<Bound<'py, PyBytes>> {
        let total = header.len() + payload.len();
        PyBytes::new_with(py, total, |buf| {
            let (hdr_dst, payload_dst) = buf.split_at_mut(header.len());
            hdr_dst.copy_from_slice(header);
            copy_masked(payload_dst, payload, mask_key);
            Ok(())
        })
    }

    /// asyncio BufferedProtocol implementation. Lives on NativeClient so the
    /// `NativeClientBuffered` subclass can forward to it. Returns a writable
    /// `recv_buf[recv_pos..capacity]` slice via `PyMemoryView_FromMemory`;
    /// uvloop fills it with kernel data, then calls `buffer_updated_impl`.
    fn get_buffer_impl<'py>(
        &self,
        py: Python<'py>,
        _size_hint: isize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut st = self.state.borrow_mut();
        const HEADROOM: usize = 65536;
        let need = st.recv_pos + HEADROOM;
        if st.recv_buf.capacity() < need {
            let extra = need - st.recv_buf.capacity();
            st.recv_buf.reserve(extra);
        }
        let cap = st.recv_buf.capacity();
        // SAFETY: bytes between len() and capacity() are uninitialized but
        // we only ever *read* the first `nbytes` past `recv_pos` — and only
        // after `buffer_updated_impl(nbytes)` confirms uvloop wrote them.
        unsafe {
            st.recv_buf.set_len(cap);
        }
        let recv_pos = st.recv_pos;
        let ptr = unsafe { st.recv_buf.as_mut_ptr().add(recv_pos) };
        let avail = cap - recv_pos;
        drop(st);
        unsafe {
            let mv = pyo3::ffi::PyMemoryView_FromMemory(
                ptr as *mut std::ffi::c_char,
                avail as pyo3::ffi::Py_ssize_t,
                pyo3::ffi::PyBUF_WRITE,
            );
            if mv.is_null() {
                return Err(PyErr::fetch(py));
            }
            Ok(Bound::from_owned_ptr(py, mv))
        }
    }

    fn buffer_updated_impl(&self, py: Python<'_>, nbytes: usize) -> PyResult<()> {
        // Defer-parse gate: if the next frame still needs more bytes than
        // recv_pos has, skip the parse pass entirely.
        let (ptr, total) = {
            let mut st = self.state.borrow_mut();
            st.recv_pos += nbytes;
            if let Some(needed) = st.next_frame_needed {
                if st.recv_pos < needed {
                    return Ok(());
                }
                st.next_frame_needed = None;
            }
            (st.recv_buf.as_ptr(), st.recv_pos)
        };
        // SAFETY: `recv_buf` is not realloc'd during parsing — only
        // `protocol.buf` / `state.backlog` / `state.pending_callback_msgs` get
        // mutated. Pointer stays valid for the slice's lifetime.
        let data = unsafe { std::slice::from_raw_parts(ptr, total) };
        let (consumed, needed) = self.parse_recv_data(py, data)?;
        {
            let mut st = self.state.borrow_mut();
            let recv_pos = st.recv_pos;
            if consumed > 0 && recv_pos > consumed {
                st.recv_buf.copy_within(consumed..recv_pos, 0);
            }
            st.recv_pos = recv_pos.saturating_sub(consumed);
            st.next_frame_needed = needed;
        }
        self.flush_pending_callbacks(py)
    }

    /// Parse and dispatch all complete frames currently sitting in State::buf.
    /// Also completes the HTTP/101 handshake on first invocation.
    #[cold]
    fn process_buffered_frames(&self, py: Python<'_>) -> PyResult<()> {
        let handshake = { self.state.borrow_mut().protocol_core().process_handshake() };
        match handshake {
            HandshakeOutcome::Pending => return Ok(()),
            HandshakeOutcome::Rejected => {
                let future = { self.state.borrow_mut().handshake_fut.take() };
                if let Some(future) = future {
                    let error = PyConnectionError::new_err("WebSocket handshake failed");
                    let _ = future.bind(py).call_method1("set_exception", (error,));
                }
                return Ok(());
            }
            HandshakeOutcome::Accepted {
                subprotocol,
                compression_enabled,
            } => {
                let future = {
                    let mut state = self.state.borrow_mut();
                    state.handshake_done = true;
                    state.subprotocol = subprotocol;
                    if !compression_enabled {
                        state.deflate = None;
                    }
                    state.handshake_fut.take()
                };
                if let Some(future) = future {
                    // Pre-refactor behavior: a set_result failure here must
                    // not abort processing of queued protocol events.
                    let _ = Self::set_future_result(py, future.bind(py), py.None());
                }
            }
            HandshakeOutcome::Complete => {}
        }

        match emit_protocol_events(
            || self.state.borrow_mut().protocol_core().next_event(),
            |event| self.handle_protocol_event(py, event),
        ) {
            Ok(()) => Ok(()),
            Err(EmitError::Core(error)) => Err(PyRuntimeError::new_err(error.0)),
            Err(EmitError::Sink(error)) => Err(error),
        }
    }

    #[cold]
    fn handle_protocol_event(&self, py: Python<'_>, event: ProtocolEvent) -> PyResult<EventFlow> {
        match event {
            ProtocolEvent::Message(payload) => {
                let message = Py::new(py, WSMessage { data: payload })?;
                let pending = {
                    let mut state = self.state.borrow_mut();
                    if state.on_message.is_some() {
                        state.pending_callback_msgs.push_back(message);
                        None
                    } else if let Some(future) = state.pending_recv.pop_front() {
                        Some((future, message))
                    } else {
                        state.backlog.push_back(message);
                        None
                    }
                };
                if let Some((future, message)) = pending {
                    Self::set_future_result(py, future.bind(py), message.into_any())?;
                }
                Ok(EventFlow::Continue)
            }
            ProtocolEvent::SendPong(payload) => {
                let write = {
                    let mut state = self.state.borrow_mut();
                    let transport = state.transport.as_ref().map(|t| t.clone_ref(py));
                    transport.map(|transport| {
                        let frame = encode_control_frame(&mut state, OP_PONG, &payload);
                        (transport, frame)
                    })
                };
                if let Some((transport, frame)) = write {
                    let _ = transport
                        .bind(py)
                        .call_method1("write", (PyBytes::new(py, &frame),));
                }
                Ok(EventFlow::Continue)
            }
            ProtocolEvent::Close { code, reason } => {
                let (pending, transport) = {
                    let mut state = self.state.borrow_mut();
                    Self::begin_peer_close(py, &mut state, code, reason)
                };
                Self::apply_peer_close(py, pending, transport);
                Ok(EventFlow::Stop)
            }
            ProtocolEvent::ProtocolError(reason) => {
                let (pending, transport, frame) = {
                    let mut state = self.state.borrow_mut();
                    Self::begin_protocol_error(py, &mut state, reason)
                };
                Self::fail_pending(py, pending, reason);
                if let Some(transport) = transport {
                    let transport = transport.bind(py);
                    let _ = transport.call_method1("write", (PyBytes::new(py, &frame),));
                    let _ = transport.call_method0("close");
                }
                Ok(EventFlow::Stop)
            }
        }
    }

    /// Resolve `future` with `value` unless it reports done. Policy shared
    /// by every set_result site: when the done() probe itself errors,
    /// resolve anyway — losing a message is worse than asyncio's
    /// InvalidStateError on a genuinely-settled future, and this path is
    /// allowed to raise.
    fn set_future_result(
        py: Python<'_>,
        future: &Bound<'_, PyAny>,
        value: Py<PyAny>,
    ) -> PyResult<()> {
        if !future
            .call_method0(pyo3::intern!(py, "done"))?
            .extract::<bool>()
            .unwrap_or(false)
        {
            future.call_method1(pyo3::intern!(py, "set_result"), (value,))?;
        }
        Ok(())
    }

    /// Fail `future` unless it reports done. Teardown policy, deliberately
    /// the opposite default of `set_future_result`: when the probe errors,
    /// assume settled and skip, and swallow the set_exception result —
    /// cleanup must not throw over an already-dead connection.
    fn set_future_exception(py: Python<'_>, future: &Bound<'_, PyAny>, error: PyErr) {
        if !future
            .call_method0(pyo3::intern!(py, "done"))
            .and_then(|done| done.extract::<bool>())
            .unwrap_or(true)
        {
            let _ = future.call_method1("set_exception", (error,));
        }
    }

    fn fail_pending(py: Python<'_>, mut pending: VecDeque<Py<PyAny>>, msg: &str) {
        while let Some(future) = pending.pop_front() {
            let error = PyConnectionError::new_err(msg.to_string());
            Self::set_future_exception(py, future.bind(py), error);
        }
    }

    fn deliver_message(py: Python<'_>, state: &mut State, msg: Py<WSMessage>) -> PyResult<()> {
        // Callback-style fast path: defer invocation until after the parse
        // loop has released its borrow on State. The user callback may call
        // back into us (e.g. ws.send(...)), which would re-enter borrow_mut().
        if state.on_message.is_some() {
            state.pending_callback_msgs.push_back(msg);
            return Ok(());
        }
        if let Some(fut) = state.pending_recv.pop_front() {
            Self::set_future_result(py, fut.bind(py), msg.into_any())?;
        } else {
            state.backlog.push_back(msg);
        }
        Ok(())
    }

    /// Record a local protocol-error close in `state` and hand back the
    /// effects the caller must apply AFTER releasing the State borrow — the
    /// same reentrancy discipline as `begin_peer_close`, plus the 1002 close
    /// frame this end puts on the wire because the peer did not send one.
    fn begin_protocol_error(
        py: Python<'_>,
        state: &mut State,
        reason: &str,
    ) -> (VecDeque<Py<PyAny>>, Option<Py<PyAny>>, Vec<u8>) {
        state.close_code = Some(1002);
        state.close_reason = Some(reason.to_string());
        state.closed = true;
        let pending = std::mem::take(&mut state.pending_recv);
        let transport = state.transport.as_ref().map(|t| t.clone_ref(py));
        let frame = encode_control_frame(state, OP_CLOSE, &1002u16.to_be_bytes());
        (pending, transport, frame)
    }

    /// Record a peer-initiated close in `state` and hand back the effects the
    /// caller must apply AFTER releasing the State borrow — the same
    /// reentrancy discipline as the slow-path event sink.
    fn begin_peer_close(
        py: Python<'_>,
        state: &mut State,
        code: Option<u16>,
        reason: Option<String>,
    ) -> (VecDeque<Py<PyAny>>, Option<Py<PyAny>>) {
        if code.is_some() {
            state.close_code = code;
            state.close_reason = reason;
        }
        state.closed = true;
        let pending = std::mem::take(&mut state.pending_recv);
        let transport = state.transport.as_ref().map(|t| t.clone_ref(py));
        (pending, transport)
    }

    fn apply_peer_close(
        py: Python<'_>,
        pending: VecDeque<Py<PyAny>>,
        transport: Option<Py<PyAny>>,
    ) {
        Self::fail_pending(py, pending, "Connection closed by peer");
        if let Some(transport) = transport {
            let _ = transport.bind(py).call_method0("close");
        }
    }
}
