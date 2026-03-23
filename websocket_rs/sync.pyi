from typing import Any

class Response:
    def send(self, message: str) -> None:
        """
        Send payload to the connected WebSocket server.
        """
        ...
    def recv(self) -> str:
        """
        Waits for the next message from the WebSocket server and returns it as a string.
        """
        ...
    def close(self) -> None:
        """
        Close WebSocket connection.
        """
        ...

def connect(
    url: str,
    proxy: str,
    headers: dict[str, Any],
    connect_timeout: int | float = 30,
    receive_timeout: int | float = 30,
) -> Response:
    """
    Establishes an asynchronous WebSocket connection.

    Args:
        url: server URL (e.g., `"ws://localhost:8765"`).
        proxy: The proxy URL (e.g., `"socks5://127.0.0.1:9050"`).
        headers: Custom HTTP headers. (e.g., `{"Authorization": "Bearer token123"}`).
        connect_timeout: Connection timeout in seconds. Default: 30.0.
        receive_timeout: Receive timeout in seconds. Default: 30.0.
    """
    ...
