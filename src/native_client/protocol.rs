//! ProtocolCore state machine: handshake accept-key validation, opcode
//! dispatch to protocol events, and permessage-deflate message decoding.
use base64::Engine;
use bytes::{Buf, Bytes, BytesMut};
use flate2::read::DeflateDecoder;
use rand::RngExt;
use sha1::{Digest, Sha1};
use std::io::Read as _;

const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

use super::codec::{
    find_header_end, parse_close_payload, parse_header, OP_BINARY, OP_CLOSE, OP_CONTINUATION,
    OP_PING, OP_PONG, OP_TEXT,
};

use crate::is_reserved_websocket_header;

pub(crate) struct ProtocolCore<'a> {
    pub(crate) buf: &'a mut BytesMut,
    pub(crate) handshake_done: &'a mut bool,
    pub(crate) expected_accept: &'a str,
    pub(crate) compression_enabled: bool,
    pub(crate) fragment_buf: &'a mut Option<BytesMut>,
    pub(crate) fragment_opcode: &'a mut u8,
    pub(crate) fragment_rsv1: &'a mut bool,
}

pub(crate) enum HandshakeOutcome {
    Complete,
    Pending,
    Accepted {
        subprotocol: Option<String>,
        compression_enabled: bool,
    },
    Rejected,
}

pub(crate) enum ProtocolEvent {
    Message(Bytes),
    SendPong(Bytes),
    Close {
        code: Option<u16>,
        reason: Option<String>,
    },
    ProtocolError(&'static str),
}

pub(crate) enum EventFlow {
    Continue,
    Stop,
}

#[derive(Debug)]
pub(crate) struct ProtocolCoreError(pub(crate) String);

#[derive(Debug)]
pub(crate) enum EmitError<E> {
    Core(ProtocolCoreError),
    Sink(E),
}

impl ProtocolCore<'_> {
    #[cold]
    pub(crate) fn process_handshake(&mut self) -> HandshakeOutcome {
        if *self.handshake_done {
            return HandshakeOutcome::Complete;
        }
        let Some(end) = find_header_end(self.buf) else {
            return HandshakeOutcome::Pending;
        };
        let headers = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        let mut matched = false;
        let mut subprotocol = None;
        let mut deflate_accepted = false;
        for line in headers.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("sec-websocket-accept:") && line.contains(self.expected_accept) {
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
        self.buf.advance(end);
        if !matched {
            return HandshakeOutcome::Rejected;
        }
        *self.handshake_done = true;
        self.compression_enabled &= deflate_accepted;
        HandshakeOutcome::Accepted {
            subprotocol,
            compression_enabled: self.compression_enabled,
        }
    }

    #[cold]
    pub(crate) fn next_event(&mut self) -> Result<Option<ProtocolEvent>, ProtocolCoreError> {
        while let Some((fin, rsv1, opcode, plen, hdr)) = parse_header(self.buf) {
            if self.buf.len() < hdr + plen {
                return Ok(None);
            }
            let total = hdr + plen;
            match opcode {
                OP_TEXT | OP_BINARY => {
                    self.buf.advance(hdr);
                    let payload = self.buf.split_to(plen).freeze();
                    if self.fragment_buf.is_some() {
                        return Ok(Some(ProtocolEvent::ProtocolError(
                            "new data frame while fragmented message is in progress",
                        )));
                    }
                    if fin {
                        let payload = if rsv1 {
                            Bytes::from(decompress_message(self.compression_enabled, &payload)?)
                        } else {
                            payload
                        };
                        return Ok(Some(ProtocolEvent::Message(payload)));
                    }
                    let mut fragment = BytesMut::with_capacity(plen);
                    fragment.extend_from_slice(&payload);
                    *self.fragment_buf = Some(fragment);
                    *self.fragment_opcode = opcode;
                    *self.fragment_rsv1 = rsv1;
                }
                OP_CONTINUATION => {
                    self.buf.advance(hdr);
                    let payload = self.buf.split_to(plen);
                    let Some(fragment) = self.fragment_buf.as_mut() else {
                        return Ok(Some(ProtocolEvent::ProtocolError(
                            "continuation frame without fragmented message",
                        )));
                    };
                    fragment.extend_from_slice(&payload);
                    if fin {
                        let fragment = self.fragment_buf.take().expect("fragment exists");
                        let compressed = *self.fragment_rsv1;
                        *self.fragment_opcode = 0;
                        *self.fragment_rsv1 = false;
                        let raw = fragment.freeze();
                        let payload = if compressed {
                            Bytes::from(decompress_message(self.compression_enabled, &raw)?)
                        } else {
                            raw
                        };
                        return Ok(Some(ProtocolEvent::Message(payload)));
                    }
                }
                OP_CLOSE => {
                    self.buf.advance(hdr);
                    let payload = self.buf.split_to(plen);
                    let (code, reason) = parse_close_payload(&payload);
                    return Ok(Some(ProtocolEvent::Close { code, reason }));
                }
                OP_PING => {
                    self.buf.advance(hdr);
                    return Ok(Some(ProtocolEvent::SendPong(
                        self.buf.split_to(plen).freeze(),
                    )));
                }
                OP_PONG => self.buf.advance(total),
                _ => self.buf.advance(total),
            }
        }
        Ok(None)
    }
}

#[cold]
pub(crate) fn emit_protocol_events<E>(
    mut next_event: impl FnMut() -> Result<Option<ProtocolEvent>, ProtocolCoreError>,
    mut sink: impl FnMut(ProtocolEvent) -> Result<EventFlow, E>,
) -> Result<(), EmitError<E>> {
    loop {
        let event = next_event().map_err(EmitError::Core)?;
        let Some(event) = event else {
            return Ok(());
        };
        if matches!(sink(event).map_err(EmitError::Sink)?, EventFlow::Stop) {
            return Ok(());
        }
    }
}
/// Permessage-deflate bookkeeping. Compression is applied/deapplied in front of
/// each message, trading a few % compression ratio for simpler, race-free code.
/// Marker struct — presence of Option<DeflateCtx>::Some means permessage-deflate
/// is negotiated. No per-connection state: Compress/Decompress are instantiated
/// fresh per message (no_context_takeover semantics either way).
pub(crate) struct DeflateCtx;

pub(crate) fn build_handshake(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    subprotocols: &[String],
    compression: bool,
) -> (Vec<u8>, String) {
    let mut key_bytes = [0u8; 16];
    rand::rng().fill(&mut key_bytes);
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
        // matching the fresh per-message decoders built by decompress_message.
        req.push_str(
            "Sec-WebSocket-Extensions: permessage-deflate; \
             client_no_context_takeover; server_no_context_takeover\r\n",
        );
    }
    for (k, v) in headers {
        if is_reserved_websocket_header(k) {
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
pub(crate) fn decompress_message(
    compression_enabled: bool,
    compressed: &[u8],
) -> Result<Vec<u8>, ProtocolCoreError> {
    if !compression_enabled {
        return Err(ProtocolCoreError(
            "received compressed frame but permessage-deflate is not enabled".to_string(),
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
        .map_err(|e| ProtocolCoreError(format!("deflate decode error: {e}")))?;
    Ok(out)
}
