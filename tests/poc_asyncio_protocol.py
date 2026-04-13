#!/usr/bin/env python3
"""Architecture PoC: pure-Python WebSocket client over asyncio.Protocol.

Goal: validate whether running on the asyncio loop thread (picows-style) really
yields the ~2x RR latency improvement vs pyo3-async-runtimes + tokio on a
separate thread pool. If this PoC (even in plain Python) gets close to picows
numbers, the architectural rewrite for websocket-rs is justified.

Minimal hand-rolled WS framing — client only, no extensions, no masking SIMD.
"""

import asyncio
import base64
import os
import statistics
import struct
import subprocess
import sys
import time
from hashlib import sha1

import uvloop

uvloop.install()


WINDOW = 100
WARMUP = 200
NUM = 1000
PORT = 8800
URI = f"ws://127.0.0.1:{PORT}"


def make_handshake(host: str, port: int, path: str = "/"):
    key = base64.b64encode(os.urandom(16)).decode()
    req = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    ).encode()
    expected_accept = base64.b64encode(
        sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()
    ).decode()
    return req, expected_accept


def encode_client_frame(opcode: int, payload: bytes) -> bytes:
    """Build a single WS frame with masking (client MUST mask)."""
    fin_opcode = 0x80 | opcode
    n = len(payload)
    mask_key = os.urandom(4)
    if n < 126:
        header = struct.pack("!BB", fin_opcode, 0x80 | n)
    elif n < 65536:
        header = struct.pack("!BBH", fin_opcode, 0x80 | 126, n)
    else:
        header = struct.pack("!BBQ", fin_opcode, 0x80 | 127, n)
    # Mask payload
    masked = bytearray(payload)
    for i in range(n):
        masked[i] ^= mask_key[i & 3]
    return header + mask_key + bytes(masked)


class WSProtocol(asyncio.Protocol):
    """Hand-rolled client WS Protocol. Runs on asyncio loop thread."""

    def __init__(self):
        self.transport: asyncio.Transport | None = None
        self._buf = bytearray()
        self._handshake_done = False
        self._handshake_fut: asyncio.Future[None] | None = None
        self._expected_accept: str = ""
        # Pending recv Futures (FIFO)
        self._pending: list[asyncio.Future[bytes]] = []
        # Backlog if messages arrive before recv is called
        self._backlog: list[bytes] = []
        self._closed = False

    def connection_made(self, transport: asyncio.Transport):  # type: ignore[override]
        self.transport = transport
        self.transport.get_extra_info("socket").setsockopt(6, 1, 1)  # TCP_NODELAY

    def data_received(self, data: bytes):  # type: ignore[override]
        self._buf.extend(data)
        if not self._handshake_done:
            if b"\r\n\r\n" in self._buf:
                end = self._buf.index(b"\r\n\r\n") + 4
                headers = bytes(self._buf[:end]).decode("latin-1", "replace")
                del self._buf[:end]
                if self._expected_accept not in headers:
                    raise RuntimeError("handshake accept mismatch")
                self._handshake_done = True
                if self._handshake_fut and not self._handshake_fut.done():
                    self._handshake_fut.set_result(None)
            else:
                return
        # Parse frames
        self._parse_frames()

    def _parse_frames(self):
        while True:
            n = len(self._buf)
            if n < 2:
                return
            b0, b1 = self._buf[0], self._buf[1]
            opcode = b0 & 0x0F
            masked = (b1 & 0x80) != 0
            plen = b1 & 0x7F
            hdr = 2
            if plen == 126:
                if n < 4:
                    return
                plen = struct.unpack_from("!H", self._buf, 2)[0]
                hdr = 4
            elif plen == 127:
                if n < 10:
                    return
                plen = struct.unpack_from("!Q", self._buf, 2)[0]
                hdr = 10
            if masked:
                if n < hdr + 4:
                    return
                hdr += 4
            if n < hdr + plen:
                return
            payload = bytes(self._buf[hdr : hdr + plen])
            del self._buf[: hdr + plen]
            if opcode in (0x1, 0x2):  # text/binary
                if self._pending:
                    fut = self._pending.pop(0)
                    if not fut.done():
                        fut.set_result(payload)
                else:
                    self._backlog.append(payload)
            elif opcode == 0x8:  # close
                self._closed = True
                for f in self._pending:
                    if not f.done():
                        f.set_exception(ConnectionError("closed"))
                self._pending.clear()
                if self.transport:
                    self.transport.close()
                return
            # ping/pong ignored for bench

    def connection_lost(self, exc):  # type: ignore[override]
        self._closed = True
        for f in self._pending:
            if not f.done():
                f.set_exception(exc or ConnectionError("closed"))
        self._pending.clear()

    def send_bytes(self, payload: bytes):
        """Fire-and-forget send. Runs on asyncio thread; no Future."""
        if self._closed or not self.transport:
            raise RuntimeError("closed")
        self.transport.write(encode_client_frame(0x2, payload))

    def recv_bytes(self) -> asyncio.Future[bytes]:
        if self._backlog:
            fut = asyncio.get_running_loop().create_future()
            fut.set_result(self._backlog.pop(0))
            return fut
        if self._closed:
            fut = asyncio.get_running_loop().create_future()
            fut.set_exception(ConnectionError("closed"))
            return fut
        fut = asyncio.get_running_loop().create_future()
        self._pending.append(fut)
        return fut


