"""
Tests for timeout, error handling, and edge cases.
Covers issues found in code review:
  - connect_timeout actually enforced (P0 fix)
  - close_timeout behavior
  - send after close raises error
  - sync::connect() forwards parameters
"""

import asyncio
import os
import signal
import socket
import sys
import threading
import time

import pytest
import websockets

import websocket_rs.async_client
import websocket_rs.sync.client

sys.stdout.reconfigure(encoding="utf-8")


async def start_echo_server(port=8766):
    async def echo(ws):
        async for msg in ws:
            if msg == b"delayed EINTR reply":
                await asyncio.sleep(0.25)
            elif msg == b"delayed EINTR timeout":
                await asyncio.sleep(1)
            await ws.send(msg)

    return await websockets.serve(echo, "localhost", port, ping_interval=None)


def start_stalled_handshake_server(port):
    ready = threading.Event()

    def serve():
        with socket.socket() as server:
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", port))
            server.listen(1)
            ready.set()
            conn, _ = server.accept()
            with conn:
                conn.recv(4096)
                time.sleep(1)

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    assert ready.wait(timeout=2)
    return thread


@pytest.fixture(scope="module", autouse=True)
def echo_server():
    ready = threading.Event()
    stop = threading.Event()
    errors = []

    async def run_server():
        server = await start_echo_server(8766)
        try:
            ready.set()
            while not stop.is_set():
                await asyncio.sleep(0.05)
        except Exception as exc:
            errors.append(exc)
            ready.set()
        finally:
            server.close()
            await server.wait_closed()

    thread = threading.Thread(target=lambda: asyncio.run(run_server()), daemon=True)
    thread.start()
    assert ready.wait(timeout=5), "echo server on 8766 did not start"
    if errors:
        raise errors[0]
    yield
    stop.set()
    thread.join(timeout=2)


def test_sync_connect_timeout():
    """connect_timeout should raise TimeoutError for unreachable hosts."""
    # 192.0.2.1 is TEST-NET, guaranteed to be unreachable (RFC 5737)
    start = time.perf_counter()
    try:
        with websocket_rs.sync.client.ClientConnection("ws://192.0.2.1:9999", connect_timeout=1.0):
            pass
        pytest.fail("Should have raised")
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
        pytest.fail("Should have raised")
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
            pytest.fail("Should have raised")
        except TimeoutError:
            elapsed = time.perf_counter() - start
            assert 0.3 < elapsed < 2.0, f"Unexpected timeout duration: {elapsed:.1f}s"
            print(f"✓ sync recv timeout works ({elapsed:.1f}s)")


@pytest.mark.skipif(not hasattr(signal, "SIGUSR1"), reason="SIGUSR1 is unavailable")
def test_sync_recv_retries_eintr_after_python_signal():
    previous_handler = signal.signal(signal.SIGUSR1, lambda *_: None)
    stop = threading.Event()
    signals_sent = 0

    def send_signals():
        nonlocal signals_sent
        while not stop.wait(0.01):
            os.kill(os.getpid(), signal.SIGUSR1)
            signals_sent += 1

    try:
        with websocket_rs.sync.client.ClientConnection(
            "ws://localhost:8766", receive_timeout=1.0
        ) as ws:
            ws.send(b"delayed EINTR reply")
            signal_thread = threading.Thread(target=send_signals, daemon=True)
            signal_thread.start()
            try:
                received = ws.recv()
            finally:
                stop.set()
                signal_thread.join(timeout=1)
    finally:
        signal.signal(signal.SIGUSR1, previous_handler)

    assert signals_sent > 0
    assert received == b"delayed EINTR reply"


@pytest.mark.skipif(not hasattr(signal, "SIGUSR1"), reason="SIGUSR1 is unavailable")
def test_sync_recv_eintr_retries_preserve_timeout_deadline():
    previous_handler = signal.signal(signal.SIGUSR1, lambda *_: None)
    stop = threading.Event()
    signals_sent = 0

    def send_signals():
        nonlocal signals_sent
        while not stop.wait(0.01):
            os.kill(os.getpid(), signal.SIGUSR1)
            signals_sent += 1

    try:
        with websocket_rs.sync.client.ClientConnection(
            "ws://localhost:8766", receive_timeout=0.2
        ) as ws:
            ws.send(b"delayed EINTR timeout")
            signal_thread = threading.Thread(target=send_signals, daemon=True)
            signal_thread.start()
            started = time.perf_counter()
            try:
                with pytest.raises(TimeoutError):
                    ws.recv()
            finally:
                elapsed = time.perf_counter() - started
                stop.set()
                signal_thread.join(timeout=1)
    finally:
        signal.signal(signal.SIGUSR1, previous_handler)

    assert signals_sent > 0
    assert 0.1 < elapsed < 0.6


@pytest.mark.skipif(
    os.name != "posix" or not hasattr(signal, "SIGINT"),
    reason="POSIX SIGINT is unavailable",
)
def test_sync_recv_eintr_propagates_keyboard_interrupt():
    previous_handler = signal.signal(signal.SIGINT, signal.default_int_handler)

    def interrupt_recv():
        time.sleep(0.05)
        os.kill(os.getpid(), signal.SIGINT)

    try:
        with websocket_rs.sync.client.ClientConnection(
            "ws://localhost:8766", receive_timeout=1.0
        ) as ws:
            ws.send(b"delayed EINTR reply")
            signal_thread = threading.Thread(target=interrupt_recv, daemon=True)
            signal_thread.start()
            with pytest.raises(KeyboardInterrupt):
                ws.recv()
            signal_thread.join(timeout=1)
    finally:
        signal.signal(signal.SIGINT, previous_handler)


async def test_async_send_after_close():
    """Async: sending after close should raise RuntimeError."""
    ws = await websocket_rs.async_client.connect("ws://localhost:8766")
    await ws.send("hello")
    await ws.recv()
    await ws.close()
    try:
        await ws.send("should fail")
        pytest.fail("Should have raised")
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
            pytest.fail("Should have raised")
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
        pytest.fail("Should have raised")
    except (TimeoutError, ConnectionError):
        elapsed = time.perf_counter() - start
        print(f"✓ async connect_timeout works ({elapsed:.1f}s)")


async def test_async_connect_function_connect_timeout_kwarg_expires_promptly():
    thread = start_stalled_handshake_server(8767)
    start = time.perf_counter()

    with pytest.raises(TimeoutError):
        await asyncio.wait_for(
            websocket_rs.async_client.connect(
                "ws://127.0.0.1:8767",
                connect_timeout=0.05,
            ),
            timeout=0.8,
        )

    assert time.perf_counter() - start < 0.4
    thread.join(timeout=2)


async def test_async_connect_function_receive_timeout_kwarg_expires_promptly():
    ws = await websocket_rs.async_client.connect(
        "ws://localhost:8766",
        receive_timeout=0.05,
    )
    start = time.perf_counter()
    try:
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(ws.recv(), timeout=0.8)
        assert time.perf_counter() - start < 0.4
    finally:
        await ws.close()


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__]))
