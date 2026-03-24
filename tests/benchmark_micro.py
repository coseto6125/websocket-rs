"""
Micro-benchmark: isolate CPU overhead of websocket-rs vs websockets.

Measures per-call overhead with high iteration counts over localhost.
"""

import asyncio
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

import websocket_rs.async_client
import websocket_rs.sync.client
import websockets
import websockets.sync.client

URI = "ws://localhost:8765"
WARMUP = 5
ITERS = 200


def fmt(count: int, elapsed: float) -> str:
    rate = count / elapsed
    us = elapsed / count * 1_000_000
    return f"{rate:>8,.0f} ops/s  ({us:>6.1f} µs/op)  total {elapsed*1000:>7.1f}ms"


# ── Sync ─────────────────────────────────────────────────────────


def bench_sync(label: str, connect_fn, iters: int, msg):
    with connect_fn(URI) as ws:
        for _ in range(WARMUP):
            ws.send(msg)
            ws.recv()
        t0 = time.perf_counter()
        for _ in range(iters):
            ws.send(msg)
            ws.recv()
        elapsed = time.perf_counter() - t0
    print(f"  {label:22} {fmt(iters, elapsed)}")


# ── Async ────────────────────────────────────────────────────────


async def bench_async(label: str, connect_fn, iters: int, msg):
    ws = await connect_fn(URI)
    try:
        for _ in range(WARMUP):
            await ws.send(msg)
            await ws.recv()
        t0 = time.perf_counter()
        for _ in range(iters):
            await ws.send(msg)
            await ws.recv()
        elapsed = time.perf_counter() - t0
    finally:
        await ws.close()
    print(f"  {label:22} {fmt(iters, elapsed)}")


async def bench_ping(label: str, connect_fn, iters: int):
    ws = await connect_fn(URI)
    try:
        for _ in range(WARMUP):
            await ws.ping(b"\x00")
        t0 = time.perf_counter()
        for _ in range(iters):
            await ws.ping(b"\x00")
        elapsed = time.perf_counter() - t0
    finally:
        await ws.close()
    print(f"  {label:22} {fmt(iters, elapsed)}")


# ── Server ───────────────────────────────────────────────────────


async def echo_server():
    async def echo(ws):
        async for msg in ws:
            await ws.send(msg)

    async with websockets.serve(echo, "localhost", 8765, ping_interval=None):
        await asyncio.Future()


# ── Main ─────────────────────────────────────────────────────────


async def main():
    server = asyncio.create_task(echo_server())
    await asyncio.sleep(0.3)

    small = "x" * 32
    large = "x" * 16384
    large_bin = b"\x00" * 16384

    try:
        print("=== Micro-Benchmark: CPU Overhead ===\n")

        print(f"[Sync] Small text (32B) x {ITERS}")
        await asyncio.to_thread(bench_sync, "websockets", websockets.sync.client.connect, ITERS, small)
        await asyncio.to_thread(bench_sync, "websocket-rs", websocket_rs.sync.client.connect, ITERS, small)
        print()

        print(f"[Sync] Large text (16KB) x {ITERS}")
        await asyncio.to_thread(bench_sync, "websockets", websockets.sync.client.connect, ITERS, large)
        await asyncio.to_thread(bench_sync, "websocket-rs", websocket_rs.sync.client.connect, ITERS, large)
        print()

        print(f"[Sync] Large binary (16KB) x {ITERS}")
        await asyncio.to_thread(bench_sync, "websockets", websockets.sync.client.connect, ITERS, large_bin)
        await asyncio.to_thread(bench_sync, "websocket-rs", websocket_rs.sync.client.connect, ITERS, large_bin)
        print()

        print(f"[Async] Small text (32B) x {ITERS}")
        await bench_async("websocket-rs", websocket_rs.async_client.connect, ITERS, small)
        print()

        print(f"[Async] Large binary (16KB) x {ITERS}")
        await bench_async("websocket-rs", websocket_rs.async_client.connect, ITERS, large_bin)
        print()

        print(f"[Async] Ping burst x {ITERS * 4}")
        await bench_ping("websocket-rs", websocket_rs.async_client.connect, ITERS * 4)

    finally:
        server.cancel()
        try:
            await server
        except asyncio.CancelledError:
            pass


if __name__ == "__main__":
    asyncio.run(main())
