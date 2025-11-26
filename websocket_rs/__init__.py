"""
WebSocket-RS: High-performance WebSocket client for Python.

This module provides API-compatible replacements for websockets library
with significantly better performance through Rust implementation.
"""

import sys

# Import Rust module
from . import websocket_rs as _websocket_rs

# Register submodules in sys.modules for proper import support
sys.modules["websocket_rs.sync"] = _websocket_rs.sync
sys.modules["websocket_rs.sync.client"] = _websocket_rs.sync.client
sys.modules["websocket_rs.async_client"] = _websocket_rs.async_client

# Re-export for convenience
sync = _websocket_rs.sync
async_client = _websocket_rs.async_client

# Version
__version__ = "0.4.0"

# Public API
__all__ = [
    "sync",
    "async_client",
]
