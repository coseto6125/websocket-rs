"""
Monkeypatch module to automatically replace websockets with websocket-rs.

Usage:
    import websocket_rs.patch  # Just import to auto-patch

    # Or manually control:
    from websocket_rs.patch import patch_all, unpatch_all
    patch_all()
"""

import sys
import warnings
from typing import Optional, Any
from functools import wraps

# Store original modules for unpatch
_original_modules = {}
_patched = False


def patch_websockets_sync():
    """Patch websockets.sync.client module."""
    try:
        # Import our high-performance implementation
        from websocket_rs import WebSocket

        # Create a compatibility wrapper for connect function
        def connect(uri, **kwargs):
            """Drop-in replacement for websockets.sync.client.connect."""
            # Extract relevant parameters
            ws = WebSocket(
                uri,
                connect_timeout=kwargs.get('open_timeout', 30),
                receive_timeout=kwargs.get('close_timeout', 30)
            )
            ws.connect()
            return ws

        # Patch the module
        if 'websockets' in sys.modules:
            import websockets
            if hasattr(websockets, 'sync'):
                _original_modules['websockets.sync.client.connect'] = getattr(
                    websockets.sync.client, 'connect', None
                )
                _original_modules['websockets.sync.client.WebSocket'] = getattr(
                    websockets.sync.client, 'WebSocket', None
                )

                websockets.sync.client.connect = connect
                websockets.sync.client.WebSocket = WebSocket

        # Also patch direct imports
        if 'websockets.sync.client' in sys.modules:
            import websockets.sync.client
            _original_modules['websockets.sync.client'] = websockets.sync.client.connect
            websockets.sync.client.connect = connect
            websockets.sync.client.WebSocket = WebSocket

        return True
    except ImportError:
        return False


def patch_websockets_async():
    """Patch websockets async client (experimental)."""
    try:
        from websocket_rs import WebSocket
        import asyncio

        class AsyncWebSocketWrapper:
            """Async wrapper for synchronous WebSocket."""

            def __init__(self, ws: WebSocket):
                self._ws = ws
                self._loop = asyncio.get_event_loop()

            async def send(self, message):
                """Async send wrapper."""
                return await self._loop.run_in_executor(None, self._ws.send, message)

            async def recv(self):
                """Async receive wrapper."""
                return await self._loop.run_in_executor(None, self._ws.recv)

            async def close(self):
                """Async close wrapper."""
                return await self._loop.run_in_executor(None, self._ws.close)

            async def __aenter__(self):
                return self

            async def __aexit__(self, *args):
                await self.close()
                return False

        async def connect_async(uri, **kwargs):
            """Async connect wrapper."""
            ws = WebSocket(
                uri,
                connect_timeout=kwargs.get('open_timeout', 30),
                receive_timeout=kwargs.get('close_timeout', 30)
            )
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, ws.connect)
            return AsyncWebSocketWrapper(ws)

        # Patch async module
        if 'websockets' in sys.modules:
            import websockets
            _original_modules['websockets.connect'] = websockets.connect
            websockets.connect = connect_async

        if 'websockets.client' in sys.modules:
            import websockets.client
            _original_modules['websockets.client.connect'] = websockets.client.connect
            websockets.client.connect = connect_async

        return True
    except ImportError:
        return False


