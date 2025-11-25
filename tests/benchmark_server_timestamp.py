#!/usr/bin/env python3
"""
WebSocket 效能測試 - 使用 Server 時間戳驗證

測試方法:
1. Client 記錄發送時間
2. Server 記錄接收時間和發送時間
3. Client 記錄接收時間

測量指標:
- 發送延遲: Client發送 → Server接收
- 接收延遲: Server發送 → Client接收
- RTT: 完整往返時間
"""

import uvloop

uvloop.install()
import asyncio
import statistics
import struct
import time

import websocket_rs.async_client
import websocket_rs.sync.client
import websockets
import websockets.sync.client
from websocket import create_connection

try:
    from picows import WSFrame, WSListener, WSMsgType, ws_connect

    PICOWS_AVAILABLE = True
except ImportError:
    PICOWS_AVAILABLE = False


def parse_and_verify_response(response, expected_payload_len, expected_msg_id=None, verify_content=True):
    """從 server 回應中解析時間戳、訊息 ID 並驗證資料完整性"""
    if isinstance(response, str):
        response = response.encode()

    # 驗證最小長度
    if len(response) < 24:
        raise ValueError(f"回應長度不足: 需要至少 24 bytes, 收到 {len(response)} bytes")

    # 解析: [msg_id 4B][recv 8B][send 8B][len 4B][message]
    msg_id, server_recv_time, server_send_time, message_len = struct.unpack("=IddI", response[:24])
    actual_message = response[24:]

    # 驗證訊息 ID（如果提供）
    if expected_msg_id is not None and msg_id != expected_msg_id:
        raise ValueError(f"訊息 ID 不符: 預期 {expected_msg_id}, 收到 {msg_id}")

    # 驗證資料長度
    if len(actual_message) != message_len:
        raise ValueError(f"訊息長度不符: server 回報 {message_len}, 實際 {len(actual_message)}")

    # 驗證 payload 長度（排除 msg_id）
    if verify_content and len(actual_message) != expected_payload_len + 4:  # +4 for msg_id
        raise ValueError(f"Payload 長度不符: 預期 {expected_payload_len + 4}, 收到 {len(actual_message)}")

    return msg_id, server_recv_time, server_send_time


async def start_timestamp_server(host: str = "localhost", port: int = 8765):
    """啟動帶時間戳和訊息 ID 的 echo server"""

    async def echo_with_timestamps(websocket):
        try:
            async for message in websocket:
                server_recv_time = time.perf_counter()

                # 轉換為 bytes
                if isinstance(message, str):
                    message_bytes = message.encode()
                else:
                    message_bytes = message

                # 解析訊息 ID（前 4 bytes）
                if len(message_bytes) >= 4:
                    msg_id = struct.unpack("=I", message_bytes[:4])[0]
                else:
                    msg_id = 0  # warmup 訊息

                server_send_time = time.perf_counter()

                # 打包: [msg_id 4B][recv 8B][send 8B][message_len 4B][原始訊息]
                response_header = struct.pack("=IddI", msg_id, server_recv_time, server_send_time, len(message_bytes))
                response = response_header + message_bytes

                await websocket.send(response)
        except websockets.exceptions.ConnectionClosedError:
            pass

    async with websockets.serve(echo_with_timestamps, host, port):
        await asyncio.Future()


