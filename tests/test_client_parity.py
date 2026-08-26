"""Conformance matrix: pin the client surface all three implementations share.

Every difference asserted here is deliberate and documented (README
"Proxy and Close Semantics", docs/API.md). A red test here means a client
drifted from the documented contract — exactly the failure mode that let
close_timeout, sync kwargs, and IPv6 URIs drift silently.
"""

import asyncio
import sys
import threading
from types import SimpleNamespace

import pytest
import websockets

import websocket_rs
import websocket_rs.sync.client

sync_connect = websocket_rs.sync.client.connect
native_connect = websocket_rs.connect


async def _echo(websocket):
    async for message in websocket:
        await websocket.send(message)


def _run_servers(stop, on_ready, errors):
    async def main():
        servers = []
        try:
            # Port 0 everywhere: no fixed ports means no cross-module coupling.
            v4 = await websockets.serve(_echo, "127.0.0.1", 0)
            # This box binds :: v6-only, so [::1] needs its own listener.
            v6 = await websockets.serve(_echo, "::1", 0)
            sub = await websockets.serve(
                _echo, "127.0.0.1", 0, subprotocols=["chat", "binary"]
            )
            servers = [v4, v6, sub]
            on_ready(SimpleNamespace(
                v4=v4.sockets[0].getsockname()[1],
                v6=v6.sockets[0].getsockname()[1],
                sub=sub.sockets[0].getsockname()[1],
            ))
            while not stop.is_set():
                await asyncio.sleep(0.05)
        except Exception as exc:
            errors.append(exc)
            on_ready(None)
        finally:
            for s in servers:
                s.close()
                await s.wait_closed()

    asyncio.run(main())


@pytest.fixture(scope="module")
def echo_server():
    sys.stdout.reconfigure(encoding="utf-8")
    ready, stop, errors = threading.Event(), threading.Event(), []
    ports_box = {}

    def on_ready(ports):
        ports_box["ports"] = ports
        ready.set()

    thread = threading.Thread(
        target=_run_servers, args=(stop, on_ready, errors), daemon=True
    )
    thread.start()
    assert ready.wait(timeout=5), "parity echo servers did not start"
    if errors:
        raise errors[0]
    yield ports_box["ports"]
    stop.set()
    thread.join(timeout=3)
    assert not thread.is_alive(), "parity echo servers did not shut down"


# --- Surface parity -------------------------------------------------------


@pytest.mark.asyncio
async def test_client_surface_members_parity_all_clients(echo_server):
    """send/recv/ping/pong/close + introspection exist on native and sync alike."""
    native_members = [
        "send", "recv", "ping", "pong", "close",
        "subprotocol", "close_code", "close_reason", "closed",
        "local_address", "remote_address", "is_open",
    ]
    ws = await native_connect(f"ws://127.0.0.1:{echo_server.v4}")
    try:
        for name in native_members:
            assert hasattr(ws, name), f"native client missing {name}"
        assert isinstance(ws.closed, bool)
        assert isinstance(ws.is_open, bool)
    finally:
        ws.close()

    sync_members = [
        "send", "recv", "ping", "pong", "close",
        "subprotocol", "close_code", "close_reason", "closed", "open",
        "local_address", "remote_address",
    ]
    with sync_connect(f"ws://127.0.0.1:{echo_server.v4}") as sws:
        for name in sync_members:
            assert hasattr(sws, name), f"sync client missing {name}"


@pytest.mark.asyncio
async def test_native_data_path_roundtrip_in_live_loop(echo_server):
    """Full native lifecycle driven from a real event loop, not asyncio.run."""
    uri = f"ws://127.0.0.1:{echo_server.v4}"
    ws = await native_connect(uri)
    try:
        assert ws.is_open
        ws.send("hello")       # fire-and-forget
        msg = await asyncio.wait_for(ws.recv(), 5)
        assert bytes(msg) == b"hello"
        ws.pong(b"keepalive")   # fire-and-forget, like ping
        ws.ping()
    finally:
        ws.close()
    assert ws.closed


# --- Sync dial semantics ---------------------------------------------------


def test_sync_connect_unknown_kwarg_raises_typeerror():
    """The old **kwargs sink is gone; unknown options fail loudly."""
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        sync_connect("ws://127.0.0.1:9", headers=[("X", "y")])
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        sync_connect("ws://127.0.0.1:9", proxy="socks5://127.0.0.1:9")
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        sync_connect("ws://127.0.0.1:9", compression=True)


