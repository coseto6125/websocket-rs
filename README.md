# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

High-performance WebSocket client implementation in Rust with Python bindings. Provides both sync and async APIs compatible with `websockets` library.

## 🎯 Performance

### Request-Response Throughput (picows-parity benchmark, 10s, RPS)

Matching picows's official benchmark methodology — RR mode (send → wait → recv → repeat), neutral Rust echo server (tokio-tungstenite), pinned cores, uvloop, N≈260–14.4k per cell over 10s:

| Payload | **ws-rs sync** | **ws-rs async** | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.8k** | 13.2k | 12.5k | 10.7k | 9.6k | 10.6k |
| 8 KB  | **13.2k** | 12.5k | 11.7k | 10.4k | 8.7k | 10.2k |
| 100 KB | **10.3k** | 9.5k | 10.0k | 8.6k | 7.4k | 4.5k |
| 2 MB  | 2.3k | **2.6k** | **2.6k** | 2.2k | 2.0k | 261 |

> websocket-rs leads in **24/24 cells** across three server architectures (tokio-tungstenite, fastwebsockets, picows-server). Sync API wins 256 B–100 KB (no asyncio overhead); async ties picows at 2 MB. Margin over picows is 2–18%; over websockets/aiohttp is 15–60%; over websocket-client is 2–10× at ≥100 KB.

### vs websockets 15.0 — Sync vs Async API (localhost, 200 roundtrips)

| Payload | Sync | Async |
|---------|------|-------|
| 32 B    | **1.8x** faster | 0.87x |
| 1 KB    | **1.9x** faster | 0.88x |
| 16 KB   | **3.2x** faster | **1.5x** faster |
| 64 KB   | **6.0x** faster | **3.1x** faster |
| 256 KB  | **7.7x** faster | **7.7x** faster |
| 1 MB    | **13.3x** faster | **18.1x** faster |

> Larger payloads amplify Rust's zero-copy parsing advantage. Async overtakes Python websockets starting at 8KB+.

📊 **[Full benchmarks — all sizes, all servers, pipelined mode, cases where we lose](docs/BENCHMARKS.md)** | 📝 **[Optimization Research](docs/OPTIMIZATION_RESEARCH.md)**

## ✨ What's New in v0.6.0

### Performance Optimizations
- `pyo3::intern!` for all Python string lookups in hot paths
- Zero-copy recv path (sync client)
- Ping/Pong filtered in background task (async client)
- TCP_NODELAY enabled by default
- Address stored as pre-parsed tuple

### SOCKS5 Proxy & Custom Headers (v0.6.0)
```python
ws = await connect("wss://example.com/ws",
                   proxy="socks5://127.0.0.1:1080",
                   headers={"Authorization": "Bearer token"})
```

### Type Stubs (PEP 561)
- Full `.pyi` type stubs for IDE autocomplete and type checkers
- `py.typed` marker included

## 🚀 Quick Start

### Installation

```bash
# From PyPI (recommended)
pip install websocket-rs

# Using uv
uv pip install websocket-rs

# From source
pip install git+https://github.com/coseto6125/websocket-rs.git
```

### Basic Usage

```python
# Sync API
from websocket_rs.sync.client import connect

with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    response = ws.recv()
    print(response)
```

```python
# Async API
import asyncio
from websocket_rs.async_client import connect

async def main():
    async with await connect("ws://localhost:8765") as ws:
        await ws.send("Hello")
        response = await ws.recv()
        print(response)

asyncio.run(main())
```

## 📖 API Documentation

### Standard API (Compatible with Python websockets)

| Method | Description | Example |
|--------|-------------|---------|
| `connect(url)` | Create and connect WebSocket | `ws = connect("ws://localhost:8765")` |
| `send(message)` | Send message (str or bytes) | `ws.send("Hello")` |
| `recv()` | Receive message | `msg = ws.recv()` |
| `close()` | Close connection | `ws.close()` |

### Connection Parameters

```python
# Sync
from websocket_rs.sync.client import ClientConnection
ws = ClientConnection(url, connect_timeout=10.0, receive_timeout=10.0, tcp_nodelay=True)

# Async
from websocket_rs.async_client import ClientConnection
ws = ClientConnection(url, headers={"Key": "val"}, proxy="socks5://host:port",
                      connect_timeout=10.0, receive_timeout=10.0, tcp_nodelay=True)
```

## 🎯 When to Use

