#!/usr/bin/env python3
import asyncio
import statistics
import time
from dataclasses import asdict, dataclass
from typing import List

import orjson
import websockets
from loguru import logger
from websocket_rs import WebSocket

# 配置日誌
logger.add("benchmark.log", rotation="100 MB", retention="1 week")


@dataclass
class BenchmarkResult:
    message_size: int
    total_messages: int
    avg_latency_ms: float
    min_latency_ms: float
    max_latency_ms: float
    std_dev_latency_ms: float  # 添加標準差
    throughput_mbps: float
    implementation: str


@dataclass
class ComparisonResult:
    message_size: int
    implementation: str
    latency_percentage: float  # 相對於 Rust 的延遲百分比
    throughput_percentage: float  # 相對於 Rust 的吞吐量百分比


async def start_echo_server(host: str = "localhost", port: int = 8765):
    async def echo(websocket):
        async for message in websocket:
            await websocket.send(message)

    logger.info(f"啟動 WebSocket echo 服務器於 {host}:{port}")
    async with websockets.serve(echo, host, port):
        await asyncio.Future()  # 運行直到被取消


async def benchmark_native_websocket(uri: str, message_size: int, num_messages: int) -> BenchmarkResult:
    logger.info(f"開始測試原生 Python WebSocket: 消息大小={message_size}字節, 消息數量={num_messages}")
    message = "a" * message_size
    latencies = []

    # 預熱連接
    async with websockets.connect(uri) as ws:
        for _ in range(10):
            await ws.send("warmup")
            await ws.recv()

    await asyncio.sleep(0.5)  # 短暫暫停

    # 實際測試
    async with websockets.connect(uri) as ws:
        for _ in range(num_messages):
            start_time = time.perf_counter()
            await ws.send(message)
            response = await ws.recv()
            end_time = time.perf_counter()
            assert len(response) == message_size
            latencies.append((end_time - start_time) * 1000)  # 轉換為毫秒

    avg_latency = sum(latencies) / len(latencies)
    min_latency = min(latencies)
    max_latency = max(latencies)
    std_dev_latency = statistics.stdev(latencies) if len(latencies) > 1 else 0
    total_bytes = message_size * num_messages * 2  # 乘2因為發送和接收
    duration = sum(latencies) / 1000  # 轉換回秒
    throughput = (total_bytes * 8) / (duration * 1_000_000)  # Mbps

    return BenchmarkResult(
        message_size=message_size,
        total_messages=num_messages,
        avg_latency_ms=avg_latency,
        min_latency_ms=min_latency,
        max_latency_ms=max_latency,
        std_dev_latency_ms=std_dev_latency,
        throughput_mbps=throughput,
        implementation="Python Native",
    )


async def benchmark_rust_websocket(uri: str, message_size: int, num_messages: int) -> BenchmarkResult:
    logger.info(f"開始測試 Rust WebSocket: 消息大小={message_size}字節, 消息數量={num_messages}")
    message = "a" * message_size
    latencies = []

    # 預熱連接
    ws_warmup = WebSocket(uri)
    async with ws_warmup:
        for _ in range(10):
            await ws_warmup.send("warmup")
            await ws_warmup.receive()

    await asyncio.sleep(0.5)  # 短暫暫停

    # 實際測試
    ws = WebSocket(uri)
    async with ws:
        for _ in range(num_messages):
            start_time = time.perf_counter()
            await ws.send(message)
            response = await ws.receive()
            end_time = time.perf_counter()
            assert len(response) == message_size
            latencies.append((end_time - start_time) * 1000)  # 轉換為毫秒

    avg_latency = sum(latencies) / len(latencies)
    min_latency = min(latencies)
    max_latency = max(latencies)
    std_dev_latency = statistics.stdev(latencies) if len(latencies) > 1 else 0
    total_bytes = message_size * num_messages * 2  # 乘2因為發送和接收
    duration = sum(latencies) / 1000  # 轉換回秒
    throughput = (total_bytes * 8) / (duration * 1_000_000)  # Mbps

    return BenchmarkResult(
        message_size=message_size,
        total_messages=num_messages,
        avg_latency_ms=avg_latency,
        min_latency_ms=min_latency,
        max_latency_ms=max_latency,
        std_dev_latency_ms=std_dev_latency,
        throughput_mbps=throughput,
        implementation="Rust",
    )


def calculate_relative_performance(rust_result: BenchmarkResult, other_result: BenchmarkResult) -> ComparisonResult:
    # 計算相對性能（Rust 為基準 100%）
    latency_percentage = (other_result.avg_latency_ms / rust_result.avg_latency_ms) * 100
    throughput_percentage = (other_result.throughput_mbps / rust_result.throughput_mbps) * 100

    return ComparisonResult(
        message_size=rust_result.message_size,
        implementation=other_result.implementation,
        latency_percentage=latency_percentage,
        throughput_percentage=throughput_percentage,
    )


