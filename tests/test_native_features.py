"""Smoke-test native_client production features: headers, subprotocol, ping, close code."""

import asyncio
import multiprocessing as mp
import time

import pytest
import uvloop

from websocket_rs.native_client import connect

uvloop.install()

PORT = 8860


def _server_frame(first_byte, payload):
    if len(payload) <= 125:
        return bytes([first_byte, len(payload)]) + payload
    if len(payload) <= 65535:
        return bytes([first_byte, 126]) + len(payload).to_bytes(2, "big") + payload
    return bytes([first_byte, 127]) + len(payload).to_bytes(8, "big") + payload


def _start_raw_ws_server(port, frames, *, extra_headers=(), wait_for_client_data=False):
    import base64
    import socket as _s
    import threading
    from hashlib import sha1

    evt = threading.Event()

    def tiny_server():
        srv = _s.socket(_s.AF_INET, _s.SOCK_STREAM)
        srv.setsockopt(_s.SOL_SOCKET, _s.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", port))
        srv.listen(1)
        evt.set()
        conn, _ = srv.accept()
        try:
            data = b""
            while b"\r\n\r\n" not in data:
                data += conn.recv(4096)

            key_line = next(
                ln for ln in data.decode("latin-1").split("\r\n") if ln.lower().startswith("sec-websocket-key:")
            )
            key = key_line.split(":", 1)[1].strip()
            accept = base64.b64encode(sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
            response_headers = [
                "HTTP/1.1 101 Switching Protocols",
                "Upgrade: websocket",
                "Connection: Upgrade",
                f"Sec-WebSocket-Accept: {accept}",
                *extra_headers,
                "",
                "",
            ]
            conn.sendall("\r\n".join(response_headers).encode())
            if wait_for_client_data:
                conn.recv(4096)
            for frame in frames:
                conn.sendall(frame)
                time.sleep(0.01)
            time.sleep(0.2)
        finally:
            conn.close()
            srv.close()

    thread = threading.Thread(target=tiny_server, daemon=True)
    thread.start()
    evt.wait(timeout=2)
    return thread


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

        async with websockets.serve(echo, "127.0.0.1", port, select_subprotocol=select_subprotocol):
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
        assert resp == b"hello"
        assert hash(resp) == hash(b"hello")
        assert {b"hello": "hit"}[resp] == "hit"
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


def test_recv_and_anext_after_client_close(server):
    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}")
        ws.send(b"before-close")
        await ws.recv()
        ws.close()
        with pytest.raises(ConnectionError):
            await ws.recv()
        with pytest.raises(StopAsyncIteration):
            await ws.__anext__()

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


def test_receive_timeout_raises(server):
    """receive_timeout should trigger TimeoutError when no message arrives."""

    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT}", receive_timeout=0.3)
        # Don't send — server has nothing to echo, so recv should time out.
        t0 = time.perf_counter()
        try:
            await ws.recv()
            raise AssertionError("expected TimeoutError")
        except TimeoutError:
            elapsed = time.perf_counter() - t0
            assert 0.25 < elapsed < 1.0, f"timeout off: {elapsed}s"
        ws.close()

    asyncio.run(run())


def test_fragmented_message_assembled():
    """Hand-craft a fragmented binary message and ensure client reassembles it."""
    t = _start_raw_ws_server(
        8818,
        [
            _server_frame(0x02, b"hello"),
            _server_frame(0x80, b"world"),
        ],
        wait_for_client_data=True,
    )

    async def run():
        ws = await connect("ws://127.0.0.1:8818")
        ws.send(b"go")
        msg = await asyncio.wait_for(ws.recv(), timeout=2)
        assert bytes(msg) == b"helloworld", f"got {bytes(msg)!r}"
        ws.close()

    asyncio.run(run())
    t.join(timeout=2)


def test_protocol_error_for_unmatched_continuation_frame():
    """A continuation frame without a fragmented message should close with 1002."""
    port = 8819
    t = _start_raw_ws_server(port, [_server_frame(0x80, b"bad")])

    async def run():
        ws = await connect(f"ws://127.0.0.1:{port}")
        with pytest.raises(ConnectionError):
            await asyncio.wait_for(ws.recv(), timeout=2)
        assert ws.close_code == 1002

    asyncio.run(run())
    t.join(timeout=2)


def test_protocol_error_for_new_data_frame_during_fragment():
    """A new data frame during fragmented message assembly should close with 1002."""
    port = 8820
    t = _start_raw_ws_server(
        port,
        [
            _server_frame(0x02, b"one"),
            _server_frame(0x82, b"two"),
        ],
    )

    async def run():
        ws = await connect(f"ws://127.0.0.1:{port}")
        with pytest.raises(ConnectionError):
            await asyncio.wait_for(ws.recv(), timeout=2)
        assert ws.close_code == 1002

    asyncio.run(run())
    t.join(timeout=2)


def test_server_close_code_and_reason_tracked():
    port = 8821
    t = _start_raw_ws_server(port, [_server_frame(0x88, (1001).to_bytes(2, "big") + b"bye")])

    async def run():
        ws = await connect(f"ws://127.0.0.1:{port}")
        with pytest.raises(ConnectionError):
            await asyncio.wait_for(ws.recv(), timeout=2)
        assert ws.close_code == 1001
        assert ws.close_reason == "bye"

    asyncio.run(run())
    t.join(timeout=2)


