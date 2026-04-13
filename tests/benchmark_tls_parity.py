#!/usr/bin/env python3
"""TLS variant of benchmark_picows_parity. Same RR methodology, but every
client connects via wss:// to a tokio-tungstenite TLS echo server using
self-signed cert from tests/certs/. Measures the production-realistic
case where TLS handshake + AES-GCM encryption sit on the data path.
"""

import asyncio
import gc
import os
import signal
import ssl
import subprocess
import sys
import time

import uvloop

uvloop.install()

# Trust our self-signed cert globally — affects sync-client backends that
# don't take an ssl_context arg (websocket-rs sync, aiohttp default).
CERT_PATH = os.path.abspath("tests/certs/cert.pem")
os.environ["SSL_CERT_FILE"] = CERT_PATH

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
    import websocket as wsc

    WSCLIENT = True
except ImportError:
    WSCLIENT = False


PORT = 8835
# 256 B / 8 KB / 100 KB / 1 MB — 1 MB matches Cloudflare's hard per-frame
# WS limit; production traffic above that doesn't survive most proxies.
# Testing at 2 MB only stresses framework limits, not real workloads.
SIZES = (256, 8 * 1024, 100 * 1024, 1024 * 1024)
DURATION = float(os.environ.get("DURATION", "10"))
WARMUP = 50
PREWARM_SECONDS = 1.0
URI = f"wss://127.0.0.1:{PORT}"


def insecure_ctx() -> ssl.SSLContext:
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    return ctx


def start_server() -> subprocess.Popen:
    import shutil

    path = "target/release/ws_echo_server_tls"
    if not os.path.exists(path):
        raise RuntimeError(f"{path} not built")
    cmd = [path, str(PORT)]
    if shutil.which("taskset"):
        cmd = ["taskset", "-c", "0", *cmd]
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    import socket as _s

    for _ in range(50):
        try:
            with _s.create_connection(("127.0.0.1", PORT), timeout=0.1):
                return p
        except OSError:
            time.sleep(0.1)
    p.terminate()
    raise RuntimeError("port didn't open")


# ---------- Client benchmarks ----------


async def bench_native(size: int, duration: float) -> int:
    payload = b"a" * size
    ctx = insecure_ctx()
    ws = await native_connect(URI, ssl_context=ctx)
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


def bench_native_sync(size: int, duration: float) -> int:
    payload = b"a" * size
    with sync_connect(URI) as ws:
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


async def bench_picows(size: int, duration: float) -> int:
    payload = b"a" * size
    loop = asyncio.get_running_loop()
    st = {"fut": None}

    class L(WSListener):
        def on_ws_frame(self, t: WSTransport, f: WSFrame):
            fut = st["fut"]
            if fut and not fut.done():
                fut.set_result(None)

    transport, _ = await ws_connect(L, URI, ssl_context=insecure_ctx())
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
    async with ws_async.connect(URI, ssl=insecure_ctx(), max_size=None) as ws:
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
    conn = aiohttp.TCPConnector(ssl=insecure_ctx())
    async with aiohttp.ClientSession(connector=conn) as session:
        async with session.ws_connect(URI, max_msg_size=0) as ws:
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


def bench_wsclient_sync(size: int, duration: float) -> int:
    payload = b"a" * size
    ws = wsc.create_connection(URI, sslopt={"cert_reqs": ssl.CERT_NONE})
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


CLIENTS: list[tuple[str, bool, object]] = [
    ("ws-rs-async", True, bench_native),
    ("ws-rs-sync", True, bench_native_sync),
    # picows TLS segfaults under sustained load in our matrix harness
    # (single isolated runs work — likely a state-corruption bug in
    # picows's wss path under WARMUP=50 + tight RR loop). Skipped to keep
    # the rest of the matrix runnable. Tracked as a workaround, not a fix.
    # ("picows", PICOWS, bench_picows if PICOWS else None),
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


async def main():
    try:
        os.sched_setaffinity(0, {1})
    except Exception:
        pass

    proc = start_server()
    try:
        await asyncio.sleep(0.3)
        enabled = [(name, fn) for name, ok, fn in CLIENTS if ok and fn is not None]
        print(f"=== Server: tokio-tungstenite (TLS, native-tls) ===")
        header = f"{'size':>8} " + " ".join(f"{name:>11}" for name, _ in enabled)
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
            row = f"{fmt_size(size):>8} " + " ".join(f"{c:>11}" for c in cells)
            print(row, flush=True)
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    asyncio.run(main())
