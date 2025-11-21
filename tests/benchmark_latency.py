#!/usr/bin/env python3
import asyncio
import contextlib
import time
import statistics

import websockets
from websocket_rs import WebSocket


async def start_echo_server(host: str = "localhost", port: int = 8765):
    async def echo(websocket):
        async for message in websocket:
            await websocket.send(message)

    async with websockets.serve(echo, host, port):
        await asyncio.Future()


async def measure_latency_python(uri: str, message_size: int, num_samples: int = 100):
    """測量 Python 原生的延遲"""
    message = "a" * message_size
    latencies = []

    async with websockets.connect(uri) as ws:
        # 預熱
        for _ in range(10):
            await ws.send("warmup")
            await ws.recv()

        # 測量單條消息的延遲
        for _ in range(num_samples):
            start = time.perf_counter()
            await ws.send(message)
            await ws.recv()
            latency = (time.perf_counter() - start) * 1000
            latencies.append(latency)

    return latencies


def measure_latency_rust(uri: str, message_size: int, num_samples: int = 100):
    """測量 Rust 的延遲"""
    message = "a" * message_size
    latencies = []

    ws = WebSocket(uri)
    ws.connect()

    try:
        # 預熱
        for _ in range(10):
            ws.send("warmup")
            ws.recv()

        # 測量單條消息的延遲
        for _ in range(num_samples):
            start = time.perf_counter()
            ws.send(message)
            ws.recv()
            latency = (time.perf_counter() - start) * 1000
            latencies.append(latency)
    finally:
        ws.close()

    return latencies


def measure_latency_batch(uri: str, message_size: int, batch_size: int, num_samples: int = 100):
    """測量批量發送的延遲（包含等待時間）"""
    message = "a" * message_size
    latencies = []

    ws = WebSocket(uri)
    ws.connect()

    try:
        # 預熱
        ws.send_batch(["warmup"] * 10)
        ws.receive_batch(10)

        # 模擬實際場景：消息到達時間有間隔
        for _ in range(num_samples // batch_size):
            batch_start = time.perf_counter()
            messages = []

            # 模擬消息陸續到達
            for i in range(batch_size):
                if i > 0:
                    # 模擬消息間隔（0.1ms）
                    time.sleep(0.0001)
                messages.append(message)

            # 批量發送
            send_start = time.perf_counter()
            ws.send_batch(messages)
            ws.receive_batch(batch_size)
            send_end = time.perf_counter()

            # 總延遲 = 等待時間 + 發送時間
            wait_time = (send_start - batch_start) * 1000
            send_time = (send_end - send_start) * 1000
            total_latency = (send_end - batch_start) * 1000

            latencies.append({
                "wait": wait_time,
                "send": send_time,
                "total": total_latency,
                "per_msg": total_latency / batch_size
            })
    finally:
        ws.close()

    return latencies


def percentile(data, percent):
    """計算百分位數"""
    sorted_data = sorted(data)
    index = int(len(sorted_data) * percent / 100)
    if index >= len(sorted_data):
        return sorted_data[-1]
    return sorted_data[index]


async def main():
    uri = "ws://localhost:8765"
    message_sizes = [1024, 8192, 32768]
    num_samples = 100

    server_task = asyncio.create_task(start_echo_server())
    await asyncio.sleep(1)

    try:
        print("=== WebSocket 延遲測試 ===\n")
        print("測量單條消息的往返延遲（RTT）")
        print("-" * 80)

        for size in message_sizes:
            print(f"\n消息大小: {size} bytes")
            print("-" * 40)

            # Python
            py_latencies = await asyncio.to_thread(
                lambda: asyncio.run(measure_latency_python(uri, size, num_samples))
            )
            print("Python Native:")
            print(f"  中位數: {statistics.median(py_latencies):.3f}ms")
            print(f"  平均值: {statistics.mean(py_latencies):.3f}ms")
            print(f"  P95:    {percentile(py_latencies, 95):.3f}ms")
            print(f"  P99:    {percentile(py_latencies, 99):.3f}ms")
            print(f"  最大值: {max(py_latencies):.3f}ms")

            # Rust 單條
            rust_latencies = await asyncio.to_thread(measure_latency_rust, uri, size, num_samples)
            print("\nRust (單條):")
            print(f"  中位數: {statistics.median(rust_latencies):.3f}ms")
            print(f"  平均值: {statistics.mean(rust_latencies):.3f}ms")
            print(f"  P95:    {percentile(rust_latencies, 95):.3f}ms")
            print(f"  P99:    {percentile(rust_latencies, 99):.3f}ms")
            print(f"  最大值: {max(rust_latencies):.3f}ms")

            # 比較
            speedup = statistics.median(py_latencies) / statistics.median(rust_latencies)
            print(f"\n速度提升: {speedup:.2f}x")

        # 批量延遲分析
        print("\n" + "=" * 80)
        print("批量發送延遲分析（包含等待時間）")
        print("-" * 80)

        size = 8192
        for batch_size in [10, 50, 100]:
            print(f"\n批量大小: {batch_size}, 消息大小: {size} bytes")
            batch_results = await asyncio.to_thread(
                measure_latency_batch, uri, size, batch_size, 100
            )

            wait_times = [r["wait"] for r in batch_results]
            send_times = [r["send"] for r in batch_results]
            total_times = [r["total"] for r in batch_results]
            per_msg_times = [r["per_msg"] for r in batch_results]

            print(f"  等待時間: {statistics.mean(wait_times):.3f}ms")
            print(f"  發送時間: {statistics.mean(send_times):.3f}ms")
            print(f"  總延遲:   {statistics.mean(total_times):.3f}ms")
            print(f"  每條消息: {statistics.mean(per_msg_times):.3f}ms")

    finally:
        server_task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await server_task


if __name__ == "__main__":
    asyncio.run(main())