def test_socks5_proxy_tunnelled_handshake(server):
    """Connect to the echo server THROUGH a minimal in-proc SOCKS5 proxy."""
    import threading

    from tests.bench_socks5_handshake import _serve_socks5  # reuse the no-auth proxy

    proxy_port = 19060
    ready = threading.Event()
    threading.Thread(target=_serve_socks5, args=(proxy_port, ready), daemon=True).start()
    ready.wait(timeout=2)

    async def run():
        ws = await connect(
            f"ws://127.0.0.1:{PORT}",
            proxy=f"socks5://127.0.0.1:{proxy_port}",
            connect_timeout=3.0,
        )
        ws.send(b"via-proxy")
        resp = await ws.recv()
        assert bytes(resp) == b"via-proxy"
        ws.close()

    asyncio.run(run())


def test_socks5_proxy_invalid_scheme_rejected():
    async def run():
        with pytest.raises(ValueError):
            await connect(
                "ws://127.0.0.1:1",
                proxy="http://127.0.0.1:9999",
                connect_timeout=1.0,
            )

    asyncio.run(run())


def test_invalid_uri_scheme_rejected():
    async def run():
        with pytest.raises(ValueError):
            await connect("http://127.0.0.1:1")

    asyncio.run(run())


def _compressed_echo_server(port, ready):
    import asyncio as _a

    import uvloop as _u
    import websockets as _ws
    from websockets.extensions.permessage_deflate import ServerPerMessageDeflateFactory

    _u.install()

    async def echo(ws):
        async for msg in ws:
            await ws.send(msg)

    async def main():
        async with _ws.serve(
            echo,
            "127.0.0.1",
            port,
            extensions=[
                ServerPerMessageDeflateFactory(
                    server_no_context_takeover=True,
                    client_no_context_takeover=True,
                )
            ],
        ):
            ready.set()
            await _a.Future()

    _a.run(main())


def _plain_echo_server(port, ready):
    import asyncio as _a

    import uvloop as _u
    import websockets as _ws

    _u.install()

    async def echo(ws):
        async for msg in ws:
            await ws.send(msg)

    async def main():
        async with _ws.serve(echo, "127.0.0.1", port, extensions=[]):
            ready.set()
            await _a.Future()

    _a.run(main())


def test_permessage_deflate_round_trip():
    """Run a compressed echo server and verify round-trip across size classes."""
    import multiprocessing as mp

    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    PORT_C = 8900
    p = ctx.Process(target=_compressed_echo_server, args=(PORT_C, ready), daemon=True)
    p.start()
    ready.wait(timeout=5)
    time.sleep(0.3)

    async def run():
        import os as _os

        ws = await connect(f"ws://127.0.0.1:{PORT_C}", compression=True)
        # Size sweep, including the 4KB boundary that initially tripped up miniz_oxide.
        for size in (10, 1000, 4096, 4097, 5000, 10000, 100000):
            msg = b"A" * size
            ws.send(msg)
            resp = await asyncio.wait_for(ws.recv(), timeout=5)
            assert bytes(resp) == msg, f"A*{size}: got {len(bytes(resp))}B"
        # Random (incompressible) input
        rnd = _os.urandom(8192)
        ws.send(rnd)
        resp = await asyncio.wait_for(ws.recv(), timeout=5)
        assert bytes(resp) == rnd
        # UTF-8 text round-trip
        tmsg = "Hello πρωτόκολλο " * 200
        ws.send(tmsg)
        resp = await asyncio.wait_for(ws.recv(), timeout=5)
        assert bytes(resp).decode() == tmsg
        ws.close()

    asyncio.run(run())
    p.terminate()
    p.join(timeout=2)


def test_compressed_fragmented_message_is_reassembled_and_decompressed():
    import zlib

    port = 8822
    msg = b"compressed-fragment-" * 200
    compressor = zlib.compressobj(wbits=-15)
    compressed = compressor.compress(msg) + compressor.flush(zlib.Z_SYNC_FLUSH)
    assert compressed.endswith(b"\x00\x00\xff\xff")
    compressed = compressed[:-4]
    mid = len(compressed) // 2
    t = _start_raw_ws_server(
        port,
        [
            _server_frame(0x42, compressed[:mid]),
            _server_frame(0x80, compressed[mid:]),
        ],
        extra_headers=("Sec-WebSocket-Extensions: permessage-deflate; server_no_context_takeover; client_no_context_takeover",),
    )

    async def run():
        ws = await connect(f"ws://127.0.0.1:{port}", compression=True)
        resp = await asyncio.wait_for(ws.recv(), timeout=2)
        assert bytes(resp) == msg
        ws.close()

    asyncio.run(run())
    t.join(timeout=2)


def test_permessage_deflate_graceful_fallback():
    """Server without deflate support: client should fall back to uncompressed transparently."""
    import multiprocessing as mp

    ctx = mp.get_context("spawn")
    ready = ctx.Event()
    PORT_C = 8901
    p = ctx.Process(target=_plain_echo_server, args=(PORT_C, ready), daemon=True)
    p.start()
    ready.wait(timeout=5)
    time.sleep(0.3)

    async def run():
        ws = await connect(f"ws://127.0.0.1:{PORT_C}", compression=True)
        msg = b"B" * 2000
        ws.send(msg)
        resp = await asyncio.wait_for(ws.recv(), timeout=5)
        assert bytes(resp) == msg
        ws.close()

    asyncio.run(run())
    p.terminate()
    p.join(timeout=2)


def test_connect_timeout_on_unreachable():
    # TEST-NET-1: 192.0.2.x guaranteed unroutable per RFC 5737.
    async def run():
        t0 = time.perf_counter()
        try:
            await connect("ws://192.0.2.1:9999", connect_timeout=0.3)
            raise AssertionError("expected timeout")
        except TimeoutError:
            elapsed = time.perf_counter() - t0
            # Should unblock well before the default OS connect timeout (~75s).
            assert elapsed < 2.0, f"timeout didn't fire: {elapsed}s"

    asyncio.run(run())
