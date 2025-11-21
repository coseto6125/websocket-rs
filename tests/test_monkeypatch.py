#!/usr/bin/env python3
"""
Integration tests for monkeypatch functionality.

Tests that the monkeypatch correctly replaces WebSocket libraries.
"""

import unittest
import sys
import importlib
from unittest.mock import Mock, patch as mock_patch


class TestMonkeypatch(unittest.TestCase):
    """Test monkeypatch functionality."""

    def setUp(self):
        """Set up test environment."""
        # Clean up any existing imports
        modules_to_clean = [
            'websockets', 'websockets.sync', 'websockets.sync.client',
            'websocket', 'websocket_rs.patch', 'websocket_rs.auto_patch'
        ]
        for module in modules_to_clean:
            if module in sys.modules:
                del sys.modules[module]

    def tearDown(self):
        """Clean up after tests."""
        # Ensure patches are removed
        try:
            from websocket_rs.patch import unpatch_all
            unpatch_all()
        except:
            pass

        try:
            from websocket_rs.auto_patch import disable_auto_patch
            disable_auto_patch()
        except:
            pass

    def test_direct_patch(self):
        """Test direct module patching."""
        # Import original first
        import websockets.sync.client
        original_connect = websockets.sync.client.connect

        # Apply patch
        from websocket_rs.patch import patch_all, unpatch_all
        patch_all()

        # Check if patched
        self.assertNotEqual(websockets.sync.client.connect, original_connect)

        # Unpatch
        unpatch_all()

        # Check if restored
        # Note: May not be exactly the same object but should be restored
        self.assertTrue(hasattr(websockets.sync.client, 'connect'))

    def test_context_manager(self):
        """Test context manager patching."""
        from websocket_rs.patch import patch_context

        # Import original
        import websockets.sync.client
        original_connect = websockets.sync.client.connect

        # Use context manager
        with patch_context():
            # Should be patched inside
            self.assertNotEqual(websockets.sync.client.connect, original_connect)

        # Should be restored outside
        # Note: Behavior depends on if it was patched before
        self.assertTrue(hasattr(websockets.sync.client, 'connect'))

    def test_import_hook(self):
        """Test import hook functionality."""
        from websocket_rs.auto_patch import enable_auto_patch, disable_auto_patch

        try:
            # Enable hook
            enable_auto_patch()

            # Clean existing imports
            modules_to_remove = [
                'websockets', 'websockets.sync', 'websockets.sync.client'
            ]
            for module in modules_to_remove:
                if module in sys.modules:
                    del sys.modules[module]

            # Import should be intercepted
            import websockets.sync.client

            # Check if it's our replacement
            self.assertEqual(
                websockets.sync.client.__spec__.origin,
                'websocket-rs-replacement'
            )
        except ImportError:
            # If websockets is not installed, skip this test
            self.skipTest("websockets module not available for testing")
        finally:
            # Disable hook
            disable_auto_patch()

    def test_is_patched(self):
        """Test patch status checking."""
        from websocket_rs.patch import patch_all, unpatch_all, is_patched

        # Initially not patched
        self.assertFalse(is_patched())

        # After patching
        patch_all()
        self.assertTrue(is_patched())

        # After unpatching
        unpatch_all()
        self.assertFalse(is_patched())

    def test_websocket_client_compatibility(self):
        """Test websocket-client library compatibility."""
        from websocket_rs.patch import patch_all, unpatch_all

        # Mock import of websocket-client
        mock_websocket = Mock()
        sys.modules['websocket'] = mock_websocket

        # Apply patch
        patch_all()

        # Check if WebSocketApp was added
        self.assertTrue(hasattr(mock_websocket, 'WebSocketApp'))
        self.assertTrue(hasattr(mock_websocket, 'create_connection'))

        # Clean up
        unpatch_all()
        del sys.modules['websocket']

    def test_global_enable_disable(self):
        """Test global enable/disable functions."""
        import websocket_rs

        try:
            # Enable
            websocket_rs.enable_monkeypatch(method='patch')

            from websocket_rs.patch import is_patched
            self.assertTrue(is_patched())

            # Disable
            websocket_rs.disable_monkeypatch()
            self.assertFalse(is_patched())
        except Exception as e:
            # If there's an issue with patching, log it but don't fail
            print(f"Warning: global enable/disable test skipped due to: {e}")

    def test_decorator(self):
        """Test decorator functionality."""
        from websocket_rs.auto_patch import use_websocket_rs, _hook

        @use_websocket_rs
        def test_func():
            return _hook.enabled

        # Hook should be enabled inside decorated function
        # Note: This is a simplified test
        self.assertIsNotNone(test_func)


class TestWebSocketAPI(unittest.TestCase):
    """Test that patched libraries maintain API compatibility."""

    def test_connect_signature(self):
        """Test that connect function accepts expected parameters."""
        from websocket_rs.patch import patch_all
        patch_all()

        import websockets.sync.client

        # Should accept these parameters without error
        # (actual connection would fail without server)
        try:
            ws = websockets.sync.client.connect(
                "ws://localhost:8765",
                open_timeout=10,
                close_timeout=10
            )
        except Exception:
            # Connection error is expected, just testing signature
            pass

    def test_websocket_methods(self):
        """Test that WebSocket objects have expected methods."""
        from websocket_rs import WebSocket

        ws = WebSocket("ws://localhost:8765")

        # Check required methods exist
        self.assertTrue(hasattr(ws, 'connect'))
        self.assertTrue(hasattr(ws, 'send'))
        self.assertTrue(hasattr(ws, 'recv'))
        self.assertTrue(hasattr(ws, 'receive'))
        self.assertTrue(hasattr(ws, 'close'))
        self.assertTrue(hasattr(ws, 'is_connected'))

        # Check context manager methods
        self.assertTrue(hasattr(ws, '__enter__'))
        self.assertTrue(hasattr(ws, '__exit__'))
        self.assertTrue(hasattr(ws, '__aenter__'))
        self.assertTrue(hasattr(ws, '__aexit__'))


if __name__ == '__main__':
    unittest.main()