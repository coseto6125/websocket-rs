# API Documentation

Complete API reference for websocket-rs.

> **Deprecation notice:** `websocket_rs.async_client` is deprecated and will be
> removed in 2.0. Use the canonical `websocket_rs.connect` (native client) instead.

## Installation

```bash
pip install websocket-rs
```

## Quick Start

```python
# Sync API
from websocket_rs.sync.client import connect

with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
```

```python
# Async API (canonical native client)
import asyncio
from websocket_rs import connect

async def main():
    ws = await connect("ws://localhost:8765")
    try:
        await ws.send("Hello")
        print(await ws.recv())
    finally:
        await ws.close()

asyncio.run(main())
```

---

## Sync API

### `websocket_rs.sync.client.connect()`

Connect to a WebSocket server (blocking). The connection dials immediately —
the returned object is already connected, with or without a `with` block.

**Signature:**
```python
def connect(
    uri: str,
    subprotocols: list[str] | None = None,
    connect_timeout: float | None = None,   # default 10.0
    receive_timeout: float | None = None,   # default 10.0
    close_timeout: float | None = None,     # default 10.0
    tcp_nodelay: bool | None = None,        # default True
) -> ClientConnection
```

**Parameters:**
- `uri` (str): WebSocket server URL (e.g., `"ws://localhost:8765"`)
- `subprotocols` (list[str], optional): Offered as `Sec-WebSocket-Protocol`; negotiated value on `.subprotocol`
- `connect_timeout` (float, optional): Connection timeout in seconds. Default: 10.0
- `receive_timeout` (float, optional): Receive timeout in seconds. Default: 10.0
- `close_timeout` (float, optional): Close-handshake timeout in seconds. Default: 10.0
- `tcp_nodelay` (bool, optional): Disable Nagle's algorithm. Default: True

Any other keyword argument raises `TypeError`: headers, ssl_context,
proxy, compression and on_message are native-client features.

**Returns:**
- `ClientConnection`: Connected WebSocket client

**Example:**
```python
from websocket_rs.sync.client import connect

# Basic connection
ws = connect("ws://echo.websocket.org")

# With custom timeouts
ws = connect(
    "ws://localhost:8765",
    connect_timeout=10.0,
    receive_timeout=60.0
)
```

---

### `WebSocket.send()`

Send a message to the server.

**Signature:**
```python
def send(self, message: str | bytes) -> None
```

**Parameters:**
- `message` (str | bytes): Message to send

**Raises:**
- `RuntimeError`: If connection is closed or send fails

**Example:**
```python
# Send text
ws.send("Hello, World!")

# Send binary
ws.send(b"\x00\x01\x02\x03")
```

---

### `WebSocket.recv()`

Receive a message from the server (blocking).

**Signature:**
```python
def recv(self) -> str | bytes
```

**Returns:**
- `str | bytes`: Received message

**Raises:**
- `RuntimeError`: If connection is closed or receive fails
- `TimeoutError`: If receive timeout is reached

**Example:**
```python
message = ws.recv()
print(f"Received: {message}")
```

---

### `WebSocket.close()`

Close the WebSocket connection.

**Signature:**
```python
def close(self) -> None
```

**Example:**
```python
ws.close()
```

---

### Context Manager Support

The sync WebSocket supports context manager protocol.

**Example:**
```python
with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
# Connection automatically closed
```

---

## Async API

### `websocket_rs.async_client.connect()`

> **Deprecated**, removed in 2.0. Emits a `DeprecationWarning`;
> use `websocket_rs.connect` instead.

Create and connect to a WebSocket server (async).

**Signature:**
```python
async def connect(
    url: str,
    connect_timeout: float | None = None,   # default 10.0
    receive_timeout: float | None = None,   # default 10.0
) -> AsyncWebSocket
```

**Parameters:**
- `url` (str): WebSocket server URL
- `connect_timeout` (float, optional): Connection timeout in seconds. Default: 10.0
- `receive_timeout` (float, optional): Receive timeout in seconds. Default: 10.0

**Returns:**
- `AsyncWebSocket`: Connected async WebSocket client

**Example:**
```python
import asyncio
from websocket_rs.async_client import connect

async def main():
    ws = await connect("ws://echo.websocket.org")

    # With custom timeouts
    ws = await connect(
        "ws://localhost:8765",
        connect_timeout=10.0,
        receive_timeout=60.0
    )

asyncio.run(main())
```

---

### `AsyncWebSocket.send()`

Send a message to the server (async).

**Signature:**
```python
async def send(self, message: str | bytes) -> None
```

**Parameters:**
- `message` (str | bytes): Message to send

**Raises:**
- `RuntimeError`: If connection is closed or send fails

**Example:**
```python
# Send text
await ws.send("Hello, World!")

# Send binary
await ws.send(b"\x00\x01\x02\x03")
```

---

### `AsyncWebSocket.recv()`

Receive a message from the server (async).

**Signature:**
```python
async def recv(self) -> str | bytes
```

**Returns:**
- `str | bytes`: Received message

**Raises:**
- `RuntimeError`: If connection is closed or receive fails
- `TimeoutError`: If receive timeout is reached

**Example:**
```python
message = await ws.recv()
print(f"Received: {message}")
```

---

### `AsyncWebSocket.close()`

Close the WebSocket connection (async).

**Signature:**
```python
async def close(self) -> None
```

**Example:**
```python
await ws.close()
```

---

### Async Context Manager Support

The async WebSocket supports async context manager protocol.

