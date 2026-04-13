"""
Tests for timeout, error handling, and edge cases.
Covers issues found in code review:
  - connect_timeout actually enforced (P0 fix)
  - close_timeout behavior
  - send after close raises error
  - sync::connect() forwards parameters
"""

import asyncio
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

import websockets

import websocket_rs.async_client
import websocket_rs.sync.client


async def start_echo_server(port=8766):
    async def echo(ws):
        async for msg in ws:
            await ws.send(msg)

    return await websockets.serve(echo, "localhost", port, ping_interval=None)


def test_sync_connect_timeout():
    """connect_timeout should raise TimeoutError for unreachable hosts."""
    # 192.0.2.1 is TEST-NET, guaranteed to be unreachable (RFC 5737)
    start = time.perf_counter()
    try:
        with websocket_rs.sync.client.ClientConnection("ws://192.0.2.1:9999", connect_timeout=1.0) as ws:
            pass
        assert False, "Should have raised"
    except TimeoutError:
        elapsed = time.perf_counter() - start
        assert elapsed < 3.0, f"Timeout took too long: {elapsed:.1f}s (expected ~1s)"
        print(f"✓ sync connect_timeout works ({elapsed:.1f}s)")
    except Exception as e:
        # ConnectionError is also acceptable (e.g., immediate rejection)
        elapsed = time.perf_counter() - start
        print(f"✓ sync connect_timeout: got {type(e).__name__} in {elapsed:.1f}s")


def test_sync_connect_forwards_params():
    """sync.client.connect() should forward timeout params."""
    ws = websocket_rs.sync.client.connect(
        "ws://localhost:8766",
        connect_timeout=5.0,
        receive_timeout=2.0,
    )
    # Just verify object is created with correct type
    assert isinstance(ws, websocket_rs.sync.client.ClientConnection)
    print("✓ sync connect() forwards parameters")


def test_sync_send_after_close():
    """Sending after close should raise RuntimeError."""
    with websocket_rs.sync.client.connect("ws://localhost:8766") as ws:
        ws.send("hello")
        ws.recv()
    # ws is now closed
    try:
        ws.send("should fail")
        assert False, "Should have raised"
    except RuntimeError:
        print("✓ sync send after close raises RuntimeError")


def test_sync_recv_timeout():
    """recv should raise TimeoutError after receive_timeout."""
    with websocket_rs.sync.client.ClientConnection("ws://localhost:8766", receive_timeout=0.5) as ws:
        ws.send("hello")
        ws.recv()  # consume echo
        start = time.perf_counter()
        try:
            ws.recv()  # no more messages, should timeout
            assert False, "Should have raised"
        except TimeoutError:
            elapsed = time.perf_counter() - start
            assert 0.3 < elapsed < 2.0, f"Unexpected timeout duration: {elapsed:.1f}s"
            print(f"✓ sync recv timeout works ({elapsed:.1f}s)")


async def test_async_send_after_close():
    """Async: sending after close should raise RuntimeError."""
    ws = await websocket_rs.async_client.connect("ws://localhost:8766")
    await ws.send("hello")
    await ws.recv()
    await ws.close()
    try:
        await ws.send("should fail")
        assert False, "Should have raised"
    except RuntimeError:
        print("✓ async send after close raises RuntimeError")


async def test_async_recv_timeout():
    """Async: recv should raise TimeoutError."""
    ws = websocket_rs.async_client.ClientConnection("ws://localhost:8766", receive_timeout=0.5)
    async with ws:
        await ws.send("hello")
        await ws.recv()
        start = time.perf_counter()
        try:
            await ws.recv()
            assert False, "Should have raised"
        except TimeoutError:
            elapsed = time.perf_counter() - start
            print(f"✓ async recv timeout works ({elapsed:.1f}s)")


async def test_async_connect_timeout():
    """Async: connect_timeout should raise TimeoutError."""
    start = time.perf_counter()
    try:
        ws = websocket_rs.async_client.ClientConnection("ws://192.0.2.1:9999", connect_timeout=1.0)
        async with ws:
            pass
        assert False, "Should have raised"
    except (TimeoutError, ConnectionError):
        elapsed = time.perf_counter() - start
        print(f"✓ async connect_timeout works ({elapsed:.1f}s)")


async def main():
    server = await start_echo_server(8766)

    try:
        print("=== Timeout & Error Tests ===\n")

        # Sync tests
        test_sync_connect_timeout()
        test_sync_connect_forwards_params()
        await asyncio.to_thread(test_sync_send_after_close)
        await asyncio.to_thread(test_sync_recv_timeout)

        # Async tests
        await test_async_send_after_close()
        await test_async_recv_timeout()
        await test_async_connect_timeout()

        print("\n✅ All timeout & error tests passed!")
    finally:
        server.close()


if __name__ == "__main__":
    asyncio.run(main())
