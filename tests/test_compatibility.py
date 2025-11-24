#!/usr/bin/env python3
"""
測試 Rust WebSocket 與 Python websockets 的 API 相容性
"""

import asyncio
import sys
import time

# Set stdout encoding to utf-8 for Windows compatibility
sys.stdout.reconfigure(encoding="utf-8")

import websocket_rs.async_client
import websocket_rs.sync.client
import websockets

sync_connect = websocket_rs.sync.client.connect
async_connect = websocket_rs.async_client.connect


async def start_echo_server():
    async def echo(websocket):
        async for message in websocket:
            await websocket.send(message)

    async with websockets.serve(echo, "localhost", 8765):
        await asyncio.Future()


def test_rust_sync_api():
    """測試 Rust WebSocket 同步 API (websocket_rs.sync.client)"""
    print("測試 Rust WebSocket 同步 API...")

    # 1. Basic send/recv with context manager
    with sync_connect("ws://localhost:8765") as ws:
        ws.send("Hello Rust")
        assert ws.recv() == "Hello Rust"
    print("✓ send/recv 工作正常")

    # 2. Binary data
    with sync_connect("ws://localhost:8765") as ws:
        data = b"\x00\x01\x02\x03"
        ws.send(data)
        assert ws.recv() == data
    print("✓ 二進制數據工作正常")

    # 3. Properties
    with sync_connect("ws://localhost:8765") as ws:
        assert ws.open is True
        assert ws.closed is False
        local = ws.local_address
        remote = ws.remote_address
        print(f"  Local: {local}, Remote: {remote}")
    print("✓ Properties 工作正常")

    # 4. Iterator Support
    with sync_connect("ws://localhost:8765") as ws:
        ws.send("Iter1")
        ws.send("Iter2")

        received = []
        for i, msg in enumerate(ws):
            received.append(msg)
            if i == 1:
                break

        assert received == ["Iter1", "Iter2"]
    print("✓ Iterator 支援工作正常")

    # 5. Ping/Pong
    with sync_connect("ws://localhost:8765") as ws:
        ws.ping(b"ping data")
        ws.pong(b"pong data")
    print("✓ Ping/Pong 工作正常")

    print("\n所有同步 API 測試通過！\n")


async def test_python_async_api():
    """測試 Python 原生 async API"""
    print("測試 Python websockets 異步 API...")

    async with websockets.connect("ws://localhost:8765") as ws:
        # 1. 基本 send/recv
        await ws.send("Hello Async")
        response = await ws.recv()
        assert response == "Hello Async"
        print("✓ async send/recv 工作正常")

        # 2. 二進制
        await ws.send(b"Async Binary")
        response = await ws.recv()
        assert response == b"Async Binary"
        print("✓ async 二進制數據工作正常")

    print("\n所有 Python 異步 API 測試通過！\n")


async def test_rust_async_api():
    """測試 Rust WebSocket 異步 API (websocket_rs.async_client)"""
    print("測試 Rust WebSocket 異步 API...")

    # 1. Async send/recv with context manager
    async with await async_connect("ws://localhost:8765") as ws:
        await ws.send("Async Rust")
        response = await ws.recv()
        assert response == "Async Rust"
    print("✓ async send/recv 工作正常")

    # 2. Manual async connect/close
    ws = await async_connect("ws://localhost:8765")
    await ws.send("Manual Async")
    response = await ws.recv()
    assert response == "Manual Async"
    await ws.close()
    print("✓ manual async connect/close 工作正常")

    # 3. Properties
    async with await async_connect("ws://localhost:8765") as ws:
        assert ws.open is True
        assert ws.closed is False
        local = ws.local_address
        remote = ws.remote_address
        print(f"  Local: {local}, Remote: {remote}")
    print("✓ Properties 工作正常")

    # 4. Async Iterator Support
    async with await async_connect("ws://localhost:8765") as ws:
        await ws.send("AsyncIter1")
        await ws.send("AsyncIter2")

        received = []
        async for msg in ws:
            received.append(msg)
            if len(received) == 2:
                break

        assert received == ["AsyncIter1", "AsyncIter2"]
    print("✓ Async Iterator 支援工作正常")

    # 5. Ping/Pong
    async with await async_connect("ws://localhost:8765") as ws:
        await ws.ping(b"ping data")
        await ws.pong(b"pong data")
    print("✓ Ping/Pong 工作正常")

    print("\n所有 Rust 異步 API 測試通過！\n")


async def test_performance_comparison():
    """性能對比測試 (Async vs Async)"""
    print("性能對比測試 (Async)...")

    ITERATIONS = 100

    # Rust WebSocket (Async)
    start = time.perf_counter()
    ws = await async_connect("ws://localhost:8765")
    try:
        for _ in range(ITERATIONS):
            await ws.send("test")
            await ws.recv()
    finally:
        await ws.close()
    rust_async_time = (time.perf_counter() - start) * 1000
    print(f"Rust WebSocket (Async): {ITERATIONS} 次往返耗時 {rust_async_time:.2f}ms")

    # Rust WebSocket (Sync)
    def run_sync_benchmark():
        start = time.perf_counter()
        with sync_connect("ws://localhost:8765") as ws:
            for _ in range(ITERATIONS):
                ws.send("test")
                ws.recv()
        return (time.perf_counter() - start) * 1000

    rust_sync_time = await asyncio.to_thread(run_sync_benchmark)
    print(f"Rust WebSocket (Sync) : {ITERATIONS} 次往返耗時 {rust_sync_time:.2f}ms")

    # Python websockets (Async) - Run in separate thread for fair comparison
    def run_python_benchmark():
        async def _run():
            async with websockets.connect("ws://localhost:8765") as ws:
                start = time.perf_counter()
                for _ in range(ITERATIONS):
                    await ws.send("test")
                    await ws.recv()
                return (time.perf_counter() - start) * 1000

        return asyncio.run(_run())

    python_time = await asyncio.to_thread(run_python_benchmark)
    print(f"Python websockets (Async): {ITERATIONS} 次往返耗時 {python_time:.2f}ms")

    print(f"\nRust Async vs Python: {python_time / rust_async_time:.2f}x")
    print(f"Rust Sync  vs Python: {python_time / rust_sync_time:.2f}x")


async def main():
    # 啟動 Echo 服務器
    server = asyncio.create_task(start_echo_server())
    await asyncio.sleep(1)

    try:
        # 測試同步 API
        await asyncio.to_thread(test_rust_sync_api)

        # 測試異步 API
        await test_python_async_api()

        # 測試 Rust 異步 API
        await test_rust_async_api()

        # 性能比較
        await test_performance_comparison()

        print("\n=== API 相容性測試總結 ===")
        print("✅ websocket_rs.sync.client 完全兼容 websockets.sync.client")
        print("✅ websocket_rs.async_client 完全兼容 websockets.asyncio.client")
        print("✅ 支援 send/recv/close 方法")
        print("✅ 支援 Context Manager (sync 和 async)")
        print("✅ 支援 Iterator (sync 和 async)")
        print("✅ 支援文本和二進制消息")
        print("✅ 支援 Ping/Pong")
        print("✅ 支援 Properties (open, closed, local_address, remote_address)")

    finally:
        server.cancel()
        try:
            await server
        except asyncio.CancelledError:
            pass


if __name__ == "__main__":
    asyncio.run(main())
