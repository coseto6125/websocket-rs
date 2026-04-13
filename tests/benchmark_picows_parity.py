#!/usr/bin/env python3
"""Replicate picows's official RR benchmark, verbatim.

Pattern (from picows README):
  * Message sizes: 256B, 8KB, 100KB, 2MB
  * Mode: request-response (send → wait → recv → repeat), NOT pipelined
  * Duration: ~10s per cell
  * Metric: requests per second (RPS), higher is better
  * Server: single neutral Rust echo (tokio-tungstenite), pinned to core 0
  * Clients tested: native (ours), picows, websockets, aiohttp, websocket-client

Run:
    python tests/benchmark_picows_parity.py           # all clients, all sizes
    DURATION=3 python tests/benchmark_picows_parity.py  # shorter for dev
"""

import asyncio
import gc
import multiprocessing as mp
import os
import signal
import subprocess
import time

import uvloop

uvloop.install()

from websocket_rs.native_client import connect as native_connect  # noqa: E402
from websocket_rs.sync.client import connect as sync_connect  # noqa: E402

try:
    from picows import WSFrame, WSListener, WSMsgType, WSTransport, ws_connect

    PICOWS = True
except ImportError:
    PICOWS = False

try:
    import websockets.asyncio.client as ws_async

    WEBSOCKETS = True
except ImportError:
    WEBSOCKETS = False

try:
    import aiohttp

    AIOHTTP = True
except ImportError:
    AIOHTTP = False

try:
    import websocket as wsc  # websocket-client (sync)

    WSCLIENT = True
except ImportError:
    WSCLIENT = False


PORT = 8833
# 256 B / 8 KB / 100 KB / 1 MB — 1 MB is the realistic upper bound for
# real WS traffic: matches Cloudflare's hard per-frame limit, Azure
# SignalR's default, and well above AWS API Gateway WS (32 KB hard cap).
# 2 MB single frames don't survive most production proxies, so testing
# at that size only stresses framework limits, not real workloads.
SIZES = (256, 8 * 1024, 100 * 1024, 1024 * 1024)
DURATION = float(os.environ.get("DURATION", "10"))
WARMUP = 50
# Discarded timing pre-pass per cell: warms Python 3.13 bytecode specialization,
# allocator heaps, TCP buffer auto-tune, and any first-large-frame slow paths
# in the server. Without this, the first 2 MB cell against a freshly-spawned
# tokio-tungstenite shows ~1.0k RPS vs ~2.4k on subsequent runs.
PREWARM_SECONDS = 1.0


def _wait_port(port: int) -> None:
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", port), timeout=0.1):
                return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError(f"port {port} didn't open")


