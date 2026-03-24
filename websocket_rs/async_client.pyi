from types import TracebackType

class ClientConnection:
    """Async WebSocket client connection backed by tokio-tungstenite."""

    def __init__(
        self,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        proxy: str | None = None,
        connect_timeout: float | None = None,
        receive_timeout: float | None = None,
    ) -> None: ...
    async def send(self, message: str | bytes) -> None: ...
    async def recv(self) -> str | bytes: ...
    async def close(self) -> None: ...
    async def ping(self, data: bytes | None = None) -> None: ...
    async def pong(self, data: bytes | None = None) -> None: ...

    @property
    def open(self) -> bool: ...
    @property
    def closed(self) -> bool: ...
    @property
    def local_address(self) -> tuple[str, int] | None: ...
    @property
    def remote_address(self) -> tuple[str, int] | None: ...
    @property
    def close_code(self) -> int | None: ...
    @property
    def close_reason(self) -> str | None: ...
    @property
    def subprotocol(self) -> str | None: ...

    async def __aenter__(self) -> ClientConnection: ...
    async def __aexit__(
        self,
        exc_type: type[BaseException] | None = None,
        exc_value: BaseException | None = None,
        traceback: TracebackType | None = None,
    ) -> bool: ...
    def __aiter__(self) -> ClientConnection: ...
    async def __anext__(self) -> str | bytes: ...

async def connect(
    uri: str,
    *,
    headers: dict[str, str] | None = None,
    proxy: str | None = None,
    **kwargs: object,
) -> ClientConnection:
    """Establish an async WebSocket connection and return a connected client.

    Args:
        uri: WebSocket server URL (e.g., ``"ws://localhost:8765"``).
        headers: Custom HTTP headers (e.g., ``{"Authorization": "Bearer token"}``).
        proxy: SOCKS5 proxy URL (e.g., ``"socks5://127.0.0.1:9050"``).
        **kwargs: Accepted for compatibility, currently ignored.
    """
    ...