async def benchmark_websockets_async(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websockets (async) - 使用滑動窗口避免緩衝區阻塞"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    client_send_times = {}
    received_ids = set()

    # 限制同時在傳輸中的訊息數量（避免緩衝區滿）
    max_in_flight = 100

    async with websockets.connect(uri) as ws:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            await ws.send(warmup_msg)
            await ws.recv()

        sent_count = 0
        recv_count = 0

        # 使用滑動窗口：發送和接收並行進行
        while recv_count < num_messages:
            # 發送訊息直到達到窗口上限或全部發送完成
            while sent_count < num_messages and (sent_count - recv_count) < max_in_flight:
                message = struct.pack("=I", sent_count) + base_message
                client_send_times[sent_count] = time.perf_counter()
                await ws.send(message)
                sent_count += 1

            # 接收一筆回應
            response = await ws.recv()
            t_recv = time.perf_counter()
            recv_count += 1

            # 驗證並解析
            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=None, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            if recv_msg_id >= num_messages:
                raise ValueError(f"無效的訊息 ID: {recv_msg_id}")

            # 計算延遲
            send_lats[recv_msg_id] = (server_recv_time - client_send_times[recv_msg_id]) * 1000
            recv_lats[recv_msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[recv_msg_id] = (t_recv - client_send_times[recv_msg_id]) * 1000

    # 驗證完整性
    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websockets (Async)",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def benchmark_websockets_request_response(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websockets (Request-Response 模式) - 真實場景"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    received_ids = set()

    async with websockets.connect(uri) as ws:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            await ws.send(warmup_msg)
            await ws.recv()

        # Request-Response 模式：發送一筆 → 等待回應 → 發下一筆
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            await ws.send(message)

            response = await ws.recv()
            t_recv = time.perf_counter()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000

    # 驗證完整性
    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websockets (RR)",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


def benchmark_websockets_sync(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websockets (sync)"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    received_ids = set()

    with websockets.sync.client.connect(uri) as ws:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            ws.send(warmup_msg)
            ws.recv()  # 丟棄 warmup 回應

        # 測試（同步模式按順序收發）
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            ws.send(message)
            response = ws.recv()
            t_recv = time.perf_counter()

            # 驗證並解析
            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            # 檢查重複
            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            # 計算延遲
            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000

    # 驗證完整性
    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websockets (Sync)",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


def benchmark_websocket_client(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websocket-client (sync)"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    received_ids = set()

    ws = create_connection(uri)
    try:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            ws.send_binary(warmup_msg)
            ws.recv()

        # 測試
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            ws.send_binary(message)
            response = ws.recv()
            t_recv = time.perf_counter()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000
    finally:
        ws.close()

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websocket-client",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


def benchmark_websocket_rs_sync(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websocket-rs (sync)"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    received_ids = set()

    with websocket_rs.sync.client.connect(uri) as ws:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            ws.send(warmup_msg)
            ws.recv()

        # 測試
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            ws.send(message)
            response = ws.recv()
            t_recv = time.perf_counter()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websocket-rs Sync",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def benchmark_websocket_rs_async(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websocket-rs (async) - 使用滑動窗口避免緩衝區阻塞"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    client_send_times = {}
    received_ids = set()
    max_in_flight = 100

    ws = await websocket_rs.async_client.connect(uri)
    try:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            await ws.send(warmup_msg)
            await ws.recv()

        sent_count = 0
        recv_count = 0

        while recv_count < num_messages:
            while sent_count < num_messages and (sent_count - recv_count) < max_in_flight:
                message = struct.pack("=I", sent_count) + base_message
                client_send_times[sent_count] = time.perf_counter()
                await ws.send(message)
                sent_count += 1

            response = await ws.recv()
            t_recv = time.perf_counter()
            recv_count += 1

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=None, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            if recv_msg_id >= num_messages:
                raise ValueError(f"無效的訊息 ID: {recv_msg_id}")

            send_lats[recv_msg_id] = (server_recv_time - client_send_times[recv_msg_id]) * 1000
            recv_lats[recv_msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[recv_msg_id] = (t_recv - client_send_times[recv_msg_id]) * 1000
    finally:
        await ws.close()

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websocket-rs Async",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def benchmark_websocket_rs_request_response(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 websocket-rs (Request-Response 模式) - 真實場景"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    received_ids = set()

    ws = await websocket_rs.async_client.connect(uri)
    try:
        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            await ws.send(warmup_msg)
            await ws.recv()

        # Request-Response 模式：發送一筆 → 等待回應 → 發下一筆
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            await ws.send(message)

            response = await ws.recv()
            t_recv = time.perf_counter()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000
    finally:
        await ws.close()

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "websocket-rs (RR)",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def benchmark_picows(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 picows - 真正的異步模式：先發送所有訊息再接收（事件驅動）"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    ws_transport = None
    response_queue = asyncio.Queue()
    client_send_times = {}
    received_ids = set()

    class BenchListener(WSListener):
        def on_ws_connected(self, transport):
            nonlocal ws_transport
            ws_transport = transport

        def on_ws_frame(self, transport, frame: WSFrame):
            if frame.msg_type in (WSMsgType.TEXT, WSMsgType.BINARY):
                data = frame.get_payload_as_bytes()
                t_recv = time.perf_counter()
                asyncio.create_task(response_queue.put((t_recv, data)))

    (transport, _) = await ws_connect(lambda: BenchListener(), uri)

    try:
        while ws_transport is None:
            await asyncio.sleep(0.001)

        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            ws_transport.send(WSMsgType.BINARY, warmup_msg)
            await response_queue.get()

        # 發送所有訊息
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            client_send_times[msg_id] = time.perf_counter()
            ws_transport.send(WSMsgType.BINARY, message)

        # 接收所有回應（可能亂序）
        for _ in range(num_messages):
            t_recv, response = await response_queue.get()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=None, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            if recv_msg_id >= num_messages:
                raise ValueError(f"無效的訊息 ID: {recv_msg_id}")

            send_lats[recv_msg_id] = (server_recv_time - client_send_times[recv_msg_id]) * 1000
            recv_lats[recv_msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[recv_msg_id] = (t_recv - client_send_times[recv_msg_id]) * 1000
    finally:
        transport.disconnect()

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "picows",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def benchmark_picows_request_response(uri: str, message_size: int, num_messages: int) -> dict:
    """測試 picows (Request-Response 模式) - 真實場景"""
    base_message = b"a" * message_size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    ws_transport = None
    response_queue = asyncio.Queue()
    received_ids = set()

    class BenchListener(WSListener):
        def on_ws_connected(self, transport):
            nonlocal ws_transport
            ws_transport = transport

        def on_ws_frame(self, transport, frame: WSFrame):
            if frame.msg_type in (WSMsgType.TEXT, WSMsgType.BINARY):
                data = frame.get_payload_as_bytes()
                t_recv = time.perf_counter()
                asyncio.create_task(response_queue.put((t_recv, data)))

    (transport, _) = await ws_connect(lambda: BenchListener(), uri)

    try:
        while ws_transport is None:
            await asyncio.sleep(0.001)

        # 預熱
        for warmup_id in range(10):
            warmup_msg = struct.pack("=I", 9999 + warmup_id) + b"warmup"
            ws_transport.send(WSMsgType.BINARY, warmup_msg)
            await response_queue.get()

        # Request-Response 模式：發送一筆 → 等待回應 → 發下一筆
        for msg_id in range(num_messages):
            message = struct.pack("=I", msg_id) + base_message
            t_send = time.perf_counter()
            ws_transport.send(WSMsgType.BINARY, message)

            # 等待這筆訊息的回應
            t_recv, response = await response_queue.get()

            recv_msg_id, server_recv_time, server_send_time = parse_and_verify_response(
                response, len(base_message), expected_msg_id=msg_id, verify_content=True
            )

            if recv_msg_id in received_ids:
                raise ValueError(f"重複收到訊息 ID: {recv_msg_id}")
            received_ids.add(recv_msg_id)

            send_lats[msg_id] = (server_recv_time - t_send) * 1000
            recv_lats[msg_id] = (t_recv - server_send_time) * 1000
            rtt_lats[msg_id] = (t_recv - t_send) * 1000
    finally:
        transport.disconnect()

    if len(received_ids) != num_messages:
        raise ValueError(f"訊息遺失: 發送 {num_messages}, 收到 {len(received_ids)}")

    return {
        "impl": "picows (RR)",
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt": statistics.mean(rtt_lats.values()),
    }


async def main():
    uri = "ws://localhost:8765"
    message_sizes = [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536]
    num_messages = 1000

    server_task = asyncio.create_task(start_timestamp_server())
    await asyncio.sleep(1)

    try:
        print("=== WebSocket 效能測試 (Server時間戳驗證 + 訊息ID追蹤) ===\n")
        print("測試方法: Server 端記錄接收/發送時間戳,客戶端驗證實際延遲")
        print("訊息驗證:")
        print("  - 每筆訊息帶唯一 ID")
        print("  - 驗證所有訊息都收到且無重複")
        print("  - 驗證訊息長度和內容完整性")
        print("  - 允許亂序接收(異步實作)")
        print("測試模式:")
        print("  - Pipelined: 滑動窗口批次收發（測試併發能力）")
        print("  - Request-Response (RR): 發送→等待→發送（真實場景）")
        print("  - Sync: 阻塞式逐筆收發")
        print("測量指標: 發送延遲(C→S) | 接收延遲(S→C) | RTT(往返)")
        if PICOWS_AVAILABLE:
            print("✓ picows 可用")
        print()

        for size in message_sizes:
            print(f"\n訊息大小: {size} bytes, 測試次數: {num_messages}")
            print("-" * 100)
            print(f"{'Implementation':22} | {'發送 (C→S)':12} | {'接收 (S→C)':12} | {'RTT':10}")
            print("-" * 100)

            # websockets Request-Response
            result = await asyncio.to_thread(lambda s=size: asyncio.run(benchmark_websockets_request_response(uri, s, num_messages)))
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            # websockets sync
            result = await asyncio.to_thread(benchmark_websockets_sync, uri, size, num_messages)
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            # websocket-client
            result = await asyncio.to_thread(benchmark_websocket_client, uri, size, num_messages)
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            # picows Request-Response
            if PICOWS_AVAILABLE:
                try:
                    result = await asyncio.wait_for(benchmark_picows_request_response(uri, size, num_messages), timeout=60.0)
                    print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")
                except TimeoutError:
                    print(f"{'picows (RR)':22} | ERROR: 測試超時 (60秒)")
                except Exception as e:
                    print(f"{'picows (RR)':22} | ERROR: {type(e).__name__}: {str(e)[:50]}")

            # websocket-rs Request-Response
            result = await benchmark_websocket_rs_request_response(uri, size, num_messages)
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            # websocket-rs sync
            result = await asyncio.to_thread(benchmark_websocket_rs_sync, uri, size, num_messages)
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            print("\n  --- Pipelined 模式（併發測試） ---")

            # websockets Pipelined
            result = await asyncio.to_thread(lambda s=size: asyncio.run(benchmark_websockets_async(uri, s, num_messages)))
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

            # picows Pipelined
            if PICOWS_AVAILABLE:
                try:
                    result = await asyncio.wait_for(benchmark_picows(uri, size, num_messages), timeout=60.0)
                    print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")
                except TimeoutError:
                    print(f"{'picows':22} | ERROR: 測試超時 (60秒)")
                except Exception as e:
                    print(f"{'picows':22} | ERROR: {type(e).__name__}: {str(e)[:50]}")

            # websocket-rs Pipelined
            result = await benchmark_websocket_rs_async(uri, size, num_messages)
            print(f"{result['impl']:22} | {result['send']:9.3f} ms | {result['recv']:9.3f} ms | {result['rtt']:7.3f} ms")

    finally:
        server_task.cancel()
        try:
            await server_task
        except asyncio.CancelledError:
            pass


if __name__ == "__main__":
    asyncio.run(main())
