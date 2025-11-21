"""
Advanced auto-patching system with import hooks.

This module provides automatic WebSocket library replacement using Python's import system.
"""

import sys
import importlib
import importlib.util
import importlib.abc
from importlib.machinery import ModuleSpec
from types import ModuleType
from typing import Optional, Sequence
import warnings


class WebSocketRSFinder(importlib.abc.MetaPathFinder):
    """Import hook to intercept WebSocket library imports."""

    def find_spec(self, fullname: str, path: Optional[Sequence[str]], target: Optional[ModuleType] = None) -> Optional[ModuleSpec]:
        """Find and potentially replace WebSocket module specs."""

        # Intercept websockets imports
        if fullname == 'websockets.sync.client':
            return self._create_replacement_spec(fullname)
        elif fullname == 'websocket':  # websocket-client
            return self._create_replacement_spec(fullname)
        elif fullname == 'websockets' and not self._is_already_imported('websockets'):
            # Only intercept base websockets if not already imported
            return self._create_replacement_spec(fullname)

        return None

    def _is_already_imported(self, name: str) -> bool:
        """Check if module is already imported."""
        return name in sys.modules

    def _create_replacement_spec(self, fullname: str) -> ModuleSpec:
        """Create a module spec for our replacement."""
        loader = WebSocketRSLoader(fullname)
        return ModuleSpec(fullname, loader, origin='websocket-rs-replacement')


class WebSocketRSLoader(importlib.abc.Loader):
    """Loader for replacing WebSocket modules with websocket-rs."""

    def __init__(self, fullname: str):
        self.fullname = fullname

    def exec_module(self, module: ModuleType) -> None:
        """Execute module replacement logic."""
        from websocket_rs import WebSocket

        if self.fullname == 'websockets.sync.client':
            # Replace websockets.sync.client
            def connect(uri, **kwargs):
                ws = WebSocket(
                    uri,
                    connect_timeout=kwargs.get('open_timeout', 30),
                    receive_timeout=kwargs.get('close_timeout', 30)
                )
                ws.connect()
                return ws

            module.connect = connect
            module.WebSocket = WebSocket
            module.websocket = WebSocket  # Some code uses lowercase

        elif self.fullname == 'websockets':
            # Create nested structure for websockets
            module.sync = ModuleType('websockets.sync')
            module.sync.client = ModuleType('websockets.sync.client')

            def connect(uri, **kwargs):
                ws = WebSocket(
                    uri,
                    connect_timeout=kwargs.get('open_timeout', 30),
                    receive_timeout=kwargs.get('close_timeout', 30)
                )
                ws.connect()
                return ws

            module.sync.client.connect = connect
            module.sync.client.WebSocket = WebSocket

        elif self.fullname == 'websocket':
            # Replace websocket-client
            module.WebSocket = WebSocket

            def create_connection(url, **kwargs):
                ws = WebSocket(url)
                ws.connect()
                return ws

            module.create_connection = create_connection

            # WebSocketApp compatibility
            class WebSocketApp:
                def __init__(self, url, on_message=None, on_error=None,
                           on_close=None, on_open=None, **kwargs):
                    self.url = url
                    self.on_message = on_message
                    self.on_error = on_error
                    self.on_close = on_close
                    self.on_open = on_open
                    self.ws = None

                def run_forever(self, **kwargs):
                    try:
                        self.ws = WebSocket(self.url)
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
                    if self.ws:
                        self.ws.send(data)

                def close(self):
                    if self.ws:
                        self.ws.close()

            module.WebSocketApp = WebSocketApp

        # Mark as loaded
        module.__loader__ = self
        module.__package__ = self.fullname.rsplit('.', 1)[0] if '.' in self.fullname else ''
        module.__spec__ = ModuleSpec(self.fullname, self, origin='websocket-rs-replacement')


class WebSocketRSImportHook:
    """Main import hook manager."""

    def __init__(self):
        self.finder = WebSocketRSFinder()
        self.enabled = False

    def enable(self):
        """Enable the import hook."""
        if not self.enabled:
            sys.meta_path.insert(0, self.finder)
            self.enabled = True
            print("✅ WebSocket-RS import hook enabled")

    def disable(self):
        """Disable the import hook."""
        if self.enabled:
            try:
                sys.meta_path.remove(self.finder)
                self.enabled = False
                print("✅ WebSocket-RS import hook disabled")
            except ValueError:
                pass

    def __enter__(self):
        """Context manager entry."""
        self.enable()
        return self

    def __exit__(self, *args):
        """Context manager exit."""
        self.disable()
        return False


# Global hook instance
_hook = WebSocketRSImportHook()


def enable_auto_patch():
    """Enable automatic patching for all future WebSocket imports."""
    _hook.enable()


def disable_auto_patch():
    """Disable automatic patching."""
    _hook.disable()


def is_auto_patch_enabled():
    """Check if auto-patching is enabled."""
    return _hook.enabled


# Decorator for patching specific functions
def use_websocket_rs(func):
    """Decorator to use websocket-rs for a specific function."""
    def wrapper(*args, **kwargs):
        with _hook:
            return func(*args, **kwargs)
    wrapper.__name__ = func.__name__
    wrapper.__doc__ = func.__doc__
    return wrapper


# Environment variable support
import os
if os.environ.get('WEBSOCKET_RS_AUTO_PATCH', '').lower() in ('1', 'true', 'yes'):
    enable_auto_patch()