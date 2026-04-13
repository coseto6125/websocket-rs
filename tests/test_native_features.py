"""Smoke-test native_client production features: headers, subprotocol, ping, close code."""

import asyncio
import multiprocessing as mp
import struct
import time

import pytest
import uvloop

uvloop.install()

from websocket_rs.native_client import connect

PORT = 8860


def _server(port, ready, cfg):
    import asyncio

    import uvloop
    import websockets

    uvloop.install()

    async def echo(ws):
        # Record handshake info for later verification
        cfg["user_agent"] = ws.request.headers.get("User-Agent")
        cfg["custom"] = ws.request.headers.get("X-Custom-Header")
        cfg["selected_subprotocol"] = ws.subprotocol
        try:
            async for msg in ws:
                if isinstance(msg, str):
                    msg = msg.encode()
                await ws.send(msg)
        except Exception:
            pass

    async def main():
        def select_subprotocol(ws, offered):
            # Accept any offered subprotocol or none
            for p in ("trade-v1", "chat"):
                if p in offered:
                    return p
            return None

        async with websockets.serve(
            echo, "127.0.0.1", port, select_subprotocol=select_subprotocol
        ):
            ready.set()
            await asyncio.Future()

    asyncio.run(main())


@pytest.fixture(scope="module")
def server():
    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    mgr = ctx.Manager()
    cfg = mgr.dict()
    p = ctx.Process(target=_server, args=(PORT, ready, cfg), daemon=True)
    p.start()
    ready.wait(timeout=5)
    time.sleep(0.2)
    yield cfg
    p.terminate()
    p.join(timeout=2)


def test_headers_and_subprotocol(server):
    async def run():
        ws = await connect(
            f"ws://127.0.0.1:{PORT}",
            headers=[("User-Agent", "websocket-rs-test"), ("X-Custom-Header", "abc")],
            subprotocols=["trade-v1", "chat"],
        )
        ws.send(b"hello")
        resp = await ws.recv()
        assert bytes(resp) == b"hello"
        # Server should have selected one of our offered subprotocols
        assert ws.subprotocol in ("trade-v1", "chat")
        ws.close()

    asyncio.run(run())
    assert server["user_agent"] == "websocket-rs-test"
    assert server["custom"] == "abc"


def test_close_code_tracked(server):
    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}")
        ws.send(b"x")
        await ws.recv()
        ws.close()
        # Give server a moment to send close-frame reply (most servers echo close)
        await asyncio.sleep(0.1)
        # After closing from our side, is_open must be False
        assert not ws.is_open

    asyncio.run(run())


def test_ping_does_not_error(server):
    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}")
        ws.ping()  # no payload
        ws.ping(b"keepalive")  # 9-byte payload
        ws.send(b"after-ping")
        resp = await ws.recv()
        assert bytes(resp) == b"after-ping"
        ws.close()

    asyncio.run(run())


def test_ping_payload_too_large_rejected(server):
    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}")
        with pytest.raises(ValueError):
            ws.ping(b"x" * 200)
        ws.close()

    asyncio.run(run())


def test_async_for_iteration(server):
    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}")
        for i in range(5):
            ws.send(f"msg-{i}".encode())
        received = []
        n = 0
        async for msg in ws:
            received.append(bytes(msg))
            n += 1
            if n >= 5:
                ws.close()
                break
        assert received == [b"msg-0", b"msg-1", b"msg-2", b"msg-3", b"msg-4"]

    asyncio.run(run())


def test_async_context_manager(server):
    async def run():
        async with await connect(f"ws://127.0.0.1:{PORT}") as ws:
            ws.send(b"hi")
            resp = await ws.recv()
            assert bytes(resp) == b"hi"
        # After context exit, ws.close() should have been invoked
        assert not ws.is_open

    asyncio.run(run())


def test_connect_timeout_on_unreachable():
    # TEST-NET-1: 192.0.2.x guaranteed unroutable per RFC 5737.
    async def run():
        t0 = time.perf_counter()
        try:
            await connect("ws://192.0.2.1:9999", connect_timeout=0.3)
            raise AssertionError("expected timeout")
        except (asyncio.TimeoutError, TimeoutError):
            elapsed = time.perf_counter() - t0
            # Should unblock well before the default OS connect timeout (~75s).
            assert elapsed < 2.0, f"timeout didn't fire: {elapsed}s"

    asyncio.run(run())