def patch_websocket_client():
    """Patch websocket-client library."""
    try:
        from websocket_rs import WebSocket as RustWebSocket

        class WebSocketApp:
            """Compatibility wrapper for websocket-client WebSocketApp."""

            def __init__(self, url, on_message=None, on_error=None,
                        on_close=None, on_open=None, **kwargs):
                self.url = url
                self.on_message = on_message
                self.on_error = on_error
                self.on_close = on_close
                self.on_open = on_open
                self.ws = None

            def run_forever(self, **kwargs):
                """Run the WebSocket connection."""
                try:
                    self.ws = RustWebSocket(self.url)
                    self.ws.connect()

                    if self.on_open:
                        self.on_open(self)

                    while self.ws.is_connected:
                        try:
                            message = self.ws.recv()
                            if message and self.on_message:
                                self.on_message(self, message)
                        except Exception as e:
                            if self.on_error:
                                self.on_error(self, e)
                            break

                    if self.on_close:
                        self.on_close(self)

                except Exception as e:
                    if self.on_error:
                        self.on_error(self, e)
                finally:
                    if self.ws:
                        self.ws.close()

            def send(self, data):
                """Send data."""
                if self.ws:
                    self.ws.send(data)

            def close(self):
                """Close connection."""
                if self.ws:
                    self.ws.close()

        # Simple WebSocket class
        class WebSocket:
            """Compatibility wrapper for websocket-client WebSocket."""

            def __init__(self):
                self._ws = None

            def connect(self, url, **kwargs):
                """Connect to WebSocket server."""
                self._ws = RustWebSocket(url)
                self._ws.connect()
                return self._ws

            def create_connection(url, **kwargs):
                """Static method to create connection."""
                ws = RustWebSocket(url)
                ws.connect()
                return ws

        # Patch the module
        if 'websocket' in sys.modules:
            import websocket
            _original_modules['websocket.WebSocketApp'] = getattr(websocket, 'WebSocketApp', None)
            _original_modules['websocket.WebSocket'] = getattr(websocket, 'WebSocket', None)
            _original_modules['websocket.create_connection'] = getattr(websocket, 'create_connection', None)

            websocket.WebSocketApp = WebSocketApp
            websocket.WebSocket = WebSocket
            websocket.create_connection = WebSocket.create_connection

        return True
    except ImportError:
        return False


def patch_all(include_async=False, force=False):
    """
    Patch all WebSocket libraries to use websocket-rs.

    Args:
        include_async: Also patch async implementations (experimental)
        force: Force patching even if already patched

    Returns:
        dict: Results of patching each library
    """
    global _patched

    if _patched and not force:
        warnings.warn("Already patched. Use force=True to re-patch.")
        return {}

    results = {
        'websockets_sync': patch_websockets_sync(),
        'websocket_client': patch_websocket_client(),
    }

    if include_async:
        results['websockets_async'] = patch_websockets_async()

    _patched = True

    if patched_libs := [lib for lib, success in results.items() if success]:
        print(f"✅ Patched: {', '.join(patched_libs)}")

    return results


def unpatch_all():
    """Restore original WebSocket implementations."""
    global _patched

    if not _patched:
        warnings.warn("Not currently patched.")
        return

    # Restore websockets.sync.client
    if 'websockets.sync.client' in sys.modules:
        import websockets.sync.client
        if 'websockets.sync.client.connect' in _original_modules:
            websockets.sync.client.connect = _original_modules['websockets.sync.client.connect']
        if 'websockets.sync.client.WebSocket' in _original_modules:
            websockets.sync.client.WebSocket = _original_modules['websockets.sync.client.WebSocket']

    # Restore websockets async
    if 'websockets' in sys.modules:
        import websockets
        if 'websockets.connect' in _original_modules:
            websockets.connect = _original_modules['websockets.connect']

    # Restore websocket-client
    if 'websocket' in sys.modules:
        import websocket
        if 'websocket.WebSocketApp' in _original_modules:
            websocket.WebSocketApp = _original_modules['websocket.WebSocketApp']
        if 'websocket.WebSocket' in _original_modules:
            websocket.WebSocket = _original_modules['websocket.WebSocket']
        if 'websocket.create_connection' in _original_modules:
            websocket.create_connection = _original_modules['websocket.create_connection']

    _original_modules.clear()
    _patched = False
    print("✅ Unpatched all WebSocket libraries")


def is_patched():
    """Check if currently patched."""
    return _patched


# Auto-patch on import (optional - uncomment to enable)
# patch_all()


# Context manager for temporary patching
class patch_context:
    """Context manager for temporary patching."""

    def __init__(self, include_async=False):
        self.include_async = include_async
        self.was_patched = _patched

    def __enter__(self):
        if not self.was_patched:
            patch_all(include_async=self.include_async)
        return self

    def __exit__(self, *args):
        if not self.was_patched:
            unpatch_all()
        return False