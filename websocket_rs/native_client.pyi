"""Native asyncio.Protocol-based WebSocket client (Rust pyclass).

Runs on the asyncio event loop thread — no tokio runtime, no cross-thread
wakeup. Frame codec in Rust with AVX2-vectorised masking. Targets parity with
or better than picows while remaining a pure Python-facing API.

All API-parity items vs the legacy ``async_client`` are implemented; the
legacy module is deprecated and will be removed in 2.0.
"""

from __future__ import annotations

import ssl as _ssl
from collections.abc import AsyncIterator
from types import TracebackType
from typing import Self


class WSMessage:
    """Zero-copy view over a received WebSocket frame payload.

    Implements the Python buffer protocol so ``memoryview(msg)``,
    ``struct.unpack_from(...)``, and ``msg[:n]`` slicing are cheap.
    ``bytes(msg)`` is the only operation that forces a copy.
    """

    def __len__(self) -> int: ...
    def __bytes__(self) -> bytes: ...
    def __getitem__(self, key: int | slice) -> int | bytes: ...
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __repr__(self) -> str: ...


class NativeClient:
    """WebSocket client integrated directly with asyncio.Protocol.

    Instances are returned by ``native_client.connect()``. Direct construction
    via ``NativeClient()`` is not supported.
    """

    def send(self, message: str | bytes) -> None:
        """Fire-and-forget send. Synchronous; encodes a frame straight into a
        ``PyBytes`` buffer and hands it to ``transport.write``. Raises
        ``RuntimeError`` if the connection is closed."""
        ...

    async def recv(self) -> WSMessage:
        """Wait for the next server message and return it as a zero-copy
        :class:`WSMessage`."""
        ...

    def ping(self, data: bytes | None = None) -> None:
        """Send a ping (opcode 0x9) control frame. Payload must be ≤125 bytes."""
        ...

    def close(self) -> None:
        """Send a close frame (best-effort) and close the underlying transport."""
        ...

    # Async iteration: ``async for msg in ws:``
    def __aiter__(self) -> AsyncIterator[WSMessage]: ...
    async def __anext__(self) -> WSMessage: ...

    # Async context manager: ``async with await connect(...) as ws:``
    async def __aenter__(self) -> Self: ...
    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None: ...

    @property
    def is_open(self) -> bool: ...
    @property
    def subprotocol(self) -> str | None: ...
    @property
    def close_code(self) -> int | None: ...
    @property
    def close_reason(self) -> str | None: ...


async def connect(
    uri: str,
    *,
    headers: list[tuple[str, str]] | None = None,
    subprotocols: list[str] | None = None,
    ssl_context: _ssl.SSLContext | None = None,
    connect_timeout: float | None = None,
    receive_timeout: float | None = None,
    proxy: str | None = None,
    compression: bool = False,
) -> NativeClient:
    """Connect to ``uri`` (``ws://`` or ``wss://``) and complete the handshake.

    - ``headers`` adds arbitrary request headers (reserved names — Host,
      Upgrade, Connection, Sec-WebSocket-* — are filtered automatically).
    - ``subprotocols`` sets ``Sec-WebSocket-Protocol``; the negotiated value
      is then on :attr:`NativeClient.subprotocol`.
    - ``ssl_context`` overrides the default ``ssl.create_default_context()``
      used for ``wss://``. TLS is driven by asyncio so the protocol sees
      decrypted bytes.
    - ``connect_timeout`` wraps the full TCP+TLS+handshake sequence in
      ``asyncio.wait_for``; raises ``TimeoutError`` on expiry.
    - ``receive_timeout`` wraps every ``recv()`` / ``async for`` step in
      ``asyncio.wait_for``; the backlog fast-path is not wrapped so
      already-queued messages return immediately.
    - ``proxy`` accepts ``socks5://[user:password@]host:port``. Handshake
      runs in ``loop.run_in_executor`` so the event loop stays responsive;
      once the tunnel is up all traffic goes through the native
      zero-copy hot path. ``picows`` does not support proxies.
    - ``compression=True`` negotiates the ``permessage-deflate`` extension
      (RFC 7692) with ``server_no_context_takeover`` +
      ``client_no_context_takeover`` for bounded per-message memory. If the
      server doesn't echo the extension header the client silently falls
      back to sending uncompressed frames. ``picows`` exposes the RSV1 bit
      but does not compress or decompress for you.
    """
    ...
