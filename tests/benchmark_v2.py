#!/usr/bin/env python3
"""WebSocket benchmark v2 — addresses fairness issues raised in issue #11.

Fixes:
1. Echo server runs in isolated subprocess (no GIL contention with clients).
2. Unified sliding window (WINDOW) across all pipelined clients.
3. picows uses event-driven refill inside on_ws_frame — no asyncio.create_task per message.
4. Warmup increased to 200 messages.
5. Reports mean, p50, p99 RTT separately for pipelined vs request-response.
"""

import asyncio
import multiprocessing as mp
import statistics
import struct
import time

import uvloop

uvloop.install()

import websocket_rs.async_client
import websocket_rs.sync.client
import websockets
import websockets.sync.client
from websocket import create_connection

try:
    from picows import WSCloseCode, WSFrame, WSListener, WSMsgType, WSTransport, ws_connect

    PICOWS_AVAILABLE = True
except ImportError:
    PICOWS_AVAILABLE = False


WINDOW = 100
WARMUP = 200
NUM_MESSAGES = 1000
PORT = 8799
URI = f"ws://127.0.0.1:{PORT}"


# -------- Server in isolated subprocess --------


def _server_proc(port: int, ready_evt):
    """Run a websockets echo server with server-side timestamps in a dedicated process."""
    import asyncio

    import uvloop
    import websockets

    uvloop.install()

    async def echo(ws):
        try:
            async for msg in ws:
                t_recv = time.perf_counter()
                if isinstance(msg, str):
                    msg = msg.encode()
                msg_id = struct.unpack("=I", msg[:4])[0] if len(msg) >= 4 else 0
                t_send = time.perf_counter()
                header = struct.pack("=IddI", msg_id, t_recv, t_send, len(msg))
                await ws.send(header + msg)
        except websockets.exceptions.ConnectionClosedError:
            pass

    async def main():
        async with websockets.serve(echo, "127.0.0.1", port):
            ready_evt.set()
            await asyncio.Future()

    asyncio.run(main())


# -------- Parsing & stats --------


def parse_response(response: bytes, payload_len: int):
    if isinstance(response, str):
        response = response.encode()
    msg_id, server_recv_time, server_send_time, message_len = struct.unpack("=IddI", response[:24])
    if len(response) - 24 != message_len or message_len != payload_len + 4:
        raise ValueError(f"payload mismatch: got {len(response) - 24}, expected {payload_len + 4}")
    return msg_id, server_recv_time, server_send_time


def stats(rtt_lats: dict[int, float], send_lats: dict[int, float], recv_lats: dict[int, float], impl: str):
    rtts = list(rtt_lats.values())
    return {
        "impl": impl,
        "send": statistics.mean(send_lats.values()),
        "recv": statistics.mean(recv_lats.values()),
        "rtt_mean": statistics.mean(rtts),
        "rtt_p50": statistics.median(rtts),
        "rtt_p99": statistics.quantiles(rtts, n=100)[98] if len(rtts) >= 100 else max(rtts),
    }


# -------- Pipelined clients (sliding window = WINDOW) --------


async def bench_websockets_pipelined(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    client_send_times = {}

    async with websockets.connect(uri) as ws:
        for i in range(WARMUP):
            await ws.send(struct.pack("=I", 9999) + b"w")
            await ws.recv()

        sent = recv = 0
        while recv < n:
            while sent < n and (sent - recv) < WINDOW:
                client_send_times[sent] = time.perf_counter()
                await ws.send(struct.pack("=I", sent) + base)
                sent += 1
            response = await ws.recv()
            t_recv = time.perf_counter()
            recv += 1
            mid, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - client_send_times[mid]) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - client_send_times[mid]) * 1000

    return stats(rtt_lats, send_lats, recv_lats, "websockets")


async def bench_websocket_rs_pipelined(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    client_send_times = {}

    ws = await websocket_rs.async_client.connect(uri)
    try:
        for i in range(WARMUP):
            await ws.send(struct.pack("=I", 9999) + b"w")
            await ws.recv()

        sent = recv = 0
        while recv < n:
            while sent < n and (sent - recv) < WINDOW:
                client_send_times[sent] = time.perf_counter()
                await ws.send(struct.pack("=I", sent) + base)
                sent += 1
            response = await ws.recv()
            t_recv = time.perf_counter()
            recv += 1
            mid, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - client_send_times[mid]) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - client_send_times[mid]) * 1000
    finally:
        await ws.close()
    return stats(rtt_lats, send_lats, recv_lats, "websocket-rs")


