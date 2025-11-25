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

## 📊 Comprehensive Benchmark Results

**Test Environment**: WSL2 Ubuntu, Python 3.13, localhost echo server, 1000 messages per test

### Request-Response Mode (Real-World Usage)

**Small Messages (512 bytes):**

| Implementation | Send (C→S) | Receive (S→C) | RTT | vs Best |
|----------------|------------|---------------|-----|---------|
| **picows (RR)** | 0.005 ms | 0.005 ms | **0.010 ms** | 🏆 1.0x |
| **websocket-rs Sync** | 0.053 ms | 0.047 ms | 0.101 ms | 10.1x slower |
| websocket-client | 0.069 ms | 0.060 ms | 0.129 ms | 12.9x slower |
| websockets (RR) | 0.084 ms | 0.083 ms | 0.167 ms | 16.7x slower |
| websockets (Sync) | 0.084 ms | 0.109 ms | 0.194 ms | 19.4x slower |
| **websocket-rs (RR)** | 0.103 ms | 0.092 ms | 0.196 ms | 19.6x slower |

**Large Messages (65536 bytes):**

| Implementation | Send (C→S) | Receive (S→C) | RTT | vs Best |
|----------------|------------|---------------|-----|---------|
| **picows (RR)** | 0.021 ms | 0.013 ms | **0.034 ms** | 🏆 1.0x |
| **websocket-rs Sync** | 0.070 ms | 0.057 ms | 0.128 ms | 3.8x slower |
| **websocket-rs (RR)** | 0.129 ms | 0.108 ms | 0.238 ms | 7.0x slower |
| websocket-client | 0.162 ms | 0.074 ms | 0.237 ms | 7.0x slower |
| websockets (RR) | 0.392 ms | 0.397 ms | 0.790 ms | 23.2x slower |
| websockets (Sync) | 0.391 ms | 0.421 ms | 0.814 ms | 23.9x slower |

**Key Insights:**
- **picows dominates RR mode**: 3-23x faster than alternatives
- **websocket-rs Sync**: Best Rust option for RR (3.8x slower than picows, 1.85x faster than websocket-client)
- **websocket-rs Async is slower than Python async**: 19% overhead due to PyO3 bridge + dual runtime

### Pipelined Mode (High Concurrency)

**Large Messages (65536 bytes, sliding window):**

| Implementation | Send (C→S) | Receive (S→C) | RTT | vs Best |
|----------------|------------|---------------|-----|---------|
| **websocket-rs Async** | 2.329 ms | 0.635 ms | **2.964 ms** | 🏆 1.0x |
| picows | 35.719 ms | 0.409 ms | 36.129 ms | 12.2x slower |
| websockets (Async) | 31.197 ms | 30.019 ms | 61.217 ms | 20.7x slower |

**Key Insights:**
- **websocket-rs Async dominates pipelined mode**: 12x faster than picows, 21x faster than websockets
- **Rust async shines in concurrency**: No GIL, Tokio scheduler, zero-cost futures
- **picows struggles with batches**: Event-driven architecture not optimized for pipelined sends

### Performance Scaling by Message Size

**Request-Response Mode:**

| Message Size | picows (RR) | websocket-rs Sync | websocket-rs (RR) | websockets (RR) |
|--------------|-------------|-------------------|-------------------|-----------------|
| 512 B | 0.010 ms | 0.101 ms | 0.196 ms | 0.167 ms |
| 1 KB | 0.010 ms | 0.102 ms | 0.199 ms | 0.175 ms |
| 2 KB | 0.012 ms | 0.106 ms | 0.198 ms | 0.183 ms |
| 4 KB | 0.011 ms | 0.103 ms | 0.201 ms | 0.212 ms |
| 8 KB | 0.012 ms | 0.109 ms | 0.201 ms | 0.255 ms |
| 16 KB | 0.015 ms | 0.109 ms | 0.204 ms | 0.339 ms |
| 32 KB | 0.020 ms | 0.115 ms | 0.210 ms | 0.489 ms |
| 64 KB | 0.034 ms | 0.128 ms | 0.238 ms | 0.790 ms |

**Pipelined Mode:**

| Message Size | websocket-rs Async | picows | websockets (Async) |
|--------------|-------------------|--------|-------------------|
| 512 B | 0.873 ms | 4.904 ms | 2.206 ms |
| 1 KB | 0.889 ms | 4.749 ms | 2.626 ms |
| 4 KB | 1.094 ms | 5.926 ms | 6.309 ms |
| 16 KB | 2.034 ms | 14.398 ms | 17.562 ms |
| 64 KB | 2.964 ms | 36.129 ms | 61.217 ms |

## 🤔 Understanding the Performance Patterns

### Why is websocket-rs Async slower than Python async in RR mode?

**websocket-rs (RR)**: 0.196 ms vs **websockets (RR)**: 0.167 ms (17% slower)

This seems counterintuitive, but it's due to:

1. **PyO3 FFI overhead**: Crossing Python/Rust boundary on every send/recv
2. **Dual async runtime cost**: Python asyncio + Tokio both running
3. **No concurrency benefit in RR**: Sequential operations can't utilize Rust's async advantages
4. **Pure Python async is optimized**: `websockets` is mature, well-tuned pure Python

### Why does websocket-rs Async dominate in Pipelined mode?

**websocket-rs Async**: 2.964 ms vs **picows**: 36.129 ms (12x faster)

Because Rust async excels at concurrency:

1. **True parallelism**: No GIL, can overlap send/receive operations
2. **Tokio's efficiency**: Work-stealing scheduler, zero-cost futures
3. **Batched system calls**: Can merge multiple I/O operations
4. **Memory efficiency**: Compile-time optimizations, no GC pauses

### Why is picows fastest in RR but slower in Pipelined?

**RR**: 0.034 ms (best) vs **Pipelined**: 36.129 ms (12x slower than Rust)

- **RR mode**: Event-driven callback architecture has minimal overhead per message
- **Pipelined mode**: Queue + async coordination overhead becomes significant with batches
- **Optimization focus**: picows optimized for event-driven patterns, not batch sends

## ✨ What's New in v0.4.0

### Pure Sync Client - Remove Async Overhead

v0.4.0 reimplements Sync Client using `tungstenite` (non-async) instead of Tokio runtime wrapper:

**Implementation Changes**:
- Use `tungstenite` (blocking I/O) instead of `tokio-tungstenite`
- Remove Tokio runtime overhead
- Direct `std::net::TcpStream` usage

**Performance Gains**:
- Request-Response RTT: **0.128 ms** (was 0.244 ms, 1.9x faster)
- **1.85x faster** than websocket-client
- **6.2x faster** than websockets

**Architecture Design**:
- Sync client: Pure blocking I/O (simple scripts, CLI tools)
- Async client: Tokio runtime (high concurrency, event-driven)
- Specialized implementation for each use case

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
