#!/usr/bin/env python3
"""Benchmark against the pure-Rust tokio-tungstenite echo server.

Per tarasko's issue #11 recommendation: use a pure high-performance server
(his baseline is C++ beast; ours is equivalent pure-Rust). This eliminates
Python server overhead and gives a tight comparison between clients.
"""

import asyncio
import os
import signal
import statistics
import struct
import subprocess
import sys
import time

import uvloop

uvloop.install()

from websocket_rs.native_client import connect as native_connect

try:
    from picows import WSFrame, WSListener, WSMsgType, ws_connect

    PICOWS = True
except ImportError:
    PICOWS = False

WINDOW = 100
WARMUP = 200
N = 1000
PORT = 8821
URI = f"ws://127.0.0.1:{PORT}"


def start_server():
    bin_path = "target/release/ws_echo_server"
    if not os.path.exists(bin_path):
        print(f"Build it first: cargo build --release --bin ws_echo_server --features echo-server-bin")
        sys.exit(1)
    proc = subprocess.Popen(
        [bin_path, str(PORT)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    # Wait for port to be open
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", PORT), timeout=0.1):
                return proc
        except OSError:
            time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("server didn't open port in time")


async def native_rr(size, n):
    base = b"a" * size
    ws = await native_connect(URI)
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
        await ws.recv()
    rtts = []
    for mid in range(n):
        t0 = time.perf_counter()
        ws.send(struct.pack("=I", mid) + base)
        await ws.recv()
        rtts.append((time.perf_counter() - t0) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def native_pipelined(size, n):
    base = b"a" * size
    ws = await native_connect(URI)
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
        await ws.recv()
    send_times = {}
    rtts = []
    sent = recv = 0
    while recv < n:
        while sent < n and (sent - recv) < WINDOW:
            send_times[sent] = time.perf_counter()
            ws.send(struct.pack("=I", sent) + base)
            sent += 1
        resp = await ws.recv()
        t_recv = time.perf_counter()
        recv += 1
        mid = struct.unpack_from("=I", resp, 0)[0]
        rtts.append((t_recv - send_times[mid]) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def picows_rr(size, n):
    base_msg = bytearray(b"a") * (size + 4)
    mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()
    state = {"fut": None}

    class L(WSListener):
        def on_ws_frame(self, t, f: WSFrame):
            fut = state["fut"]
            if fut and not fut.done():
                fut.set_result(None)

    transport, _ = await ws_connect(L, URI)
    for _ in range(WARMUP):
        mv[:4] = struct.pack("=I", 9999)
        state["fut"] = loop.create_future()
        transport.send(WSMsgType.BINARY, base_msg)
        await state["fut"]
    rtts = []
    for mid in range(n):
        mv[:4] = struct.pack("=I", mid)
        state["fut"] = loop.create_future()
        t0 = time.perf_counter()
        transport.send(WSMsgType.BINARY, base_msg)
        await state["fut"]
        rtts.append((time.perf_counter() - t0) * 1000)
    transport.disconnect()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def picows_pipelined(size, n):
    base_msg = bytearray(b"a") * (size + 4)
    mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()
    done = loop.create_future()
    send_times = {}
    rtts = []

    class L(WSListener):
        def __init__(self):
            self.transport = None
            self.sent = 0
            self.received = 0
            self.in_warmup = True
            self.w_done = 0

        def on_ws_connected(self, t):
            self.transport = t
            for _ in range(WARMUP):
                mv[:4] = struct.pack("=I", 9999)
                t.send(WSMsgType.BINARY, base_msg)

        def on_ws_frame(self, t, f: WSFrame):
            if self.in_warmup:
                self.w_done += 1
                if self.w_done >= WARMUP:
                    self.in_warmup = False
                    self._fill()
                return
            t_recv = time.perf_counter()
            payload = bytes(f.get_payload_as_memoryview())
            mid = struct.unpack_from("=I", payload, 0)[0]
            rtts.append((t_recv - send_times[mid]) * 1000)
            self.received += 1
            if self.received >= n:
                t.disconnect()
                if not done.done():
                    done.set_result(None)
                return
            self._fill()

        def _fill(self):
            while self.sent < n and (self.sent - self.received) < WINDOW:
                mv[:4] = struct.pack("=I", self.sent)
                send_times[self.sent] = time.perf_counter()
                self.transport.send(WSMsgType.BINARY, base_msg)
                self.sent += 1

    transport, _ = await ws_connect(L, URI)
    await done
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def main():
    server = start_server()
    await asyncio.sleep(0.2)
    print(f"Server = pure Rust tokio-tungstenite echo (pid={server.pid})\n")
    print(f"{'size':>6} {'mode':>12} {'client':>12} | mean    p50    p99")
    print("-" * 60)
    try:
        for size in (512, 1024, 4096, 16384, 65536):
            for mode in ("RR", "pipelined"):
                fn_native = native_rr if mode == "RR" else native_pipelined
                fn_picows = picows_rr if mode == "RR" else picows_pipelined
                m, p50, p99 = await fn_native(size, N)
                print(f"{size:>6}B {mode:>12} {'native':>12} | {m:6.4f} {p50:6.4f} {p99:6.4f} ms")
                if PICOWS:
                    m, p50, p99 = await fn_picows(size, N)
                    print(f"{size:>6}B {mode:>12} {'picows':>12} | {m:6.4f} {p50:6.4f} {p99:6.4f} ms")
            print()
    finally:
        server.send_signal(signal.SIGTERM)
        server.wait(timeout=3)


if __name__ == "__main__":
    asyncio.run(main())
