"""Native asyncio.Protocol-based WebSocket client (Rust pyclass).

Runs on the asyncio event loop thread — no tokio runtime, no cross-thread
wakeup. Frame codec in Rust with AVX2-vectorised masking. Targets parity with
or better than picows while remaining a pure Python-facing API.

Limitations:
- SOCKS5 proxy not yet supported (TLS wss:// works via asyncio ssl plumbing)
- Binary and Text frames only; no ping/pong, no fragments, no permessage-deflate
- No custom headers / subprotocol / receive_timeout
"""

import ssl as _ssl

class NativeClient:
    """WebSocket client integrated directly with asyncio.Protocol.

    Instances are returned by ``native_client.connect()``. Direct construction
    via ``NativeClient()`` is not supported.
    """

    def send(self, message: str | bytes) -> None:
        """Fire-and-forget send. Runs synchronously on the asyncio loop thread;
        no awaitable is produced. Raises if the connection is closed."""
        ...

    async def recv(self) -> bytes:
        """Wait for the next server message and return its payload as bytes."""
        ...

    def close(self) -> None:
        """Send a close frame (best-effort) and close the underlying transport."""
        ...

    @property
    def is_open(self) -> bool:
        """True iff the handshake completed and the transport is still live."""
        ...

async def connect(
    uri: str,
    *,
    ssl_context: _ssl.SSLContext | None = None,
) -> NativeClient:
    """Connect to ``uri`` (``ws://`` or ``wss://``) and complete the handshake.

    For ``wss://`` URIs a default ``ssl.create_default_context()`` is used
    unless ``ssl_context`` is supplied. TLS is handled by the asyncio
    transport, so the protocol sees decrypted frame bytes.
    """
    ...
