# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

High-performance WebSocket client implementation in Rust with Python bindings. Provides both sync and async APIs with optional compatibility layer for `websockets` library.

## 🎯 Performance Overview

### When to Use What

**For Request-Response Patterns** (chat apps, API calls, gaming):
- 🥇 **picows**: 0.034 ms RTT - Best for extreme low-latency requirements
- 🥈 **websocket-rs Sync**: 0.128 ms RTT - Best balance of performance and simplicity
- 🥉 **websocket-client**: 0.237 ms RTT - Good for simple sync applications

**For High-Concurrency Pipelining** (data streaming, batch processing):
- 🥇 **websocket-rs Async**: 2.964 ms RTT - **12x faster** than picows, **21x faster** than websockets
- 🥈 **picows**: 36.129 ms RTT - Struggles with pipelined batches
- 🥉 **websockets Async**: 61.217 ms RTT - Pure Python limitations

### Why Different Patterns Matter

WebSocket applications use two fundamentally different communication patterns:

1. **Request-Response (RR)**: Send one message → wait for reply → send next
   - Used by: Chat apps, API calls, online games, command-response systems
   - Characteristics: Sequential, blocking, no concurrency
   - Winner: **picows** (event-driven C extension)

2. **Pipelined**: Send multiple messages without waiting → receive all responses
   - Used by: Data streaming, bulk operations, high-throughput systems
   - Characteristics: Concurrent, non-blocking, batched I/O
   - Winner: **websocket-rs Async** (Rust async with Tokio)

📊 **[View Detailed Benchmarks](docs/BENCHMARKS.md)** - Comprehensive performance comparison with test methodology

## ✨ What's New in v0.4.1

### Changes
- Fixed version number sync across all package files
- Cleaned up project structure (removed obsolete python/ directory)
- Improved .gitignore rules for better precision
- Added Cargo.lock to version control
- Created separate BENCHMARKS.md for detailed performance data

### v0.4.0 Highlights

**Pure Sync Client** - Reimplemented using `tungstenite` (non-async):
- Request-Response RTT: **0.128 ms** (was 0.244 ms, **1.9x faster**)
- **1.85x faster** than websocket-client
- **6.2x faster** than websockets

**Architecture Design**:
- Sync client: Pure blocking I/O (simple scripts, CLI tools)
- Async client: Tokio runtime (high concurrency, event-driven)

**Backward Compatibility**:
- 100% API compatible
- No code changes required

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
# Direct usage - Sync API
from websocket_rs.sync_client import connect

with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    response = ws.recv()
    print(response)
```

```python
# Direct usage - Async API
import asyncio
from websocket_rs.async_client import connect

async def main():
    ws = await connect("ws://localhost:8765")
    try:
        await ws.send("Hello")
        response = await ws.recv()
        print(response)
    finally:
        await ws.close()

asyncio.run(main())
```

```python
# Monkeypatch mode (zero code changes)
import websocket_rs
websocket_rs.enable_monkeypatch()

# Existing code using websockets now uses Rust implementation
import websockets.sync.client
with websockets.sync.client.connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
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
connect(
    url: str,                    # WebSocket server URL
    connect_timeout: float = 30, # Connection timeout (seconds)
    receive_timeout: float = 30  # Receive timeout (seconds)
)
```

## 🎯 Choosing the Right Implementation

### Choose **picows** if you need:
- ✅ Absolute lowest latency (<0.1 ms)
- ✅ Request-response pattern (chat, API calls)
- ✅ Team comfortable with event-driven callbacks
- ❌ NOT for: Batch/pipelined operations

### Choose **websocket-rs Sync** if you need:
- ✅ Simple blocking API
- ✅ Good performance (0.1-0.13 ms RTT)
- ✅ Drop-in replacement for `websockets.sync`
- ✅ Request-response pattern
- ✅ 1.85x faster than websocket-client
- ❌ NOT for: Async/await integration

### Choose **websocket-rs Async** if you need:
- ✅ High-concurrency pipelining
- ✅ Batch operations (12x faster than picows)
- ✅ Data streaming applications
- ✅ Integration with Python asyncio
- ❌ NOT for: Simple request-response (use Sync instead)

### Choose **websockets (Python)** if you need:
- ✅ Rapid prototyping
- ✅ Mature ecosystem
- ✅ Comprehensive documentation
- ✅ Low-frequency communication (<10 msg/s)
- ❌ NOT for: High-performance requirements

## 🔧 Advanced Installation

### From GitHub Releases (Pre-built wheels)

```bash
# Specify version
uv pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.3.0/websocket_rs-0.3.0-cp312-abi3-linux_x86_64.whl
```

### From Source

**Requirements**:
- Python 3.12+
- Rust 1.70+ ([rustup.rs](https://rustup.rs/))

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

# Run comprehensive benchmarks (RR + Pipelined)
python tests/benchmark_server_timestamp.py

# Run optimized benchmarks
python tests/benchmark_optimized.py
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

### Performance Trade-offs

**Request-Response Mode:**
- ❌ PyO3 FFI overhead on every call
- ❌ Dual runtime coordination (asyncio + Tokio)
- ✅ Still competitive with pure Python sync
- ✅ Better than Python async for large messages

**Pipelined Mode:**
- ✅ FFI overhead amortized across batch
- ✅ Tokio's concurrency advantages shine
- ✅ No GIL blocking
- ✅ Significant speedup over all Python alternatives

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