async def bench_picows_pipelined(uri: str, size: int, n: int) -> dict:
    """Event-driven sliding window — refill inside on_ws_frame (no asyncio.create_task per msg)."""
    base_msg = bytearray(b"a") * (size + 4)
    base_mv = memoryview(base_msg)
    done_fut: asyncio.Future = asyncio.get_running_loop().create_future()

    client_send_times: dict[int, float] = {}
    send_lats: dict[int, float] = {}
    recv_lats: dict[int, float] = {}
    rtt_lats: dict[int, float] = {}

    class Listener(WSListener):
        def __init__(self):
            self.transport: WSTransport | None = None
            self.sent = 0
            self.received = 0
            self.in_warmup = True
            self.warmup_done = 0

        def on_ws_connected(self, transport: WSTransport):
            self.transport = transport
            for _ in range(WARMUP):
                base_mv[:4] = struct.pack("=I", 9999)
                transport.send(WSMsgType.BINARY, base_msg)

        def on_ws_frame(self, transport: WSTransport, frame: WSFrame):
            if frame.msg_type == WSMsgType.CLOSE:
                if not done_fut.done():
                    done_fut.set_result(None)
                return
            if self.in_warmup:
                self.warmup_done += 1
                if self.warmup_done >= WARMUP:
                    self.in_warmup = False
                    self._fill()
                return

            t_recv = time.perf_counter()
            payload = bytes(frame.get_payload_as_memoryview())
            mid, t_srv_recv, t_srv_send = parse_response(payload, size)
            send_lats[mid] = (t_srv_recv - client_send_times[mid]) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - client_send_times[mid]) * 1000
            self.received += 1

            if self.received >= n:
                transport.send_close(WSCloseCode.OK)
                transport.disconnect()
                if not done_fut.done():
                    done_fut.set_result(None)
                return
            self._fill()

        def _fill(self):
            while self.sent < n and (self.sent - self.received) < WINDOW:
                base_mv[:4] = struct.pack("=I", self.sent)
                client_send_times[self.sent] = time.perf_counter()
                self.transport.send(WSMsgType.BINARY, base_msg)
                self.sent += 1

    transport, listener = await ws_connect(Listener, uri)
    try:
        await done_fut
    finally:
        transport.disconnect()
    return stats(rtt_lats, send_lats, recv_lats, "picows")


# -------- Request-Response clients --------


async def bench_websocket_rs_rr(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}

    ws = await websocket_rs.async_client.connect(uri)
    try:
        for i in range(WARMUP):
            await ws.send(struct.pack("=I", 9999) + b"w")
            await ws.recv()
        for mid in range(n):
            t_send = time.perf_counter()
            await ws.send(struct.pack("=I", mid) + base)
            response = await ws.recv()
            t_recv = time.perf_counter()
            _, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - t_send) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - t_send) * 1000
    finally:
        await ws.close()
    return stats(rtt_lats, send_lats, recv_lats, "websocket-rs RR")


async def bench_websockets_rr(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    async with websockets.connect(uri) as ws:
        for i in range(WARMUP):
            await ws.send(struct.pack("=I", 9999) + b"w")
            await ws.recv()
        for mid in range(n):
            t_send = time.perf_counter()
            await ws.send(struct.pack("=I", mid) + base)
            response = await ws.recv()
            t_recv = time.perf_counter()
            _, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - t_send) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - t_send) * 1000
    return stats(rtt_lats, send_lats, recv_lats, "websockets RR")


async def bench_picows_rr(uri: str, size: int, n: int) -> dict:
    """RR mode: send one, wait for response via a Future, send next. No asyncio.create_task."""
    base_msg = bytearray(b"a") * (size + 4)
    base_mv = memoryview(base_msg)
    loop = asyncio.get_running_loop()

    state = {"transport": None, "fut": None, "payload": None, "t_recv": 0.0}

    class Listener(WSListener):
        def on_ws_connected(self, transport: WSTransport):
            state["transport"] = transport

        def on_ws_frame(self, transport: WSTransport, frame: WSFrame):
            if frame.msg_type == WSMsgType.CLOSE:
                return
            state["t_recv"] = time.perf_counter()
            state["payload"] = bytes(frame.get_payload_as_memoryview())
            fut = state["fut"]
            if fut is not None and not fut.done():
                fut.set_result(None)

    transport, _ = await ws_connect(Listener, uri)
    try:
        # Warmup
        for i in range(WARMUP):
            base_mv[:4] = struct.pack("=I", 9999)
            state["fut"] = loop.create_future()
            transport.send(WSMsgType.BINARY, base_msg)
            await state["fut"]

        send_lats, recv_lats, rtt_lats = {}, {}, {}
        for mid in range(n):
            base_mv[:4] = struct.pack("=I", mid)
            state["fut"] = loop.create_future()
            t_send = time.perf_counter()
            transport.send(WSMsgType.BINARY, base_msg)
            await state["fut"]
            t_recv = state["t_recv"]
            _, t_srv_recv, t_srv_send = parse_response(state["payload"], size)
            send_lats[mid] = (t_srv_recv - t_send) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - t_send) * 1000
    finally:
        transport.send_close(WSCloseCode.OK)
        transport.disconnect()
    return stats(rtt_lats, send_lats, recv_lats, "picows RR")