**Example:**
```python
async with connect("ws://localhost:8765") as ws:
    await ws.send("Hello")
    print(await ws.recv())
# Connection automatically closed
```

---

## Complete Examples

### Echo Client (Sync)

```python
from websocket_rs.sync.client import connect

def echo_client():
    with connect("ws://echo.websocket.org") as ws:
        # Send messages
        messages = ["Hello", "World", "Goodbye"]
        for msg in messages:
            ws.send(msg)
            response = ws.recv()
            print(f"Sent: {msg}, Received: {response}")

echo_client()
```

### Echo Client (Async)

```python
import asyncio
from websocket_rs.async_client import connect

async def echo_client():
    ws = await connect("ws://echo.websocket.org")
    try:
        # Send messages
        messages = ["Hello", "World", "Goodbye"]
        for msg in messages:
            await ws.send(msg)
            response = await ws.recv()
            print(f"Sent: {msg}, Received: {response}")
    finally:
        await ws.close()

asyncio.run(echo_client())
```

### Request-Response Pattern (Sync)

```python
from websocket_rs.sync.client import connect
import json

def api_request(endpoint, data):
    with connect("ws://api.example.com") as ws:
        # Send request
        request = json.dumps({
            "endpoint": endpoint,
            "data": data
        })
        ws.send(request)

        # Wait for response
        response = ws.recv()
        return json.loads(response)

# Use it
result = api_request("/users/create", {"name": "Alice"})
print(result)
```

### Streaming Data (Async)

```python
import asyncio
from websocket_rs.async_client import connect

async def stream_data():
    ws = await connect("ws://stream.example.com")
    try:
        # Subscribe to stream
        await ws.send('{"action": "subscribe", "channel": "prices"}')

        # Receive streaming data
        async def receive_loop():
            while True:
                try:
                    data = await ws.recv()
                    print(f"Received: {data}")
                except Exception as e:
                    print(f"Error: {e}")
                    break

        await receive_loop()
    finally:
        await ws.close()

asyncio.run(stream_data())
```

### Concurrent Connections (Async)

```python
import asyncio
from websocket_rs.async_client import connect

async def fetch_data(url, message):
    ws = await connect(url)
    try:
        await ws.send(message)
        response = await ws.recv()
        return response
    finally:
        await ws.close()

async def main():
    # Connect to multiple servers concurrently
    urls = [
        "ws://server1.example.com",
        "ws://server2.example.com",
        "ws://server3.example.com",
    ]

    tasks = [fetch_data(url, "Hello") for url in urls]
    results = await asyncio.gather(*tasks)

    for url, result in zip(urls, results):
        print(f"{url}: {result}")

asyncio.run(main())
```

---

## Error Handling

### Common Exceptions

- `RuntimeError`: Connection closed or operation failed
- `TimeoutError`: Operation timeout exceeded
- `ValueError`: Invalid parameters

### Example

```python
from websocket_rs.sync.client import connect

try:
    with connect("ws://localhost:8765", connect_timeout=5.0) as ws:
        ws.send("Hello")
        message = ws.recv()
        print(message)
except TimeoutError:
    print("Connection or receive timeout")
except RuntimeError as e:
    print(f"WebSocket error: {e}")
except Exception as e:
    print(f"Unexpected error: {e}")
```

---

## Performance Tips

### For Request-Response Pattern
Use **Sync API** for best performance:
```python
# Fastest for request-response
from websocket_rs.sync.client import connect
```

### For High Concurrency / Pipelining
Use **Async API** for best performance:
```python
# Fastest for concurrent operations
from websocket_rs.async_client import connect
```

### Event Loop Caching (v0.3.1+)
The async client automatically caches the event loop on first access:
```python
# Both patterns benefit from event loop caching
# Pattern 1: Using context manager (recommended)
async with connect("ws://...") as ws:
    await ws.send("msg")  # Uses cached event loop

# Pattern 2: Manual management (also optimized)
ws = await connect("ws://...")
await ws.send("msg")  # Event loop cached on first access
await ws.close()
```

**Performance impact:**
- 25% improvement for non-context-manager usage
- ~0.014μs saved per operation
- Automatic write-back on first access

### Batch Operations
When sending/receiving multiple messages, use async API:
```python
async def batch_send():
    ws = await connect("ws://localhost:8765")

    # Send all messages first (pipelined)
    for i in range(1000):
        await ws.send(f"Message {i}")

    # Then receive all responses
    for i in range(1000):
        response = await ws.recv()

    await ws.close()
```

---

## Version Information

Check the installed version:

```python
import websocket_rs
print(websocket_rs.__version__)
```

---

## Comparison with Python `websockets`

websocket-rs is designed as a high-performance drop-in replacement:

| Feature | websocket-rs | websockets (Python) |
|---------|--------------|---------------------|
| **Sync API** | ✅ Fast | ✅ Standard |
| **Async API** | ✅ Very Fast (concurrent) | ✅ Standard |
| **Request-Response** | 🏆 0.2-0.3 ms | 0.8-1.0 ms |
| **Pipelined (64KB)** | 🏆 3.7 ms | 67.6 ms |
| **Zero dependencies** | ✅ Yes | ❌ No |
| **Rust implementation** | ✅ Yes | ❌ Pure Python |

See [README.md](README.md) for comprehensive benchmarks.

---

## Support

- **GitHub**: https://github.com/coseto6125/websocket-rs
- **Issues**: https://github.com/coseto6125/websocket-rs/issues
- **PyPI**: https://pypi.org/project/websocket-rs/
