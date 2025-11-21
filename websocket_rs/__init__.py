"""
WebSocket-RS: High-performance WebSocket client for Python.

This module provides a drop-in replacement for Python WebSocket libraries
with significantly better performance through Rust implementation.
"""

# Import main WebSocket class from Rust module
try:
    from .websocket_rs import WebSocket
except ImportError:
    # Fallback for development
    from websocket_rs import WebSocket

# Import patching utilities
from . import patch
from . import auto_patch

# Version
__version__ = "0.2.0"

# Public API
__all__ = [
    'WebSocket',
    'patch',
    'auto_patch',
    'enable_monkeypatch',
    'disable_monkeypatch',
]


def enable_monkeypatch(method='auto', include_async=False):
    """
    Enable global WebSocket library patching.

    Args:
        method: Patching method
            - 'auto': Use import hooks (recommended)
            - 'patch': Direct module patching
            - 'both': Use both methods
        include_async: Also patch async implementations (experimental)

    Examples:
        >>> import websocket_rs
        >>> websocket_rs.enable_monkeypatch()
        >>> # Now all WebSocket imports will use websocket-rs

        >>> import websockets.sync.client
        >>> ws = websockets.sync.client.connect("ws://localhost:8765")
        >>> # This will actually use websocket-rs!
    """
    if method in ('auto', 'both'):
        auto_patch.enable_auto_patch()

    if method in ('patch', 'both'):
        patch.patch_all(include_async=include_async)


def disable_monkeypatch():
    """
    Disable global WebSocket library patching.

    This restores the original WebSocket implementations.
    """
    auto_patch.disable_auto_patch()
    if patch.is_patched():
        patch.unpatch_all()


# Convenience aliases
connect = WebSocket  # For drop-in replacement