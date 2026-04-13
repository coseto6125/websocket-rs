#!/usr/bin/env python3
"""Benchmark native_client vs picows using a picows-based echo server.

Running server-side on picows removes python-websockets as a possible
bottleneck at 64KB pipelined, so the remaining gap is purely client-side.
"""

import asyncio
import multiprocessing as mp
import statistics
import struct
import time

import uvloop

uvloop.install()

from websocket_rs.native_client import connect as native_connect

from picows import WSCloseCode, WSFrame, WSListener, WSMsgType, WSTransport, ws_connect, ws_create_server

WINDOW = 100
WARMUP = 200
N = 1000
PORT = 8810
URI = f"ws://127.0.0.1:{PORT}"


def _picows_echo_server(port, ready):
    import asyncio

    import uvloop

    uvloop.install()
    from picows import WSListener, WSMsgType, WSTransport, ws_create_server

    class EchoListener(WSListener):
        def on_ws_frame(self, transport: WSTransport, frame: WSFrame):
            if frame.msg_type == WSMsgType.CLOSE:
                transport.send_close(WSCloseCode.OK)
                transport.disconnect()
                return
            if frame.msg_type in (WSMsgType.BINARY, WSMsgType.TEXT):
                t_recv = time.perf_counter()
                payload_mv = frame.get_payload_as_memoryview()
                n = len(payload_mv)
                mid = struct.unpack_from("=I", payload_mv, 0)[0] if n >= 4 else 0
                t_send = time.perf_counter()
                out = bytearray(24 + n)
                struct.pack_into("=IddI", out, 0, mid, t_recv, t_send, n)
                out[24:] = payload_mv
                transport.send(WSMsgType.BINARY, out)

    async def main():
        server = await ws_create_server(EchoListener, "127.0.0.1", port)
        ready.set()
        await asyncio.Future()
        server.close()

    asyncio.run(main())


def parse(resp, size):
    mid, t_srv_recv, t_srv_send, ln = struct.unpack("=IddI", resp[:24])
    return mid, t_srv_recv, t_srv_send


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
        mid, *_ = parse(resp, size)
        rtts.append((t_recv - send_times[mid]) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def picows_rr(size, n):
    base_msg = bytearray(b"a") * (size + 4)
    mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()
    state = {"fut": None, "payload": None}

    class L(WSListener):
        def on_ws_frame(self, t, f: WSFrame):
            state["payload"] = bytes(f.get_payload_as_memoryview())
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
            mid, *_ = parse(payload, size)
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
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    p = ctx.Process(target=_picows_echo_server, args=(PORT, ready), daemon=True)
    p.start()
    ready.wait(timeout=10)
    await asyncio.sleep(0.3)

    print(f"Server = picows  (unified fast echo)\n")
    print(f"{'size':>6} {'mode':>12} {'client':>12} | mean    p50    p99")
    print("-" * 60)
    try:
        for size in (512, 1024, 4096, 16384, 65536):
            for mode in ("RR", "pipelined"):
                fn_native = native_rr if mode == "RR" else native_pipelined
                fn_picows = picows_rr if mode == "RR" else picows_pipelined
                m, p50, p99 = await fn_native(size, N)
                print(f"{size:>6}B {mode:>12} {'native':>12} | {m:6.4f} {p50:6.4f} {p99:6.4f} ms")
                m, p50, p99 = await fn_picows(size, N)
                print(f"{size:>6}B {mode:>12} {'picows':>12} | {m:6.4f} {p50:6.4f} {p99:6.4f} ms")
            print()
    finally:
        p.terminate()
        p.join(timeout=3)


if __name__ == "__main__":
    asyncio.run(main())
