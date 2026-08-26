from types import TracebackType

class ClientConnection:
    """Sync WebSocket client connection backed by tungstenite."""

    def __init__(
        self,
        url: str,
        connect_timeout: float | None = None,
        receive_timeout: float | None = None,
        close_timeout: float | None = None,
        tcp_nodelay: bool | None = None,
        subprotocols: list[str] | None = None,
    ) -> None: ...
    def send(self, message: str | bytes) -> None: ...
    def recv(self) -> str | bytes: ...
    def close(self) -> None: ...
    def ping(self, data: bytes | None = None) -> None: ...
    def pong(self, data: bytes | None = None) -> None: ...

    @property
    def open(self) -> bool: ...
    @property
    def closed(self) -> bool: ...
    @property
    def local_address(self) -> tuple[str, int] | None: ...
    @property
    def remote_address(self) -> tuple[str, int] | None: ...
    @property
    def subprotocol(self) -> str | None: ...
    @property
    def close_code(self) -> int | None: ...
    @property
    def close_reason(self) -> str | None: ...

    def __enter__(self) -> ClientConnection:
        """Re-enter an open connection; dials again only if it was closed."""
        ...
    def __exit__(
        self,
        exc_type: type[BaseException] | None = None,
        exc_value: BaseException | None = None,
        traceback: TracebackType | None = None,
    ) -> bool: ...
    def __iter__(self) -> ClientConnection: ...
    def __next__(self) -> str | bytes: ...

def connect(
    uri: str,
    connect_timeout: float | None = None,
    receive_timeout: float | None = None,
    close_timeout: float | None = None,
    tcp_nodelay: bool | None = None,
    subprotocols: list[str] | None = None,
) -> ClientConnection:
    """Create a connected sync WebSocket client (dials immediately).

    Args:
        uri: WebSocket server URL (e.g., ``"ws://localhost:8765"``).
        subprotocols: Protocols to offer via Sec-WebSocket-Protocol; the
            negotiated value lands on :attr:`ClientConnection.subprotocol`.
        connect_timeout: Connection timeout in seconds. Default: 10.0.
        receive_timeout: Receive timeout in seconds. Default: 10.0.
        close_timeout: Close handshake timeout in seconds. Default: 10.0.
        tcp_nodelay: Disable Nagle's algorithm. Default: True.

    Raises:
        TypeError: on any other keyword argument — headers, proxy,
            ssl_context, compression and on_message are native-client
            features; the sync client never accepted them silently.
    """
    ...
