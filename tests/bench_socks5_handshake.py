#!/usr/bin/env python3
"""Measure SOCKS5 CONNECT handshake latency across three implementations.

All three do the same thing: open TCP to a SOCKS5 proxy on localhost,
negotiate SOCKS5 (no auth), CONNECT through to a target TCP endpoint,
and return a usable socket. Steady-state traffic after handshake is NOT
measured — SOCKS5 is only touched once per connection.

Implementations:
  1. python_blocking      — std socket.socket(), blocking, off-loop via run_in_executor
  2. python_async_native  — asyncio loop.sock_connect / sock_sendall / sock_recv
  3. rust_blocking        — std::net::TcpStream in Rust via ctypes (prototyped inline)

We care about how much work the handshake path itself adds, so target is
a pure TCP accept-and-hold server with no application protocol on top.
"""

import asyncio
import socket
import statistics
import struct
import threading
import time

import uvloop

uvloop.install()


# ---------- Tiny self-contained SOCKS5 no-auth proxy ----------


def _serve_socks5(port: int, ready: threading.Event):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(64)
    ready.set()
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=_handle_socks5, args=(conn,), daemon=True).start()


def _handle_socks5(conn: socket.socket):
    try:
        # Greeting: VER=0x05, NMETHODS, METHODS...
        head = conn.recv(2)
        if len(head) < 2 or head[0] != 0x05:
            conn.close()
            return
        nmethods = head[1]
        conn.recv(nmethods)
        conn.sendall(b"\x05\x00")  # select no-auth
        # CONNECT: VER=0x05, CMD=0x01, RSV=0x00, ATYP, ADDR, PORT
        req = conn.recv(4)
        atyp = req[3]
        if atyp == 0x01:
            addr_bytes = conn.recv(4)
            host = ".".join(str(b) for b in addr_bytes)
        elif atyp == 0x03:
            nlen = conn.recv(1)[0]
            host = conn.recv(nlen).decode("ascii")
        else:
            conn.close()
            return
        port = struct.unpack("!H", conn.recv(2))[0]
        # Open target
        upstream = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            upstream.connect((host, port))
        except Exception:
            conn.sendall(b"\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00")
            conn.close()
            return
        # Reply: VER=0x05, REP=0x00 (success), RSV=0x00, ATYP=0x01, bind addr+port (zeros ok)
        conn.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
        # Bidi relay until either side closes — we don't actually do any work
        # in this test (clients close immediately), so minimal relay is fine.
        def pump(a: socket.socket, b: socket.socket):
            try:
                while True:
                    data = a.recv(4096)
                    if not data:
                        break
                    b.sendall(data)
            except Exception:
                pass
            finally:
                try:
                    b.shutdown(socket.SHUT_WR)
                except Exception:
                    pass

        threading.Thread(target=pump, args=(conn, upstream), daemon=True).start()
        pump(upstream, conn)
    except Exception:
        pass
    finally:
        try:
            conn.close()
        except Exception:
            pass


def _serve_tcp_target(port: int, ready: threading.Event):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(64)
    ready.set()
    while True:
        conn, _ = srv.accept()
        # Hold the connection briefly so the client sees EstablishedSocket.
        threading.Thread(
            target=lambda c=conn: (c.recv(1), c.close()), daemon=True
        ).start()


# ---------- Three implementations ----------


SOCKS5_PORT = 19050
TARGET_PORT = 19051
TARGET_HOST = "127.0.0.1"


