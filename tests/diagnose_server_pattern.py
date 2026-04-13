#!/usr/bin/env python3
"""Measure how each server writes WS frames back — chunk count & sizes.

Connects raw TCP, does WS handshake, sends a single large message, then
logs every data_received call on our asyncio.Protocol. If picows server
writes a 64KB echo as (say) 8 smaller chunks while tokio-tungstenite sends
it in 1-2 chunks, that directly explains why picows-client (which likely
has a different read loop) is faster against picows-server: the echo shape
matches its reader.
"""

import asyncio
import base64
import multiprocessing as mp
import os
import signal
import struct
import subprocess
import time

import uvloop

uvloop.install()


def handshake_bytes(host, port):
    key = base64.b64encode(os.urandom(16)).decode()
    req = (
        f"GET / HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    ).encode()
    return req


def encode_frame(payload, opcode=0x2):
    n = len(payload)
    mask = os.urandom(4)
    if n < 126:
        hdr = bytes([0x80 | opcode, 0x80 | n])
    elif n < 65536:
        hdr = bytes([0x80 | opcode, 0x80 | 126]) + n.to_bytes(2, "big")
    else:
        hdr = bytes([0x80 | opcode, 0x80 | 127]) + n.to_bytes(8, "big")
    masked = bytearray(payload)
    for i in range(n):
        masked[i] ^= mask[i & 3]
    return hdr + mask + bytes(masked)


class Probe(asyncio.Protocol):
    def __init__(self):
        self.chunks: list[int] = []
        self.transport = None
        self.handshake_done = False
        self.buf = bytearray()
        self.done = asyncio.get_event_loop().create_future()
        self.target_bytes = 0
        self.received_total = 0
        self.first_chunk_ts: float | None = None
        self.last_chunk_ts: float | None = None

    def connection_made(self, transport):
        self.transport = transport

    def data_received(self, data):
        now = time.perf_counter()
        if self.first_chunk_ts is None and self.handshake_done:
            self.first_chunk_ts = now
        self.last_chunk_ts = now

        if not self.handshake_done:
            self.buf.extend(data)
            if b"\r\n\r\n" in self.buf:
                end = self.buf.index(b"\r\n\r\n") + 4
                del self.buf[:end]
                self.handshake_done = True
                # any remaining bytes count as chunk data
                if self.buf:
                    self.chunks.append(len(self.buf))
                    self.received_total = len(self.buf)
            return

        self.chunks.append(len(data))
        self.received_total += len(data)
        if self.received_total >= self.target_bytes and not self.done.done():
            self.done.set_result(None)

    def connection_lost(self, exc):
        if not self.done.done():
            self.done.set_exception(exc or ConnectionError("closed"))


async def probe(uri_host, port, payload_size):
    loop = asyncio.get_running_loop()
    proto = Probe()
    await loop.create_connection(lambda: proto, uri_host, port)
    proto.transport.write(handshake_bytes(uri_host, port))

    # Wait for handshake
    for _ in range(500):
        if proto.handshake_done:
            break
        await asyncio.sleep(0.001)
    else:
        raise RuntimeError("handshake timeout")

    payload = b"a" * payload_size
    # Server will echo back a framed response. We need to figure out total expected bytes.
    # tokio-tungstenite + fastws prepend 24-byte header (=IddI) + payload. Binary opcode,
    # no masking (server->client), so frame header is 2/4/10 bytes depending on length.
    resp_payload = 24 + payload_size
    if resp_payload < 126:
        frame_hdr_len = 2
    elif resp_payload < 65536:
        frame_hdr_len = 4
    else:
        frame_hdr_len = 10
    proto.target_bytes = frame_hdr_len + resp_payload

    # Reset timers after handshake done
    proto.first_chunk_ts = None
    proto.chunks = []
    proto.received_total = len(proto.buf)
    if proto.received_total:
        proto.chunks.append(proto.received_total)

    t_send = time.perf_counter()
    proto.transport.write(encode_frame(payload))
    await asyncio.wait_for(proto.done, timeout=5)

    proto.transport.close()
    return {
        "chunks": proto.chunks,
        "total": proto.received_total,
        "expected": proto.target_bytes,
        "first_delay": (proto.first_chunk_ts - t_send) * 1000 if proto.first_chunk_ts else None,
        "fan_out": (proto.last_chunk_ts - proto.first_chunk_ts) * 1000 if proto.last_chunk_ts and proto.first_chunk_ts else 0.0,
    }


# ---- Servers ----
def start_rust(bin_name, port):
    p = subprocess.Popen(
        [f"target/release/{bin_name}", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", port), timeout=0.1):
                return p
        except OSError:
            time.sleep(0.1)
    raise RuntimeError("server didn't start")


def _picows_echo(port, ready):
    import asyncio

    import uvloop

    uvloop.install()
    from picows import (
        WSCloseCode,
        WSFrame,
        WSListener,
        WSMsgType,
        WSTransport,
        ws_create_server,
    )

    class L(WSListener):
        def on_ws_frame(self, t: WSTransport, f: WSFrame):
            if f.msg_type == WSMsgType.CLOSE:
                t.send_close(WSCloseCode.OK)
                t.disconnect()
                return
            if f.msg_type in (WSMsgType.BINARY, WSMsgType.TEXT):
                mv = f.get_payload_as_memoryview()
                n = len(mv)
                mid = struct.unpack_from("=I", mv, 0)[0] if n >= 4 else 0
                out = bytearray(24 + n)
                struct.pack_into("=IddI", out, 0, mid, 0.0, 0.0, n)
                out[24:] = mv
                t.send(WSMsgType.BINARY, out)

    async def main():
        await ws_create_server(L, "127.0.0.1", port)
        ready.set()
        await asyncio.Future()

    asyncio.run(main())


def start_picows(port):
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    p = ctx.Process(target=_picows_echo, args=(port, ready), daemon=True)
    p.start()
    ready.wait(timeout=10)
    time.sleep(0.3)
    return p


class PipelineProbe(asyncio.Protocol):
    """Fire N messages rapid-fire and record every data_received call size/timestamp."""

    def __init__(self, expected_total_bytes: int):
        self.handshake_done = False
        self.buf = bytearray()
        self.transport = None
        self.chunks: list[tuple[float, int]] = []
        self.received = 0
        self.expected = expected_total_bytes
        self.done = asyncio.get_event_loop().create_future()

    def connection_made(self, t):
        self.transport = t

    def data_received(self, data):
        if not self.handshake_done:
            self.buf.extend(data)
            if b"\r\n\r\n" in self.buf:
                end = self.buf.index(b"\r\n\r\n") + 4
                del self.buf[:end]
                self.handshake_done = True
                if self.buf:
                    self.chunks.append((time.perf_counter(), len(self.buf)))
                    self.received += len(self.buf)
            return
        self.chunks.append((time.perf_counter(), len(data)))
        self.received += len(data)
        if self.received >= self.expected and not self.done.done():
            self.done.set_result(None)


async def probe_pipelined(host, port, payload_size, n):
    loop = asyncio.get_running_loop()
    payload = b"a" * payload_size
    resp_body = 24 + payload_size
    frame_hdr = 2 if resp_body < 126 else (4 if resp_body < 65536 else 10)
    expected_per = frame_hdr + resp_body

    proto = PipelineProbe(expected_per * n)
    await loop.create_connection(lambda: proto, host, port)
    proto.transport.write(handshake_bytes(host, port))
    for _ in range(500):
        if proto.handshake_done:
            break
        await asyncio.sleep(0.001)

    # Fire all N messages back-to-back BEFORE awaiting any response
    frame = encode_frame(payload)
    t_start = time.perf_counter()
    for _ in range(n):
        proto.transport.write(frame)

    await asyncio.wait_for(proto.done, timeout=30)
    t_end = time.perf_counter()
    proto.transport.close()

    return {
        "n_chunks": len(proto.chunks),
        "chunk_sizes": [c[1] for c in proto.chunks],
        "total_ms": (t_end - t_start) * 1000,
        "bytes_received": proto.received,
    }


async def main():
    servers = [
        ("tokio-tungstenite", start_rust("ws_echo_server", 8840), 8840),
        ("fastwebsockets", start_rust("ws_echo_fastws", 8841), 8841),
        ("picows", start_picows(8842), 8842),
    ]
    await asyncio.sleep(0.3)

    try:
        print("=== Single-message probe ===")
        for size in (4096, 16384, 65536):
            print(f"\npayload={size}B:")
            for label, _, port in servers:
                samples = [await probe("127.0.0.1", port, size) for _ in range(5)]
                samples.sort(key=lambda s: len(s["chunks"]))
                med = samples[len(samples) // 2]
                print(f"  {label:18} chunks={len(med['chunks']):2d}  sizes={med['chunks'][:3]}{'...' if len(med['chunks']) > 3 else ''}")

        print("\n=== Pipelined probe (100 msgs back-to-back) ===")
        for size in (4096, 16384, 65536):
            print(f"\npayload={size}B ×100 msgs:")
            for label, _, port in servers:
                r = await probe_pipelined("127.0.0.1", port, size, 100)
                unique_sizes = sorted(set(r["chunk_sizes"]))
                from collections import Counter

                dist = Counter(r["chunk_sizes"]).most_common(4)
                print(f"  {label:18} {r['n_chunks']:3d} chunks  total={r['total_ms']:.3f}ms  size_distribution={dist}")
    finally:
        for _, p, _ in servers:
            try:
                if isinstance(p, subprocess.Popen):
                    p.send_signal(signal.SIGTERM)
                    p.wait(timeout=3)
                else:
                    p.terminate()
                    p.join(timeout=3)
            except Exception:
                pass


if __name__ == "__main__":
    asyncio.run(main())
