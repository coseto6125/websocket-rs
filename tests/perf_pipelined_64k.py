#!/usr/bin/env python3
"""Minimal harness: one client (native|picows) against tokio-tungstenite,
64KB pipelined only. Designed to be wrapped by `perf stat`.

Usage: python perf_pipelined_64k.py {native|picows}
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

SIZE = 65536
N = int(os.environ.get("PERF_N", "5000"))
WARMUP = 200
WINDOW = 100
PORT = 8830


def parse_mid(resp):
    return struct.unpack("=I", resp[:4])[0]


def start_server():
    path = "target/release/ws_echo_server"
    p = subprocess.Popen(
        ["taskset", "-c", "0", path, str(PORT)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", PORT), timeout=0.1):
                return p
        except OSError:
            time.sleep(0.1)
    p.terminate()
    raise RuntimeError("server didn't open port")


async def native_pipelined():
    from websocket_rs.native_client import connect as native_connect

    base = b"a" * SIZE
    ws = await native_connect(f"ws://127.0.0.1:{PORT}")
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
        await ws.recv()
    send_times = {}
    rtts = []
    sent = recv = 0
    while recv < N:
        while sent < N and (sent - recv) < WINDOW:
            send_times[sent] = time.perf_counter()
            ws.send(struct.pack("=I", sent) + base)
            sent += 1
        resp = await ws.recv()
        t_recv = time.perf_counter()
        recv += 1
        rtts.append((t_recv - send_times[parse_mid(resp)]) * 1000)
    ws.close()
    return rtts


async def native_callback_pipelined():
    """Native client using on_message callback — picows-style architecture."""
    from websocket_rs.native_client import connect as native_connect

    base = b"a" * SIZE
    loop = asyncio.get_running_loop()
    done = loop.create_future()
    send_times = {}
    rtts = []
    state = {"ws": None, "sent": 0, "received": 0, "in_warmup": True, "w_done": 0}

    def on_msg(msg):
        if state["in_warmup"]:
            state["w_done"] += 1
            if state["w_done"] >= WARMUP:
                state["in_warmup"] = False
                _fill()
            return
        t_recv = time.perf_counter()
        mid = struct.unpack_from("=I", memoryview(msg))[0]
        rtts.append((t_recv - send_times[mid]) * 1000)
        state["received"] += 1
        if state["received"] >= N:
            if not done.done():
                done.set_result(None)
            return
        _fill()

    def _fill():
        ws = state["ws"]
        while state["sent"] < N and (state["sent"] - state["received"]) < WINDOW:
            send_times[state["sent"]] = time.perf_counter()
            ws.send(struct.pack("=I", state["sent"]) + base)
            state["sent"] += 1

    ws = await native_connect(f"ws://127.0.0.1:{PORT}", on_message=on_msg)
    state["ws"] = ws
    for _ in range(WARMUP):
        ws.send(struct.pack("=I", 9999) + b"w")
    await done
    ws.close()
    return rtts


async def picows_pipelined():
    from picows import WSFrame, WSListener, WSMsgType, ws_connect

    base_msg = bytearray(b"a") * (SIZE + 4)
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
            rtts.append((t_recv - send_times[parse_mid(payload)]) * 1000)
            self.received += 1
            if self.received >= N:
                t.disconnect()
                if not done.done():
                    done.set_result(None)
                return
            self._fill()

        def _fill(self):
            while self.sent < N and (self.sent - self.received) < WINDOW:
                mv[:4] = struct.pack("=I", self.sent)
                send_times[self.sent] = time.perf_counter()
                self.transport.send(WSMsgType.BINARY, base_msg)
                self.sent += 1

    transport, _ = await ws_connect(L, f"ws://127.0.0.1:{PORT}")
    await done
    return rtts


async def amain(client: str):
    try:
        os.sched_setaffinity(0, {1})
    except Exception:
        pass
    p = start_server()
    try:
        await asyncio.sleep(0.3)
        fn = {
            "native": native_pipelined,
            "native_cb": native_callback_pipelined,
            "picows": picows_pipelined,
        }[client]
        rtts = await fn()
        print(
            f"{client}: mean={statistics.mean(rtts):.3f} p50={statistics.median(rtts):.3f} p99={statistics.quantiles(rtts, n=100)[98]:.3f} ms",
            file=sys.stderr,
        )
    finally:
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()


if __name__ == "__main__":
    client = sys.argv[1] if len(sys.argv) > 1 else "native"
    assert client in ("native", "native_cb", "picows")
    asyncio.run(amain(client))