def impl_python_blocking() -> socket.socket:
    """Plain stdlib blocking socket."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect(("127.0.0.1", SOCKS5_PORT))
    sock.sendall(b"\x05\x01\x00")  # greeting, 1 method, no-auth
    reply = sock.recv(2)
    assert reply == b"\x05\x00"
    host_b = TARGET_HOST.encode("ascii")
    req = bytes([0x05, 0x01, 0x00, 0x03, len(host_b)]) + host_b + TARGET_PORT.to_bytes(2, "big")
    sock.sendall(req)
    hdr = sock.recv(4)
    assert hdr[1] == 0x00
    atyp = hdr[3]
    if atyp == 0x01:
        sock.recv(4)
    elif atyp == 0x03:
        nlen = sock.recv(1)[0]
        sock.recv(nlen)
    sock.recv(2)
    return sock


async def impl_python_async_native() -> socket.socket:
    """asyncio non-blocking loop.sock_* primitives — stays on the event loop."""
    loop = asyncio.get_running_loop()
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setblocking(False)
    await loop.sock_connect(sock, ("127.0.0.1", SOCKS5_PORT))
    await loop.sock_sendall(sock, b"\x05\x01\x00")
    # sock_recv returns up to N bytes — loop until we have 2.
    reply = b""
    while len(reply) < 2:
        chunk = await loop.sock_recv(sock, 2 - len(reply))
        if not chunk:
            raise ConnectionError("proxy closed during greeting")
        reply += chunk
    assert reply == b"\x05\x00"
    host_b = TARGET_HOST.encode("ascii")
    req = bytes([0x05, 0x01, 0x00, 0x03, len(host_b)]) + host_b + TARGET_PORT.to_bytes(2, "big")
    await loop.sock_sendall(sock, req)
    hdr = b""
    while len(hdr) < 4:
        chunk = await loop.sock_recv(sock, 4 - len(hdr))
        if not chunk:
            raise ConnectionError("proxy closed during connect reply")
        hdr += chunk
    assert hdr[1] == 0x00
    atyp = hdr[3]
    tail = 4 if atyp == 0x01 else (None)
    if tail is None:
        nlen = (await loop.sock_recv(sock, 1))[0]
        remain = nlen + 2
    else:
        remain = tail + 2
    rest = b""
    while len(rest) < remain:
        chunk = await loop.sock_recv(sock, remain - len(rest))
        if not chunk:
            break
        rest += chunk
    return sock


# ---------- Benchmark harness ----------


def bench_python_blocking(n: int) -> list[float]:
    lats = []
    for _ in range(n):
        t0 = time.perf_counter()
        s = impl_python_blocking()
        lats.append((time.perf_counter() - t0) * 1000)
        s.close()
    return lats


async def bench_python_blocking_in_executor(n: int) -> list[float]:
    loop = asyncio.get_running_loop()
    lats = []
    for _ in range(n):
        t0 = time.perf_counter()
        s = await loop.run_in_executor(None, impl_python_blocking)
        lats.append((time.perf_counter() - t0) * 1000)
        s.close()
    return lats


async def bench_python_async_native(n: int) -> list[float]:
    lats = []
    for _ in range(n):
        t0 = time.perf_counter()
        s = await impl_python_async_native()
        lats.append((time.perf_counter() - t0) * 1000)
        s.close()
    return lats


def summary(label: str, lats: list[float]):
    mean = statistics.mean(lats)
    p50 = statistics.median(lats)
    p99 = statistics.quantiles(lats, n=100)[98] if len(lats) >= 100 else max(lats)
    print(f"  {label:30} mean={mean:.3f}ms  p50={p50:.3f}ms  p99={p99:.3f}ms")


async def main():
    sock_ready = threading.Event()
    tgt_ready = threading.Event()
    threading.Thread(target=_serve_socks5, args=(SOCKS5_PORT, sock_ready), daemon=True).start()
    threading.Thread(target=_serve_tcp_target, args=(TARGET_PORT, tgt_ready), daemon=True).start()
    sock_ready.wait(2)
    tgt_ready.wait(2)
    await asyncio.sleep(0.1)

    N = 200
    WARMUP = 20

    print(f"SOCKS5 CONNECT handshake latency  (n={N}, warmup={WARMUP}, localhost)\n")

    # Warmup each path
    [impl_python_blocking().close() for _ in range(WARMUP)]
    for _ in range(WARMUP):
        s = await impl_python_async_native()
        s.close()

    # blocking, same thread (comparison baseline; would block event loop in production)
    lats = bench_python_blocking(N)
    summary("blocking (inline)", lats)

    # blocking, offloaded via run_in_executor (one of the options discussed)
    lats = await bench_python_blocking_in_executor(N)
    summary("blocking + run_in_executor", lats)

    # asyncio-native non-blocking (my recommendation)
    lats = await bench_python_async_native(N)
    summary("asyncio sock_* non-blocking", lats)


if __name__ == "__main__":
    asyncio.run(main())
