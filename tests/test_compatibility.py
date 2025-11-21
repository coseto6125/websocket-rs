#!/usr/bin/env python3
"""
測試 Rust WebSocket 與 Python websockets 的 API 相容性
"""
import sys
import asyncio
import time

# Set stdout encoding to utf-8 for Windows compatibility
sys.stdout.reconfigure(encoding='utf-8')

import websockets
from websocket_rs import WebSocket


async def start_echo_server():
    async def echo(websocket):
        async for message in websocket:
            await websocket.send(message)
    async with websockets.serve(echo, "localhost", 8765):
        await asyncio.Future()


def test_rust_sync_api():
    """測試 Rust 同步 API"""
    print("測試 Rust WebSocket 同步 API...")

    # 1. 基本連接
    ws = WebSocket("ws://localhost:8765")
    ws.connect()

    # 2. 標準 send/recv 方法
    ws.send("Hello")
    response = ws.recv()
    assert response == "Hello", f"Expected 'Hello', got {response}"
    print("✓ send/recv 工作正常")

    # 3. receive 別名
    ws.send("World")
    response = ws.receive()
    assert response == "World", f"Expected 'World', got {response}"
    print("✓ receive 別名工作正常")

    # 4. 二進制數據
    ws.send(b"Binary")
    response = ws.recv()
    assert response == b"Binary", f"Expected b'Binary', got {response}"
    print("✓ 二進制數據工作正常")

    # 5. 關閉連接
    ws.close()
    print("✓ close 工作正常")

    # 6. Context Manager (sync)
    with WebSocket("ws://localhost:8765") as ws:
        ws.send("Context")
        response = ws.recv()
        assert response == "Context"
    print("✓ Context Manager 工作正常")

    # 7. 批量操作（擴展功能）
    ws = WebSocket("ws://localhost:8765")
    ws.connect()
    ws.send_batch(["msg1", "msg2", "msg3"])
    responses = ws.receive_batch(3)
    assert len(responses) == 3
    assert responses == ["msg1", "msg2", "msg3"]
    ws.close()
    print("✓ 批量操作工作正常")

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


def test_performance_comparison():
    """性能對比測試"""
    print("性能對比測試...")

    # Rust 版本
    ws = WebSocket("ws://localhost:8765")
    ws.connect()

    start = time.perf_counter()
    for _ in range(100):
        ws.send("test")
        ws.recv()
    rust_time = (time.perf_counter() - start) * 1000
    ws.close()

    print(f"Rust WebSocket: 100 次往返耗時 {rust_time:.2f}ms")

    # Python 原生（同步方式）
    async def python_test():
        async with websockets.connect("ws://localhost:8765") as ws:
            start = time.perf_counter()
            for _ in range(100):
                await ws.send("test")
                await ws.recv()
            return (time.perf_counter() - start) * 1000

    python_time = asyncio.run(python_test())
    print(f"Python websockets: 100 次往返耗時 {python_time:.2f}ms")

    speedup = python_time / rust_time
    print(f"\n速度提升: {speedup:.2f}x")


async def main():
    # 啟動 Echo 服務器
    server = asyncio.create_task(start_echo_server())
    await asyncio.sleep(1)

    try:
        # 測試同步 API
        await asyncio.to_thread(test_rust_sync_api)

        # 測試異步 API
        await test_python_async_api()

        # 性能比較
        await asyncio.to_thread(test_performance_comparison)

        print("\n=== API 相容性測試總結 ===")
        print("✅ Rust WebSocket 完全兼容 Python websockets 基本 API")
        print("✅ 支援 send/recv/receive/close 方法")
        print("✅ 支援同步 Context Manager")
        print("✅ 支援文本和二進制消息")
        print("✅ 額外提供批量操作優化 API")

    finally:
        server.cancel()
        try:
            await server
        except asyncio.CancelledError:
            pass


if __name__ == "__main__":
    asyncio.run(main())