| Scenario | Recommendation | Why |
|----------|---------------|-----|
| Simple scripts, CLI tools | **Sync** | 1.8x faster, no event loop needed |
| Large payload (16KB+) | **Sync or Async** | 3-18x faster, zero-copy parsing |
| High concurrency | **Async** | Supports thousands of connections |
| Small messages, low latency | **Sync** or Python websockets | Async bridge overhead for small msgs |
| Cross-network (real servers) | **Either** | Network latency dominates, < 1% difference |

## 🔧 Advanced Installation

### From GitHub Releases (Pre-built wheels)

```bash
# Specify version (example for Linux x86_64, Python 3.12+)
uv pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.6.0/websocket_rs-0.6.0-cp312-abi3-manylinux_2_34_x86_64.whl
```

### From Source

**Requirements**:
- Python 3.12+
- Rust 1.80+ ([rustup.rs](https://rustup.rs/))

```bash
git clone https://github.com/coseto6125/websocket-rs.git
cd websocket-rs
pip install maturin
maturin develop --release
```

### Using in pyproject.toml

```toml
[project]
dependencies = [
    "websocket-rs @ git+https://github.com/coseto6125/websocket-rs.git@main",
]
```

## 🧪 Running Tests and Benchmarks

```bash
# Run API compatibility tests
python tests/test_compatibility.py

# Run picows-parity RPS benchmark (5 clients × 3 server architectures × 4 sizes, ~10 min)
python tests/benchmark_picows_parity.py

# Run latency-distribution benchmark (RR + pipelined, mean/p50/p99)
python tests/benchmark_three_servers.py
```

## 🛠️ Development

### Local Development with uv (Recommended)

```bash
# Install uv (if not already installed)
curl -LsSf https://astral.sh/uv/install.sh | sh

# Setup development environment
make install  # Creates venv and installs dependencies

# Build and test
make dev      # Development build
make test     # Run tests
make bench    # Run benchmarks

# Or manually with uv
uv venv
source .venv/bin/activate
uv pip install -e ".[dev]"
maturin develop --release
```

### Traditional Development (pip)

```bash
# Install development dependencies
pip install maturin pytest websockets

# Development mode (fast iteration)
maturin develop

# Release mode (best performance)
maturin develop --release

# Watch mode (auto-recompile)
maturin develop --release --watch
```

## 📐 Technical Architecture

### Why Rust for WebSockets?

1. **Zero-cost abstractions**: Rust's async/await compiles to efficient state machines
2. **Tokio runtime**: Work-stealing scheduler optimized for I/O-bound tasks
3. **No GIL**: True parallelism for concurrent operations
4. **Memory safety**: No segfaults, data races, or memory leaks

### Performance Architecture

**Sync Client** — Pure `tungstenite` (blocking I/O), no async runtime overhead:
```
Python → GIL release → Rust tungstenite → socket I/O → result
```

**Async Client** — Actor pattern with `tokio-tungstenite`:
```
Python → mpsc channel → tokio background task → socket I/O
         ↑ ReadyFuture fast path when data is buffered
```

The async bridge adds ~36µs per operation (channel + `call_soon_threadsafe`).
This is negligible for large payloads or cross-network usage, but visible on localhost small messages.

## 🐛 Troubleshooting

### Compilation Issues

```bash
# Check Rust version
rustc --version  # Requires >= 1.70

# Clean and rebuild
cargo clean
maturin develop --release

# Verbose mode
maturin develop --release -v
```

### Runtime Issues

- **TimeoutError**: Increase `connect_timeout` parameter
- **Module not found**: Run `maturin develop` first
- **Connection refused**: Check if server is running
- **Performance not as expected**: Ensure using `--release` build

## 🤝 Contributing

Contributions welcome! Please ensure:

1. All tests pass
2. API compatibility is maintained
3. Performance benchmarks included
4. Documentation updated

## 📄 License

MIT License - See [LICENSE](LICENSE)

## 🙏 Acknowledgments

- [PyO3](https://github.com/PyO3/pyo3) - Rust Python bindings
- [Tokio](https://tokio.rs/) - Async runtime
- [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) - WebSocket implementation
- [websockets](https://github.com/python-websockets/websockets) - Python WebSocket library
- [picows](https://github.com/tarasko/picows) - High-performance Python WebSocket client

## 📚 Further Reading

- [Why Rust async is fast](https://tokio.rs/tokio/tutorial)
- [PyO3 performance guide](https://pyo3.rs/main/doc/pyo3/performance)
- [WebSocket protocol RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455)
