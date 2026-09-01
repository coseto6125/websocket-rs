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
import shutil
import signal
import socket
import subprocess
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


def _serve_delayed_handshake(listener, delay):
    """Complete one WebSocket handshake, but only after `delay` seconds."""
    listener.settimeout(5)  # never block the thread forever on a client that never dials
    conn, _ = listener.accept()
    with conn:
        request = b""
        while b"\r\n\r\n" not in request:
            chunk = conn.recv(4096)
            if not chunk:
                return
            request += chunk
        time.sleep(delay)
        conn.sendall(
            "\r\n".join([
                "HTTP/1.1 101 Switching Protocols",
                "Upgrade: websocket",
                "Connection: Upgrade",
                f"Sec-WebSocket-Accept: {_ws_accept_key(request)}",
                "",
                "",
            ]).encode()
        )
        conn.recv(4096)  # hold the connection open until the client closes


@pytest.mark.skipif(not hasattr(signal, "SIGUSR1"), reason="SIGUSR1 is unavailable")
def test_sync_connect_eintr_during_handshake_does_not_fake_a_timeout():
    """No deadline is begun before recv(), so an interrupted handshake read
    must resume instead of reporting `read deadline elapsed`."""
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]

    server_errors = []

    def serve():
        try:
            _serve_delayed_handshake(listener, delay=0.3)
        except Exception as exc:  # surfaced below, never swallowed
            server_errors.append(exc)

    previous_handler = signal.signal(signal.SIGUSR1, lambda *_: None)
    stop = threading.Event()
    signals_sent = 0

    def send_signals():
        nonlocal signals_sent
        while not stop.wait(0.01):
            os.kill(os.getpid(), signal.SIGUSR1)
            signals_sent += 1

    server_thread = threading.Thread(target=serve, daemon=True)
    server_thread.start()
    signal_thread = threading.Thread(target=send_signals, daemon=True)
    signal_thread.start()
    try:
        with websocket_rs.sync.client.ClientConnection(
            f"ws://127.0.0.1:{port}", connect_timeout=5.0, receive_timeout=1.0
        ) as ws:
            assert ws.remote_address[1] == port
    finally:
        stop.set()
        signal_thread.join(timeout=1)
        server_thread.join(timeout=5)
        listener.close()
        signal.signal(signal.SIGUSR1, previous_handler)

    assert signals_sent > 0
    assert server_errors == [], f"server failed: {server_errors}"


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


# ---- peer-initiated close: the transport calls eof_received() on our protocol ----


def _ws_accept_key(request: bytes) -> str:
    import base64
    from hashlib import sha1

    key = next(
        ln.split(":", 1)[1].strip()
        for ln in request.decode("latin-1").split("\r\n")
        if ln.lower().startswith("sec-websocket-key:")
    )
    return base64.b64encode(sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()


@pytest.fixture(scope="session")
def tls_cert(tmp_path_factory):
    """Self-signed cert in a temp dir. tests/certs/ is gitignored and CI never runs
    `make tls-certs`; writing into the source tree would also race across workers."""
    if shutil.which("openssl") is None:
        pytest.skip("openssl not available")
    certs = tmp_path_factory.mktemp("certs")
    cert, key = certs / "cert.pem", certs / "key.pem"
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-days", "3650", "-nodes",
         "-keyout", str(key), "-out", str(cert), "-subj", "/CN=127.0.0.1",
         "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
         "-addext", "basicConstraints=critical,CA:FALSE",
         "-addext", "extendedKeyUsage=serverAuth"],
        check=True, capture_output=True,
    )
    return cert, key


