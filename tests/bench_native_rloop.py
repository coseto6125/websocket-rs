#!/usr/bin/env python3
"""Compare NativeClient under uvloop vs rloop.

Both the client benchmark process and the echo server subprocess are swapped
so we fairly isolate loop impact from other noise.
"""

import asyncio
import multiprocessing as mp
import os
import statistics
import struct
import sys
import time

LOOP_NAME = os.environ.get("LOOP", "uvloop")
if LOOP_NAME == "rloop":
    import rloop

    asyncio.set_event_loop_policy(rloop.EventLoopPolicy())
elif LOOP_NAME == "uvloop":
    import uvloop

    uvloop.install()
else:
    pass  # default asyncio

from websocket_rs.native_client import connect as native_connect

try:
    from picows import WSFrame, WSListener, WSMsgType, ws_connect

    PICOWS = True
except ImportError:
    PICOWS = False

WINDOW = 100
WARMUP = 200
N = 1000
PORT = 8802
URI = f"ws://127.0.0.1:{PORT}"


def _server(port, ready, loop_name):
    # Server always uses uvloop for a stable, fast baseline — we're measuring
    # client-side loop impact, not server-side.
    import uvloop

    uvloop.install()
    _ = loop_name  # unused

    import websockets

    async def echo(ws):
        try:
            async for msg in ws:
                t_recv = time.perf_counter()
                if isinstance(msg, str):
                    msg = msg.encode()
                mid = struct.unpack("=I", msg[:4])[0] if len(msg) >= 4 else 0
                t_send = time.perf_counter()
                hdr = struct.pack("=IddI", mid, t_recv, t_send, len(msg))
                await ws.send(hdr + msg)
        except Exception:
            pass

    async def main():
        async with websockets.serve(echo, "127.0.0.1", port):
            ready.set()
            await asyncio.Future()

    asyncio.run(main())


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
        mid = struct.unpack("=I", resp[:4])[0]
        rtts.append((t_recv - send_times[mid]) * 1000)
    ws.close()
    return statistics.mean(rtts), statistics.median(rtts), statistics.quantiles(rtts, n=100)[98]


async def main():
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    p = ctx.Process(target=_server, args=(PORT, ready, LOOP_NAME), daemon=True)
    p.start()
    ready.wait(timeout=10)
    await asyncio.sleep(0.2)

    print(f"LOOP={LOOP_NAME}  (asyncio.policy={type(asyncio.get_event_loop_policy()).__name__})")
    print(f"{'size':>6} {'mode':>12} | mean    p50    p99")
    print("-" * 45)
    try:
        for size in (512, 4096, 16384, 65536):
            m, p50, p99 = await native_rr(size, N)
            print(f"{size:>6}B {'RR':>12} | {m:.4f} {p50:.4f} {p99:.4f} ms")
            m, p50, p99 = await native_pipelined(size, N)
            print(f"{size:>6}B {'pipelined':>12} | {m:.4f} {p50:.4f} {p99:.4f} ms")
    finally:
        p.terminate()
        p.join(timeout=3)


if __name__ == "__main__":
    asyncio.run(main())