# -------- Sync clients (RR only) --------


def bench_websocket_rs_sync(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    with websocket_rs.sync.client.connect(uri) as ws:
        for i in range(WARMUP):
            ws.send(struct.pack("=I", 9999) + b"w")
            ws.recv()
        for mid in range(n):
            t_send = time.perf_counter()
            ws.send(struct.pack("=I", mid) + base)
            response = ws.recv()
            t_recv = time.perf_counter()
            _, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - t_send) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - t_send) * 1000
    return stats(rtt_lats, send_lats, recv_lats, "websocket-rs Sync")


def bench_websocket_client_sync(uri: str, size: int, n: int) -> dict:
    base = b"a" * size
    send_lats, recv_lats, rtt_lats = {}, {}, {}
    ws = create_connection(uri)
    try:
        for i in range(WARMUP):
            ws.send_binary(struct.pack("=I", 9999) + b"w")
            ws.recv()
        for mid in range(n):
            t_send = time.perf_counter()
            ws.send_binary(struct.pack("=I", mid) + base)
            response = ws.recv()
            t_recv = time.perf_counter()
            _, t_srv_recv, t_srv_send = parse_response(response, size)
            send_lats[mid] = (t_srv_recv - t_send) * 1000
            recv_lats[mid] = (t_recv - t_srv_send) * 1000
            rtt_lats[mid] = (t_recv - t_send) * 1000
    finally:
        ws.close()
    return stats(rtt_lats, send_lats, recv_lats, "websocket-client")


# -------- Runner --------


def _print(r: dict):
    print(
        f"  {r['impl']:20} | send {r['send']:7.3f} | recv {r['recv']:7.3f} "
        f"| mean {r['rtt_mean']:7.3f} | p50 {r['rtt_p50']:7.3f} | p99 {r['rtt_p99']:7.3f} ms"
    )


async def main():
    sizes = [512, 1024, 4096, 16384, 65536]

    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    proc = ctx.Process(target=_server_proc, args=(PORT, ready), daemon=True)
    proc.start()
    ready.wait(timeout=5)
    await asyncio.sleep(0.2)

    print(f"Benchmark v2 — server in subprocess pid={proc.pid}")
    print(f"window={WINDOW} warmup={WARMUP} messages={NUM_MESSAGES}\n")

    try:
        for size in sizes:
            print(f"=== size={size}B ===")
            print("  -- Request-Response --")
            _print(await asyncio.to_thread(lambda s=size: asyncio.run(bench_websockets_rr(URI, s, NUM_MESSAGES))))
            _print(await asyncio.to_thread(bench_websocket_client_sync, URI, size, NUM_MESSAGES))
            if PICOWS_AVAILABLE:
                _print(await asyncio.to_thread(lambda s=size: asyncio.run(bench_picows_rr(URI, s, NUM_MESSAGES))))
            _print(await bench_websocket_rs_rr(URI, size, NUM_MESSAGES))
            _print(await asyncio.to_thread(bench_websocket_rs_sync, URI, size, NUM_MESSAGES))

            print("  -- Pipelined (window=100) --")
            _print(await asyncio.to_thread(lambda s=size: asyncio.run(bench_websockets_pipelined(URI, s, NUM_MESSAGES))))
            if PICOWS_AVAILABLE:
                _print(await asyncio.to_thread(lambda s=size: asyncio.run(bench_picows_pipelined(URI, s, NUM_MESSAGES))))
            _print(await bench_websocket_rs_pipelined(URI, size, NUM_MESSAGES))
            print()
    finally:
        proc.terminate()
        proc.join(timeout=3)


if __name__ == "__main__":
    asyncio.run(main())