async def run_benchmarks():
    uri = "ws://localhost:8765"
    message_sizes = [1024, 4096, 8192, 16384, 32768]  # 1KB, 4KB, 8KB, 16KB, 32KB
    num_messages = 1000
    results: List[BenchmarkResult] = []
    comparisons: List[ComparisonResult] = []

    # 啟動服務器
    server_task = asyncio.create_task(start_echo_server())
    await asyncio.sleep(1)  # 等待服務器啟動

    try:
        # 進行多次測試以獲得更可靠的結果
        for run in range(3):
            logger.info(f"\n開始第 {run + 1} 輪測試")
            run_results = []

            for size in message_sizes:
                # 測試 Rust WebSocket
                rust_result = await benchmark_rust_websocket(uri, size, num_messages)
                run_results.append(rust_result)

                # 測試原生 Python WebSocket
                python_result = await benchmark_native_websocket(uri, size, num_messages)
                run_results.append(python_result)

                # 計算相對性能
                comparison = calculate_relative_performance(rust_result, python_result)

                logger.info(f"\n消息大小: {size} 字節")
                logger.info(
                    f"Rust 平均延遲: {rust_result.avg_latency_ms:.2f}ms (標準差: {rust_result.std_dev_latency_ms:.2f}ms)"
                )
                logger.info(f"Rust 吞吐量: {rust_result.throughput_mbps:.2f}Mbps")
                logger.info(
                    f"Python 平均延遲: {python_result.avg_latency_ms:.2f}ms (標準差: {python_result.std_dev_latency_ms:.2f}ms)"
                )
                logger.info(f"Python 吞吐量: {python_result.throughput_mbps:.2f}Mbps")
                logger.info(f"Python 相對延遲: {comparison.latency_percentage:.1f}% (Rust = 100%)")
                logger.info(f"Python 相對吞吐量: {comparison.throughput_percentage:.1f}% (Rust = 100%)")

                await asyncio.sleep(1)  # 冷卻時間

            # 將本輪結果添加到總結果
            results.extend(run_results)

            # 計算並添加本輪比較結果
            for i in range(0, len(run_results), 2):
                if i + 1 < len(run_results):
                    comp = calculate_relative_performance(run_results[i], run_results[i + 1])
                    comparisons.append(comp)

            await asyncio.sleep(2)  # 輪次之間的冷卻時間

    finally:
        # 停止服務器
        server_task.cancel()
        try:
            await server_task
        except asyncio.CancelledError:
            pass

    # 計算平均結果
    avg_results = {}
    for result in results:
        key = (result.message_size, result.implementation)
        if key not in avg_results:
            avg_results[key] = []
        avg_results[key].append(result)

    final_results = []
    for key, result_list in avg_results.items():
        size, impl = key
        avg_latency = sum(r.avg_latency_ms for r in result_list) / len(result_list)
        min_latency = min(r.min_latency_ms for r in result_list)
        max_latency = max(r.max_latency_ms for r in result_list)
        std_dev = sum(r.std_dev_latency_ms for r in result_list) / len(result_list)
        throughput = sum(r.throughput_mbps for r in result_list) / len(result_list)

        final_results.append(
            BenchmarkResult(
                message_size=size,
                total_messages=num_messages,
                avg_latency_ms=avg_latency,
                min_latency_ms=min_latency,
                max_latency_ms=max_latency,
                std_dev_latency_ms=std_dev,
                throughput_mbps=throughput,
                implementation=impl,
            )
        )

    # 計算最終比較結果
    final_comparisons = []
    for size in message_sizes:
        rust_result = next((r for r in final_results if r.message_size == size and r.implementation == "Rust"), None)
        python_result = next(
            (r for r in final_results if r.message_size == size and r.implementation == "Python Native"), None
        )

        if rust_result and python_result:
            final_comparisons.append(calculate_relative_performance(rust_result, python_result))

    # 保存結果
    output = {
        "raw_results": [asdict(r) for r in results],
        "avg_results": [asdict(r) for r in final_results],
        "comparisons": [asdict(c) for c in final_comparisons],
    }

    with open("benchmark_results.json", "wb") as f:
        f.write(orjson.dumps(output, option=orjson.OPT_INDENT_2))

    # 輸出總結
    logger.info("\n性能比較總結：")
    for comp in final_comparisons:
        logger.info(f"\n消息大小: {comp.message_size} 字節")
        logger.info(f"Python 相對延遲: {comp.latency_percentage:.1f}% (Rust = 100%)")
        logger.info(f"Python 相對吞吐量: {comp.throughput_percentage:.1f}% (Rust = 100%)")

    logger.info("\n基準測試完成，結果已保存到 benchmark_results.json")


if __name__ == "__main__":
    asyncio.run(run_benchmarks())
