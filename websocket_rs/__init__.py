"""WebSocket-RS: high-performance WebSocket client for Python.

Official async entry point:

    from websocket_rs import connect

    async def main():
        async with await connect("wss://...") as ws:
            ws.send(b"hello")           # sync, fire-and-forget
            msg = await ws.recv()       # returns WSMessage (zero-copy view)
            async for msg in ws:
                ...

This thin re-export maps to :mod:`websocket_rs.native_client` which runs
directly on the asyncio event loop (no tokio bridge, no cross-thread
wakeup) and uses a Rust frame codec with AVX2-vectorised masking and
zero-copy recv via a buffer-protocol ``WSMessage``.

The legacy :mod:`websocket_rs.async_client` remains available for
drop-in compatibility with older code but emits DeprecationWarning; it
will be removed in 2.0.
"""

import sys

from . import websocket_rs as _websocket_rs

# Register submodules in sys.modules for proper import support.
sys.modules["websocket_rs.sync"] = _websocket_rs.sync
sys.modules["websocket_rs.sync.client"] = _websocket_rs.sync.client
sys.modules["websocket_rs.async_client"] = _websocket_rs.async_client
sys.modules["websocket_rs.native_client"] = _websocket_rs.native_client

# Submodules for callers that want explicit addressing.
sync = _websocket_rs.sync
async_client = _websocket_rs.async_client
native_client = _websocket_rs.native_client

# Canonical API: async connect returns a NativeClient.
connect = native_client.connect

# Canonical message type (zero-copy buffer-protocol view).
WSMessage = native_client.WSMessage

__version__ = "0.6.0"

__all__ = [
    "connect",
    "WSMessage",
    "native_client",
    "sync",
    "async_client",
]
