//! Frame codec primitives: AVX-512/AVX2 masking kernels, WebSocket header
//! parsing, and the shared frame walker used by the frame-aligned receive
//! fast paths (ProtocolCore::next_event keeps its own walk for fragmented
//! and compressed traffic).
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// WebSocket opcodes per RFC 6455 §5.2.
pub(crate) const OP_CONTINUATION: u8 = 0x0;
pub(crate) const OP_TEXT: u8 = 0x1;
pub(crate) const OP_BINARY: u8 = 0x2;
pub(crate) const OP_CLOSE: u8 = 0x8;
pub(crate) const OP_PING: u8 = 0x9;
pub(crate) const OP_PONG: u8 = 0xA;

/// CPU feature detection is cached (one `cpuid` per process) via OnceLock.
#[cfg(target_arch = "x86_64")]
pub(crate) fn has_avx512f() -> bool {
    use std::sync::OnceLock;
    static DETECTED: OnceLock<bool> = OnceLock::new();
    *DETECTED.get_or_init(|| std::is_x86_feature_detected!("avx512f"))
}

/// Copy `src` into `dst` while applying the 4-byte XOR mask in the same pass.
///
/// Equivalent to `dst.copy_from_slice(src)` followed by an in-place XOR, but touches
/// each byte once instead of twice, which is what matters for outbound frames
/// large enough to leave L2. `mask[0]` aligns with `src[0]`, matching the RFC
/// 6455 rule that masking is indexed from the start of the payload.
#[inline]
pub(crate) fn copy_masked(dst: &mut [u8], src: &[u8], mask: [u8; 4]) {
    // Load-bearing for soundness, not a sanity check: the AVX-512 kernel reads
    // `src` for `dst.len()` bytes, so a shorter `src` would read out of bounds.
    // The panic is a cold out-of-line call, leaving one predictable compare on
    // the hot path.
    if dst.len() != src.len() {
        length_mismatch(dst.len(), src.len());
    }
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx512f() {
            unsafe { copy_masked_avx512(dst, src, mask) };
            return;
        }
    }
    copy_masked_fallback(dst, src, mask);
}

#[cold]
#[inline(never)]
pub(crate) fn length_mismatch(dst_len: usize, src_len: usize) -> ! {
    panic!("copy_masked requires equal lengths: dst={dst_len} src={src_len}");
}

/// Scalar u32 XOR-copy. Both slices are read/written unaligned in 4-byte steps,
/// which rustc auto-vectorises to 16-/32-byte XOR on any x86-64 baseline; even
/// unvectorised it beats a byte-at-a-time loop ~4x.
#[inline]
pub(crate) fn copy_masked_fallback(dst: &mut [u8], src: &[u8], mask: [u8; 4]) {
    let mask_u32 = u32::from_ne_bytes(mask);
    let words = dst.len() / 4;
    let (dw, dtail) = dst.split_at_mut(words * 4);
    let (sw, stail) = src.split_at(words * 4);
    for (d, s) in dw
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(sw.as_chunks::<4>().0)
    {
        let v = u32::from_ne_bytes(*s) ^ mask_u32;
        d.copy_from_slice(&v.to_ne_bytes());
    }
    for (i, (d, s)) in dtail.iter_mut().zip(stail.iter()).enumerate() {
        *d = *s ^ mask[i & 3];
    }
}

/// AVX-512 XOR-copy — 64 bytes per load/xor/store. Unaligned access carries no
/// penalty on AVX-512, and the trailing bytes reuse the scalar path.
///
/// SAFETY: two preconditions, both established by `copy_masked`:
/// - AVX-512F is available on this CPU (checked via the cached `has_avx512f()`);
/// - `dst.len() == src.len()`, because the loop bound comes from `dst` while the
///   loads come from `src` (asserted, not merely debug-asserted, in the wrapper).
///
/// `dst` and `src` are distinct slices — Rust's borrow rules guarantee it at
/// every call site — so the stores cannot alias the loads.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn copy_masked_avx512(dst: &mut [u8], src: &[u8], mask: [u8; 4]) {
    use std::arch::x86_64::*;
    let mask_vec = _mm512_set1_epi32(u32::from_ne_bytes(mask) as i32);
    let len = dst.len();
    let dptr = dst.as_mut_ptr();
    let sptr = src.as_ptr();
    let full = len / 64;
    for i in 0..full {
        let off = i * 64;
        let v = _mm512_loadu_si512(sptr.add(off) as *const __m512i);
        let x = _mm512_xor_si512(v, mask_vec);
        _mm512_storeu_si512(dptr.add(off) as *mut __m512i, x);
    }
    let tail = full * 64;
    if tail < len {
        copy_masked_fallback(&mut dst[tail..], &src[tail..], mask);
    }
}

/// Minimum bytes needed before a frame header can be (potentially) fully parsed.
pub(crate) const MIN_HDR: usize = 2;

/// Parse a single server frame header (no mask — server->client frames are never masked).
/// Returns (fin, opcode, payload_len, header_size) or None if not enough data.
pub(crate) fn parse_header(buf: &[u8]) -> Option<(bool, bool, u8, usize, usize)> {
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

/// Close-frame payload → (code, reason). Empty/short payloads carry neither.
pub(crate) fn parse_close_payload(payload: &[u8]) -> (Option<u16>, Option<String>) {
    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
    let reason = (payload.len() > 2).then(|| String::from_utf8_lossy(&payload[2..]).into_owned());
    (code, reason)
}

/// Payload construction strategy for `scan_frame_aligned`.
pub(crate) enum PayloadMode<'py, 'a> {
    /// Copy payload bytes out of the incoming buffer. Plain `data_received`
    /// chunks and buffered windows, where the window is invalidated right
    /// after the call, both need this.
    Copy,
    /// Zero-copy: wrap a slice of the incoming `PyBytes` as the `Bytes`
    /// owner. No memcpy of message payloads on this path.
    ZeroCopy { pb: &'a Bound<'py, PyBytes> },
}

pub(crate) struct FastFrame<'a> {
    pub(crate) opcode: u8,
    pub(crate) payload: &'a [u8],
    pub(crate) payload_start: usize,
}

pub(crate) enum VisitOutcome {
    Continue,
    Stop,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ScanOutcome {
    Exhausted { consumed: usize },
    Partial { consumed: usize, needed: usize },
    Fallback { consumed: usize },
    Stopped { consumed: usize },
}

#[inline(always)]
pub(crate) fn walk_frames<'a, E>(
    data: &'a [u8],
    mut visitor: impl FnMut(FastFrame<'a>) -> Result<VisitOutcome, E>,
) -> Result<ScanOutcome, E> {
    let mut off = 0usize;
    while let Some((fin, rsv1, opcode, plen, hdr)) = parse_header(&data[off..]) {
        if data.len() - off < hdr + plen {
            return Ok(ScanOutcome::Partial {
                consumed: off,
                needed: hdr + plen,
            });
        }
        if !fin || opcode == OP_CONTINUATION || rsv1 {
            return Ok(ScanOutcome::Fallback { consumed: off });
        }
        let total = hdr + plen;
        let payload_start = off + hdr;
        let frame = FastFrame {
            opcode,
            payload: &data[payload_start..off + total],
            payload_start,
        };
        if matches!(visitor(frame)?, VisitOutcome::Stop) {
            return Ok(ScanOutcome::Stopped {
                consumed: off + total,
            });
        }
        off += total;
    }
    Ok(ScanOutcome::Exhausted { consumed: off })
}
pub(crate) fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}
