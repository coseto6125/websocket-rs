from . import async_client, sync

def enable_monkeypatch() -> None:
    """
    Enables monkey-patching globally. This substitutes `websocket_rs`
    (backed by the `websocket-rs` crate) in place of native websocket
    implementations where applicable.
    """
    ...

__all__ = [
    "enable_monkeypatch",
    "sync",
    "async_client",
]