def test_sync_connect_eager_dial_returns_connected_client(echo_server):
    """connect() dials immediately — no `with` required."""
    ws = sync_connect(f"ws://127.0.0.1:{echo_server.v4}")
    try:
        assert ws.open
        ws.send("ping")
        assert ws.recv() == "ping"
    finally:
        ws.close()


def test_sync_enter_idempotent_no_redial(echo_server):
    """Re-entering `with` keeps the same socket (local port unchanged)."""
    conn = sync_connect(f"ws://127.0.0.1:{echo_server.v4}")
    try:
        first = conn.local_address
        with conn as entered:
            assert entered is conn
            assert conn.local_address == first
    finally:
        conn.close()


# --- IPv6 literals ---------------------------------------------------------


@pytest.mark.asyncio
async def test_native_ipv6_loopback_uri_connects(echo_server):
    """ws://[::1] must resolve, dial, and complete the handshake."""
    ws = await native_connect(f"ws://[::1]:{echo_server.v6}")
    try:
        assert ws.is_open
        peer = ws.remote_address
        assert peer is not None and peer[0] == "::1"
    finally:
        ws.close()


def test_sync_ipv6_loopback_uri_connects(echo_server):
    """Regression: http keeps brackets in host(); getaddrinfo needs them gone."""
    with sync_connect(f"ws://[::1]:{echo_server.v6}") as sws:
        assert sws.open
        assert sws.remote_address[0] == "::1"





# --- Addresses -------------------------------------------------------------


@pytest.mark.asyncio
async def test_addresses_visible_while_open_none_after_close(echo_server):
    """local/remote addresses read off the transport; None once torn down."""
    ws = await native_connect(f"ws://127.0.0.1:{echo_server.v4}")
    local, remote = ws.local_address, ws.remote_address
    assert local is not None and remote is not None
    ws.close()
    assert ws.closed
    assert ws.local_address is None
    assert ws.remote_address is None


# --- Subprotocols ----------------------------------------------------------


@pytest.mark.asyncio
async def test_subprotocol_negotiated_parity_all_clients(echo_server):
    """Offered protocols land in .subprotocol on both live clients."""
    ws = await native_connect(f"ws://127.0.0.1:{echo_server.sub}", subprotocols=["chat"])
    try:
        assert ws.subprotocol == "chat"
    finally:
        ws.close()

    with sync_connect(f"ws://127.0.0.1:{echo_server.sub}", subprotocols=["chat"]) as sws:
        assert sws.subprotocol == "chat"


# --- Documented asymmetries ------------------------------------------------


def test_close_timeout_is_sync_only_documented_difference(echo_server):
    """native rejects close_timeout (fire-and-forget close); sync accepts it."""
    ws = sync_connect(f"ws://127.0.0.1:{echo_server.v4}", close_timeout=5.0)
    ws.close()

    with pytest.raises(TypeError):
        native_connect(f"ws://127.0.0.1:{echo_server.v4}", close_timeout=5.0)


# --- pong guards -----------------------------------------------------------


@pytest.mark.asyncio
async def test_pong_payload_limit_matches_ping_guard(echo_server):
    """Control frames cap at 125 bytes; the guard fires before any transport."""
    ws = await native_connect(f"ws://127.0.0.1:{echo_server.v4}")
    try:
        with pytest.raises(ValueError, match="125 bytes"):
            ws.pong(b"x" * 126)
    finally:
        ws.close()


def test_sync_pong_payload_limit_matches_native(echo_server):
    """Same control-frame guard on the sync client; no silent RFC violation."""
    ws = sync_connect(f"ws://127.0.0.1:{echo_server.v4}")
    try:
        with pytest.raises(ValueError, match="125 bytes"):
            ws.pong(b"x" * 126)
        with pytest.raises(ValueError, match="125 bytes"):
            ws.ping(b"x" * 126)
    finally:
        ws.close()


@pytest.mark.asyncio
async def test_pong_after_close_raises_runtime_error(echo_server):
    """Closed client -> explicit error, not a silent drop."""
    ws = await native_connect(f"ws://127.0.0.1:{echo_server.v4}")
    ws.close()
    assert ws.closed
    with pytest.raises(RuntimeError, match="[Cc]losed|[Nn]o transport"):
        ws.pong(b"x")