def start_rust_server(bin_name: str, port: int) -> subprocess.Popen:
    import shutil

    path = f"target/release/{bin_name}"
    if not os.path.exists(path):
        raise RuntimeError(f"{path} not built")
    cmd = [path, str(port)]
    if shutil.which("taskset"):
        cmd = ["taskset", "-c", "0", *cmd]
    p = subprocess.Popen(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_port(port)
    except RuntimeError:
        p.terminate()
        raise
    return p


def _picows_plain_echo_proc(port: int, ready):
    """Plain echo — reply with exact payload, no extra framing."""
    import asyncio as _asyncio
    import os as _os

    import uvloop as _uvloop

    try:
        _os.sched_setaffinity(0, {0})
    except Exception:
        pass
    _uvloop.install()
    from picows import (
        WSCloseCode,
        WSFrame,
        WSListener,
        WSMsgType,
        WSTransport,
        ws_create_server,
    )

    class EchoL(WSListener):
        def on_ws_frame(self, transport: WSTransport, frame: WSFrame):
            if frame.msg_type == WSMsgType.CLOSE:
                transport.send_close(WSCloseCode.OK)
                transport.disconnect()
                return
            if frame.msg_type in (WSMsgType.BINARY, WSMsgType.TEXT):
                transport.send(frame.msg_type, frame.get_payload_as_bytes())

    async def main():
        await ws_create_server(EchoL, "127.0.0.1", port)
        ready.set()
        await _asyncio.Future()

    _asyncio.run(main())


def start_picows_server(port: int):
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    p = ctx.Process(target=_picows_plain_echo_proc, args=(port, ready), daemon=True)
    p.start()
    ready.wait(timeout=10)
    time.sleep(0.3)
    return p


SERVERS: list[tuple[str, str, str | None]] = [
    ("tokio-tungstenite", "rust", "ws_echo_server"),
    ("fastwebsockets", "rust", "ws_echo_fastws"),
    ("picows", "picows", None),
]


# ---------- Client benchmarks ----------


async def bench_native(size: int, duration: float) -> int:
    payload = b"a" * size
    ws = await native_connect(f"ws://127.0.0.1:{PORT}")
    try:
        for _ in range(WARMUP):
            ws.send(payload)
            await ws.recv()
        count = 0
        deadline = time.perf_counter() + duration
        while time.perf_counter() < deadline:
            ws.send(payload)
            await ws.recv()
            count += 1
        return count
    finally:
        ws.close()


async def bench_picows(size: int, duration: float) -> int:
    payload = b"a" * size
    loop = asyncio.get_running_loop()
    st = {"fut": None}

    class L(WSListener):
        def on_ws_frame(self, t: WSTransport, f: WSFrame):
            fut = st["fut"]
            if fut and not fut.done():
                fut.set_result(None)

    transport, _ = await ws_connect(L, f"ws://127.0.0.1:{PORT}")
    try:
        for _ in range(WARMUP):
            st["fut"] = loop.create_future()
            transport.send(WSMsgType.BINARY, payload)
            await st["fut"]
        count = 0
        deadline = time.perf_counter() + duration
        while time.perf_counter() < deadline:
            st["fut"] = loop.create_future()
            transport.send(WSMsgType.BINARY, payload)
            await st["fut"]
            count += 1
        return count
    finally:
        transport.disconnect()


async def bench_websockets(size: int, duration: float) -> int:
    payload = b"a" * size
    async with ws_async.connect(f"ws://127.0.0.1:{PORT}", max_size=None) as ws:
        for _ in range(WARMUP):
            await ws.send(payload)
            await ws.recv()
        count = 0
        deadline = time.perf_counter() + duration
        while time.perf_counter() < deadline:
            await ws.send(payload)
            await ws.recv()
            count += 1
        return count


async def bench_aiohttp(size: int, duration: float) -> int:
    payload = b"a" * size
    async with aiohttp.ClientSession() as session:
        async with session.ws_connect(f"ws://127.0.0.1:{PORT}", max_msg_size=0) as ws:
            for _ in range(WARMUP):
                await ws.send_bytes(payload)
                await ws.receive()
            count = 0
            deadline = time.perf_counter() + duration
            while time.perf_counter() < deadline:
                await ws.send_bytes(payload)
                await ws.receive()
                count += 1
            return count


def bench_native_sync(size: int, duration: float) -> int:
    """websocket-rs sync client (tungstenite-backed, no asyncio)."""
    payload = b"a" * size
    with sync_connect(f"ws://127.0.0.1:{PORT}") as ws:
        for _ in range(WARMUP):
            ws.send(payload)
            ws.recv()
        count = 0
        deadline = time.perf_counter() + duration
        while time.perf_counter() < deadline:
            ws.send(payload)
            ws.recv()
            count += 1
        return count


def bench_wsclient_sync(size: int, duration: float) -> int:
    """websocket-client is sync; run in a thread elsewhere if mixing with async."""
    payload = b"a" * size
    ws = wsc.create_connection(f"ws://127.0.0.1:{PORT}")
    try:
        for _ in range(WARMUP):
            ws.send_binary(payload)
            ws.recv()
        count = 0
        deadline = time.perf_counter() + duration
        while time.perf_counter() < deadline:
            ws.send_binary(payload)
            ws.recv()
            count += 1
        return count
    finally:
        ws.close()


# ---------- Runner ----------


CLIENTS: list[tuple[str, bool, object]] = [
    ("ws-rs-async", True, bench_native),
    ("ws-rs-sync", True, bench_native_sync),
    ("picows", PICOWS, bench_picows if PICOWS else None),
    ("websockets", WEBSOCKETS, bench_websockets if WEBSOCKETS else None),
    ("aiohttp", AIOHTTP, bench_aiohttp if AIOHTTP else None),
    ("ws-client", WSCLIENT, bench_wsclient_sync if WSCLIENT else None),
]


def fmt_size(n: int) -> str:
    if n >= 1024 * 1024:
        return f"{n // (1024 * 1024)}MB"
    if n >= 1024:
        return f"{n // 1024}KB"
    return f"{n}B"


def fmt_rps(r: float) -> str:
    if r >= 1000:
        return f"{r / 1000:.1f}k"
    return f"{r:.0f}"


async def run_one_async(fn, size: int, duration: float) -> int:
    # Discarded pre-pass to prime cold paths (see PREWARM_SECONDS comment).
    gc.collect()
    await fn(size, PREWARM_SECONDS)
    gc.collect()
    gc.disable()
    try:
        return await fn(size, duration)
    finally:
        gc.enable()


def run_one_sync(fn, size: int, duration: float) -> int:
    gc.collect()
    fn(size, PREWARM_SECONDS)
    gc.collect()
    gc.disable()
    try:
        return fn(size, duration)
    finally:
        gc.enable()


async def run_matrix(label: str):
    enabled = [(name, fn) for name, ok, fn in CLIENTS if ok and fn is not None]
    print(f"\n=== Server: {label} ===")
    header = f"{'size':>8} " + " ".join(f"{name:>10}" for name, _ in enabled)
    print(header)
    print("-" * len(header))
    for size in SIZES:
        cells = []
        for _, fn in enabled:
            try:
                if asyncio.iscoroutinefunction(fn):
                    n = await run_one_async(fn, size, DURATION)
                else:
                    n = await asyncio.to_thread(run_one_sync, fn, size, DURATION)
                cells.append(fmt_rps(n / DURATION))
            except Exception as e:
                cells.append(f"FAIL:{type(e).__name__}")
        row = f"{fmt_size(size):>8} " + " ".join(f"{c:>10}" for c in cells)
        print(row, flush=True)


async def main():
    try:
        os.sched_setaffinity(0, {1})
    except Exception:
        pass

    for label, kind, bin_name in SERVERS:
        if kind == "rust":
            assert bin_name is not None
            proc = start_rust_server(bin_name, PORT)
        else:
            proc = start_picows_server(PORT)
        try:
            await asyncio.sleep(0.3)
            await run_matrix(label)
        finally:
            if isinstance(proc, subprocess.Popen):
                proc.send_signal(signal.SIGTERM)
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
            else:
                proc.terminate()
                proc.join(timeout=3)
            await asyncio.sleep(0.3)


if __name__ == "__main__":
    asyncio.run(main())