async def connect(host: str, port: int) -> WSProtocol:
    loop = asyncio.get_running_loop()
    proto = WSProtocol()
    handshake_fut = loop.create_future()
    proto._handshake_fut = handshake_fut
    req, expected = make_handshake(host, port)
    proto._expected_accept = expected
    await loop.create_connection(lambda: proto, host, port)
    assert proto.transport
    proto.transport.write(req)
    await handshake_fut
    return proto


# ---------- Echo server in subprocess ----------


def _server_proc(port: int, ready_evt):
    import asyncio

    import uvloop
    import websockets

    uvloop.install()

    async def echo(ws):
        try:
            async for msg in ws:
                t_recv = time.perf_counter()
                if isinstance(msg, str):
                    msg = msg.encode()
                mid = struct.unpack("=I", msg[:4])[0] if len(msg) >= 4 else 0
                t_send = time.perf_counter()
                header = struct.pack("=IddI", mid, t_recv, t_send, len(msg))
                await ws.send(header + msg)
        except Exception:
            pass

    async def main():
        async with websockets.serve(echo, "127.0.0.1", port):
            ready_evt.set()
            await asyncio.Future()

    asyncio.run(main())


# ---------- Benchmark ----------


def parse_resp(resp: bytes, sz: int):
    mid, t_srv_recv, t_srv_send, ln = struct.unpack("=IddI", resp[:24])
    return mid, t_srv_recv, t_srv_send


async def bench_rr(size: int, n: int):
    base = b"a" * size
    proto = await connect("127.0.0.1", PORT)
    for _ in range(WARMUP):
        proto.send_bytes(struct.pack("=I", 9999) + b"w")
        await proto.recv_bytes()

    rtts = []
    for mid in range(n):
        t0 = time.perf_counter()
        proto.send_bytes(struct.pack("=I", mid) + base)
        resp = await proto.recv_bytes()
        t1 = time.perf_counter()
        rtts.append((t1 - t0) * 1000)
    if proto.transport:
        proto.transport.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def bench_pipelined(size: int, n: int):
    base = b"a" * size
    proto = await connect("127.0.0.1", PORT)
    for _ in range(WARMUP):
        proto.send_bytes(struct.pack("=I", 9999) + b"w")
        await proto.recv_bytes()

    send_times: dict[int, float] = {}
    rtts: list[float] = []
    sent = recv = 0
    while recv < n:
        while sent < n and (sent - recv) < WINDOW:
            send_times[sent] = time.perf_counter()
            proto.send_bytes(struct.pack("=I", sent) + base)
            sent += 1
        resp = await proto.recv_bytes()
        t_recv = time.perf_counter()
        recv += 1
        mid, *_ = parse_resp(resp, size)
        rtts.append((t_recv - send_times[mid]) * 1000)
    if proto.transport:
        proto.transport.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def main():
    import multiprocessing as mp

    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    proc = ctx.Process(target=_server_proc, args=(PORT, ready), daemon=True)
    proc.start()
    ready.wait(timeout=5)
    await asyncio.sleep(0.2)

    print(f"PoC: pure-Python asyncio.Protocol WS client (uvloop)\n")
    print(f"{'Size':>6} {'Mode':>12} | mean      p50      p99")
    print("-" * 55)

    try:
        for size in (512, 1024, 4096, 16384, 65536):
            mean, p50, p99 = await bench_rr(size, NUM)
            print(f"{size:>6}B {'RR':>12} | {mean:.4f}ms {p50:.4f}ms {p99:.4f}ms")
            mean, p50, p99 = await bench_pipelined(size, NUM)
            print(f"{size:>6}B {'pipelined':>12} | {mean:.4f}ms {p50:.4f}ms {p99:.4f}ms")
    finally:
        proc.terminate()
        proc.join(timeout=3)


if __name__ == "__main__":
    asyncio.run(main())
