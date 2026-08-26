"""Conformance matrix: pin the client surface all three implementations share.

Every difference asserted here is deliberate and documented (README
"Proxy and Close Semantics", docs/API.md). A red test here means a client
drifted from the documented contract — exactly the failure mode that let
close_timeout, sync kwargs, and IPv6 URIs drift silently.
"""

import asyncio
import sys
import threading

import pytest
import websockets

import websocket_rs
import websocket_rs.sync.client

sys.stdout.reconfigure(encoding="utf-8")

sync_connect = websocket_rs.sync.client.connect
native_connect = websocket_rs.connect


def connect_native(uri, **kwargs):
    """native connect() is an async function: run it to completion."""
    async def _go():
        return await native_connect(uri, **kwargs)

    return asyncio.run(_go())


async def _echo(websocket):
    async for message in websocket:
        await websocket.send(message)


def _run_servers(stop: threading.Event, ready: threading.Event, errors: list):
    async def main():
        servers = []
        try:
            servers.append(await websockets.serve(_echo, "127.0.0.1", 8766))
            # This box binds :: v6-only, so [::1] needs its own listener.
            servers.append(await websockets.serve(_echo, "::1", 8767))
            servers.append(
                await websockets.serve(_echo, "127.0.0.1", 8768, subprotocols=["chat", "binary"])
            )
            ready.set()
            while not stop.is_set():
                await asyncio.sleep(0.05)
        except Exception as exc:
            errors.append(exc)
            ready.set()
        finally:
            for s in servers:
                s.close()

    asyncio.run(main())


@pytest.fixture(scope="module")
def echo_server():
    ready, stop, errors = threading.Event(), threading.Event(), []
    thread = threading.Thread(target=_run_servers, args=(stop, ready, errors), daemon=True)
    thread.start()
    assert ready.wait(timeout=5), "parity echo servers did not start"
    if errors:
        raise errors[0]
    yield
    stop.set()
    thread.join(timeout=2)


def test_client_surface_members_parity_all_clients(echo_server):
    """send/recv/ping/pong/close + introspection exist on native and sync alike."""
    native_members = [
        "send", "recv", "ping", "pong", "close",
        "subprotocol", "close_code", "close_reason", "closed",
        "local_address", "remote_address", "is_open",
    ]
    ws = connect_native("ws://127.0.0.1:8766")
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
    with sync_connect("ws://127.0.0.1:8766") as sws:
        for name in sync_members:
            assert hasattr(sws, name), f"sync client missing {name}"


def test_sync_connect_unknown_kwarg_raises_typeerror():
    """The old **kwargs sink is gone; unknown options fail loudly."""
    with pytest.raises(TypeError):
        sync_connect("ws://127.0.0.1:9", headers=[("X", "y")])
    with pytest.raises(TypeError):
        sync_connect("ws://127.0.0.1:9", proxy="socks5://127.0.0.1:9")
    with pytest.raises(TypeError):
        sync_connect("ws://127.0.0.1:9", compression=True)


def test_sync_connect_eager_dial_returns_connected_client(echo_server):
    """connect() dials immediately — no `with` required."""
    ws = sync_connect("ws://127.0.0.1:8766")
    try:
        assert ws.open
        ws.send("ping")
        assert ws.recv() == "ping"
    finally:
        ws.close()


def test_sync_enter_idempotent_no_redial(echo_server):
    """Re-entering `with` keeps the same socket (local port unchanged)."""
    conn = sync_connect("ws://127.0.0.1:8766")
    try:
        first = conn.local_address
        with conn as entered:
            assert entered is conn
            assert conn.local_address == first
    finally:
        conn.close()


def test_native_ipv6_loopback_uri_connects(echo_server):
    """ws://[::1] must resolve, dial, and complete the handshake."""
    ws = connect_native("ws://[::1]:8767")
    try:
        assert ws.is_open
        peer = ws.remote_address
        assert peer is not None and peer[0] == "::1"
    finally:
        ws.close()


def test_addresses_visible_while_open_none_after_close(echo_server):
    """local/remote addresses read off the transport; None once torn down."""
    ws = connect_native("ws://127.0.0.1:8766")
    local, remote = ws.local_address, ws.remote_address
    assert local is not None and remote is not None
    ws.close()
    assert ws.closed
    assert ws.local_address is None
    assert ws.remote_address is None


def test_subprotocol_negotiated_parity_all_clients(echo_server):
    """Offered protocols land in .subprotocol on both live clients."""
    ws = connect_native("ws://127.0.0.1:8768", subprotocols=["chat"])
    try:
        assert ws.subprotocol == "chat"
    finally:
        ws.close()

    with sync_connect("ws://127.0.0.1:8768", subprotocols=["chat"]) as sws:
        assert sws.subprotocol == "chat"


def test_close_timeout_is_sync_only_documented_difference(echo_server):
    """native rejects close_timeout (fire-and-forget close); sync accepts it."""
    ws = sync_connect("ws://127.0.0.1:8766", close_timeout=5.0)
    ws.close()

    async def _rejects():
        native_connect("ws://127.0.0.1:8766", close_timeout=5.0)

    with pytest.raises(TypeError):
        asyncio.run(_rejects())
