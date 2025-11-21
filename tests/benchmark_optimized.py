#!/usr/bin/env python3
import asyncio
import statistics
import time

import websockets
from websocket_rs import WebSocket


async def start_echo_server(host: str = "localhost", port: int = 8765):
    async def echo(websocket):
        async for message in websocket:
            await websocket.send(message)

    async with websockets.serve(echo, host, port):
        await asyncio.Future()


async def benchmark_python_native(uri: str, message_size: int, num_messages: int) -> dict:
    message = "a" * message_size
    latencies = []

    async with websockets.connect(uri) as ws:
        for _ in range(10):
            await ws.send("warmup")
            await ws.recv()

        for _ in range(num_messages):
            start = time.perf_counter()
            await ws.send(message)
            await ws.recv()
            latencies.append((time.perf_counter() - start) * 1000)

    return {
        "impl": "Python Native",
        "avg_ms": sum(latencies) / len(latencies),
        "std_ms": statistics.stdev(latencies),
    }


def benchmark_rust_optimized(uri: str, message_size: int, num_messages: int) -> dict:
    """Rust 優化版本"""
    message = "a" * message_size
    latencies = []

    ws = WebSocket(uri)
    ws.connect()

    try:
        # 預熱
        for _ in range(10):
            ws.send_sync("warmup")
            ws.receive_sync()

        # 單條消息測試
        for _ in range(num_messages):
            start = time.perf_counter()
            ws.send_sync(message)
            ws.receive_sync()
            latencies.append((time.perf_counter() - start) * 1000)
    finally:
        ws.close()

    return {
        "impl": "Rust Optimized",
        "avg_ms": sum(latencies) / len(latencies),
        "std_ms": statistics.stdev(latencies),
    }


def benchmark_rust_batch(uri: str, message_size: int, num_messages: int, batch_size: int = 10) -> dict:
    """Rust 批量版本"""
    message = "a" * message_size
    latencies = []

    ws = WebSocket(uri)
    ws.connect()

    try:
        # 預熱
        ws.send_batch(["warmup"] * 10)
        ws.receive_batch(10)

        # 批量測試
        num_batches = num_messages // batch_size
        messages = [message] * batch_size

        for _ in range(num_batches):
            start = time.perf_counter()
            ws.send_batch(messages)
            ws.receive_batch(batch_size)
            batch_time = (time.perf_counter() - start) * 1000
            # 分攤到每個消息
            latencies.extend([batch_time / batch_size] * batch_size)
    finally:
        ws.close()

    return {
        "impl": f"Rust Batch({batch_size})",
        "avg_ms": sum(latencies) / len(latencies),
        "std_ms": statistics.stdev(latencies) if len(latencies) > 1 else 0,
    }


async def main():
    uri = "ws://localhost:8765"
    message_sizes = [1024, 8192, 32768, 65536]
    num_messages = 1000

    server_task = asyncio.create_task(start_echo_server())
    await asyncio.sleep(1)

    try:
        print("=== WebSocket 性能測試（優化版） ===\n")

        for size in message_sizes:
            print(f"\n消息大小: {size} bytes, 消息數量: {num_messages}")
            print("-" * 60)

            # Python Native
            result = await asyncio.to_thread(
                lambda: asyncio.run(benchmark_python_native(uri, size, num_messages))
            )
            baseline = result['avg_ms']
            print(f"{result['impl']:20} | avg: {result['avg_ms']:6.3f}ms | std: {result['std_ms']:5.3f}ms | 相對: 100%")

            # Rust 優化版 (在新線程運行避免 deadlock)
            result = await asyncio.to_thread(benchmark_rust_optimized, uri, size, num_messages)
            relative = (baseline / result['avg_ms']) * 100
            print(f"{result['impl']:20} | avg: {result['avg_ms']:6.3f}ms | std: {result['std_ms']:5.3f}ms | 相對: {relative:3.0f}%")

            # Rust 批量版 (batch=10)
            result = await asyncio.to_thread(benchmark_rust_batch, uri, size, num_messages, 10)
            relative = (baseline / result['avg_ms']) * 100
            print(f"{result['impl']:20} | avg: {result['avg_ms']:6.3f}ms | std: {result['std_ms']:5.3f}ms | 相對: {relative:3.0f}%")

            # Rust 批量版 (batch=100)
            result = await asyncio.to_thread(benchmark_rust_batch, uri, size, num_messages, 100)
            relative = (baseline / result['avg_ms']) * 100
            print(f"{result['impl']:20} | avg: {result['avg_ms']:6.3f}ms | std: {result['std_ms']:5.3f}ms | 相對: {relative:3.0f}%")

    finally:
        server_task.cancel()
        try:
            await server_task
        except asyncio.CancelledError:
            pass


if __name__ == "__main__":
    asyncio.run(main())