def _start_closing_ws_server(certs):
    """Handshake, send one frame, then close from the server side.

    Returns (thread, port, errors). Port 0 lets the OS pick a free one, so
    parallel runs and stray local services cannot collide. TLS closes via
    unwrap() so a close_notify actually goes out — a bare close() just drops the
    socket, and only close_notify reaches eof_received.
    """
    import ssl

    errors = []
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port = srv.getsockname()[1]
    srv.listen(1)

    def serve():
        conn = None
        try:
            srv.settimeout(5)
            conn, _ = srv.accept()
            conn.settimeout(5)  # never block the thread forever on a stuck peer
            if certs is not None:
                cert, key = certs
                ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                ctx.load_cert_chain(cert, key)
                conn = ctx.wrap_socket(conn, server_side=True)
            request = b""
            while b"\r\n\r\n" not in request:
                request += conn.recv(4096)
            conn.sendall(
                "\r\n".join([
                    "HTTP/1.1 101 Switching Protocols",
                    "Upgrade: websocket",
                    "Connection: Upgrade",
                    f"Sec-WebSocket-Accept: {_ws_accept_key(request)}",
                    "",
                    "",
                ]).encode()
                + bytes([0x81, 3])
                + b"bye"
            )
        except Exception as error:  # surfaced by the test, never swallowed
            errors.append(error)
        finally:
            if conn is not None:
                if certs is not None:
                    try:
                        conn = conn.unwrap()
                    except OSError:
                        pass
                conn.close()
            srv.close()

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return thread, port, errors


def _event_loops():
    """uvloop where it is installed; it has no Windows wheels."""
    try:
        import uvloop  # noqa: F401
    except ImportError:
        return ["asyncio"]
    return ["asyncio", "uvloop"]


@pytest.mark.parametrize("loop_name", _event_loops())
@pytest.mark.parametrize("tls", [False, True], ids=["ws", "wss"])
def test_peer_close_does_not_raise_in_transport_callback(loop_name, tls, request):
    """The peer closes first, so the transport calls eof_received() on our protocol.

    NativeClient is a pyclass and inherits nothing from asyncio.Protocol, so the
    base class default is not available and the method has to exist on our side.
    Every call site invokes it unguarded except uvloop's plain-TCP _on_eof, so a
    missing method raised inside asyncio's own callback — under TLS that lands in
    _fatal_error and reads as a connection failure.

    The second recv() is what makes this a real check rather than a sleep: it must
    fail promptly because the EOF closed the transport and connection_lost failed
    the pending future. Asserting no exception at all reached the loop (not just
    AttributeError) keeps any other broken callback from passing silently.
    """
    import ssl

    import websocket_rs

    certs = request.getfixturevalue("tls_cert") if tls else None
    thread, port, server_errors = _start_closing_ws_server(certs)
    caught = []

    ssl_ctx = None
    if tls:
        ssl_ctx = ssl.create_default_context()
        ssl_ctx.check_hostname = False
        ssl_ctx.verify_mode = ssl.CERT_NONE

    async def run():
        asyncio.get_running_loop().set_exception_handler(lambda _loop, ctx: caught.append(ctx))
        scheme = "wss" if tls else "ws"
        ws = await websocket_rs.connect(f"{scheme}://127.0.0.1:{port}", ssl_context=ssl_ctx, receive_timeout=5)
        assert bytes(await ws.recv()) == b"bye"
        started = time.perf_counter()
        with pytest.raises((OSError, RuntimeError, ConnectionError, EOFError)):
            # connection_lost must fail it immediately; a TimeoutError here would
            # mean the pending receive hung until the timeout instead.
            await asyncio.wait_for(ws.recv(), timeout=2)
        assert time.perf_counter() - started < 1
        try:
            ws.close()
        except Exception:
            pass

    if loop_name == "uvloop":
        import uvloop

        uvloop.run(run())
    else:
        loop = asyncio.SelectorEventLoop()  # CPython's own, not an installed uvloop
        try:
            loop.run_until_complete(run())
        finally:
            loop.close()

    thread.join(timeout=5)
    assert not thread.is_alive(), "server thread did not finish"
    assert server_errors == [], f"server failed: {server_errors}"
    assert caught == [], f"transport callback failed: {caught}"


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__]))
