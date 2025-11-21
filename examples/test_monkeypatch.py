#!/usr/bin/env python3
"""
Test monkeypatch functionality of websocket-rs.

This script demonstrates various ways to use the monkeypatch feature
to transparently replace WebSocket libraries with websocket-rs.
"""

import time
import sys


def test_direct_patch():
    """Test direct module patching."""
    print("\n=== Test 1: Direct Patch ===")

    # Import original library first
    import websockets.sync.client

    # Now patch it
    from websocket_rs.patch import patch_all, unpatch_all

    print("Before patch:")
    print(f"  websockets.sync.client.connect: {websockets.sync.client.connect}")

    # Apply patch
    patch_all()

    print("\nAfter patch:")
    print(f"  websockets.sync.client.connect: {websockets.sync.client.connect}")

    # Now any code using websockets will use websocket-rs
    # ws = websockets.sync.client.connect("ws://localhost:8765")
    # This would use the Rust implementation!

    # Restore original
    unpatch_all()
    print("\nAfter unpatch:")
    print(f"  websockets.sync.client.connect: {websockets.sync.client.connect}")


def test_auto_patch():
    """Test automatic import hook patching."""
    print("\n=== Test 2: Auto Patch (Import Hook) ===")

    # Enable auto-patching before imports
    from websocket_rs.auto_patch import enable_auto_patch, disable_auto_patch

    enable_auto_patch()
    print("Auto-patch enabled")

    # Remove from sys.modules to force reimport
    if 'websockets' in sys.modules:
        del sys.modules['websockets']
    if 'websockets.sync' in sys.modules:
        del sys.modules['websockets.sync']
    if 'websockets.sync.client' in sys.modules:
        del sys.modules['websockets.sync.client']

    # Now import - it will be automatically patched
    import websockets.sync.client
    print(f"  websockets.sync.client module: {websockets.sync.client.__spec__.origin}")

    # Disable for cleanup
    disable_auto_patch()


def test_context_manager():
    """Test context manager for temporary patching."""
    print("\n=== Test 3: Context Manager ===")

    from websocket_rs.patch import patch_context

    # Use context manager for temporary patching
    with patch_context() as ctx:
        print("Inside context - patched")
        import websockets.sync.client
        print(f"  connect: {websockets.sync.client.connect}")

    print("\nOutside context - original")
    # Original implementation restored


def test_decorator():
    """Test decorator for function-level patching."""
    print("\n=== Test 4: Decorator ===")

    from websocket_rs.auto_patch import use_websocket_rs

    @use_websocket_rs
    def my_websocket_function():
        """This function will use websocket-rs automatically."""
        # Any WebSocket imports here will use websocket-rs
        import websockets.sync.client
        print(f"  Inside decorated function: {websockets.sync.client}")
        # ws = websockets.sync.client.connect("ws://localhost:8765")
        # This would use websocket-rs!

    my_websocket_function()
    print("  Outside decorated function - uses original")


def test_environment_variable():
    """Test environment variable auto-patching."""
    print("\n=== Test 5: Environment Variable ===")

    import os

    # Set environment variable
    os.environ['WEBSOCKET_RS_AUTO_PATCH'] = '1'

    # Reimport to trigger env check
    import importlib
    import websocket_rs.auto_patch
    importlib.reload(websocket_rs.auto_patch)

    print(f"  Auto-patch enabled via env: {websocket_rs.auto_patch.is_auto_patch_enabled()}")

    # Clean up
    del os.environ['WEBSOCKET_RS_AUTO_PATCH']


def test_global_enable():
    """Test global enable/disable functions."""
    print("\n=== Test 6: Global Enable/Disable ===")

    import websocket_rs

    # Enable global monkeypatch
    websocket_rs.enable_monkeypatch(method='both')
    print("  Global monkeypatch enabled")

    # Now all WebSocket usage will use websocket-rs
    import websockets.sync.client
    print(f"  websockets.sync.client: {websockets.sync.client.connect}")

    # Disable
    websocket_rs.disable_monkeypatch()
    print("  Global monkeypatch disabled")


def benchmark_comparison():
    """Simple benchmark to show performance difference."""
    print("\n=== Performance Comparison ===")

    import websocket_rs

    # Test message
    message = "x" * 1024  # 1KB message

    print("\nNOTE: Actual benchmark requires a WebSocket server running.")
    print("The monkeypatch would make existing code automatically faster.")

    # Example (requires server):
    """
    # Original code
    import websockets.sync.client

    start = time.time()
    with websockets.sync.client.connect("ws://localhost:8765") as ws:
        for _ in range(100):
            ws.send(message)
            ws.recv()
    original_time = time.time() - start

    # With monkeypatch
    websocket_rs.enable_monkeypatch()

    start = time.time()
    with websockets.sync.client.connect("ws://localhost:8765") as ws:
        for _ in range(100):
            ws.send(message)
            ws.recv()
    patched_time = time.time() - start

    print(f"Original: {original_time:.3f}s")
    print(f"Patched: {patched_time:.3f}s")
    print(f"Speedup: {original_time/patched_time:.2f}x")
    """


def main():
    """Run all tests."""
    print("WebSocket-RS Monkeypatch Test Suite")
    print("=" * 40)

    tests = [
        test_direct_patch,
        test_auto_patch,
        test_context_manager,
        test_decorator,
        test_environment_variable,
        test_global_enable,
        benchmark_comparison,
    ]

    for test in tests:
        try:
            test()
        except ImportError as e:
            print(f"  Skipped: {e}")
        except Exception as e:
            print(f"  Error: {e}")

    print("\n" + "=" * 40)
    print("Tests completed!")


if __name__ == "__main__":
    main()