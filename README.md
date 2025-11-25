# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

High-performance WebSocket client implementation in Rust with Python bindings. Provides both sync and async APIs with optional compatibility layer for `websockets` library.

## 🎯 Performance Overview

### When to Use What

**For Request-Response Patterns** (chat apps, API calls, gaming):
- 🥇 **picows**: 0.056 ms RTT - Best for extreme low-latency requirements
- 🥈 **websocket-rs Sync**: 0.244 ms RTT - Best balance of performance and simplicity
- 🥉 **websocket-client**: 0.427 ms RTT - Good for simple sync applications

**For High-Concurrency Pipelining** (data streaming, batch processing):
- 🥇 **websocket-rs Async**: 3.656 ms RTT - **7x faster** than picows, **18x faster** than websockets
- 🥈 **picows**: 25.821 ms RTT - Struggles with pipelined batches
- 🥉 **websockets Async**: 67.591 ms RTT - Pure Python limitations

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
| websocket-client | 0.065 ms | 0.057 ms | 0.123 ms | 12.3x slower |
| **websocket-rs Sync** | 0.057 ms | 0.075 ms | 0.133 ms | 13.3x slower |
| websockets (RR) | 0.082 ms | 0.081 ms | 0.165 ms | 16.5x slower |
| **websocket-rs (RR)** | 0.098 ms | 0.088 ms | 0.187 ms | 18.7x slower |
| websockets (Sync) | 0.084 ms | 0.109 ms | 0.194 ms | 19.4x slower |

**Large Messages (65536 bytes):**

| Implementation | Send (C→S) | Receive (S→C) | RTT | vs Best |
|----------------|------------|---------------|-----|---------|
| **picows (RR)** | 0.032 ms | 0.023 ms | **0.056 ms** | 🏆 1.0x |
| **websocket-rs Sync** | 0.143 ms | 0.100 ms | 0.244 ms | 4.4x slower |
| **websocket-rs (RR)** | 0.130 ms | 0.168 ms | 0.298 ms | 5.3x slower |
| websocket-client | 0.252 ms | 0.174 ms | 0.427 ms | 7.6x slower |
| websockets (RR) | 0.467 ms | 0.481 ms | 0.949 ms | 17x slower |
| websockets (Sync) | 0.464 ms | 0.546 ms | 1.011 ms | 18x slower |

**Key Insights:**
- **picows dominates RR mode**: 4-18x faster than alternatives
- **websocket-rs Sync**: Best Rust option for RR (4.4x slower than picows, but simpler API)
- **websocket-rs Async is slower than Python async**: 13% overhead due to PyO3 bridge + dual runtime

### Pipelined Mode (High Concurrency)

**Large Messages (65536 bytes, sliding window):**

| Implementation | Send (C→S) | Receive (S→C) | RTT | vs Best |
|----------------|------------|---------------|-----|---------|
| **websocket-rs Async** | 2.846 ms | 0.810 ms | **3.656 ms** | 🏆 1.0x |
| picows | 25.444 ms | 0.377 ms | 25.821 ms | 7.1x slower |
| websockets (Async) | 32.609 ms | 34.981 ms | 67.591 ms | 18.5x slower |

**Key Insights:**
- **websocket-rs Async dominates pipelined mode**: 7x faster than picows, 18x faster than websockets
- **Rust async shines in concurrency**: No GIL, Tokio scheduler, zero-cost futures
- **picows struggles with batches**: Event-driven architecture not optimized for pipelined sends

### Performance Scaling by Message Size

**Request-Response Mode:**

| Message Size | picows (RR) | websocket-rs Sync | websocket-rs (RR) | websockets (RR) |
|--------------|-------------|-------------------|-------------------|-----------------|
| 512 B | 0.010 ms | 0.133 ms | 0.187 ms | 0.165 ms |
| 1 KB | 0.010 ms | 0.142 ms | 0.190 ms | 0.161 ms |
| 2 KB | 0.011 ms | 0.141 ms | 0.195 ms | 0.185 ms |
| 4 KB | 0.011 ms | 0.144 ms | 0.201 ms | 0.213 ms |
| 8 KB | 0.012 ms | 0.145 ms | 0.198 ms | 0.258 ms |
| 16 KB | 0.014 ms | 0.224 ms | 0.281 ms | 0.332 ms |
| 32 KB | 0.040 ms | 0.212 ms | 0.261 ms | 0.639 ms |
| 64 KB | 0.056 ms | 0.244 ms | 0.298 ms | 0.949 ms |

**Pipelined Mode:**

| Message Size | websocket-rs Async | picows | websockets (Async) |
|--------------|-------------------|--------|-------------------|
| 512 B | 0.803 ms | 5.104 ms | 2.037 ms |
| 1 KB | 0.929 ms | 4.764 ms | 2.682 ms |
| 4 KB | 1.139 ms | 5.456 ms | 6.413 ms |
| 16 KB | 2.691 ms | 10.826 ms | 27.038 ms |
| 64 KB | 3.656 ms | 25.821 ms | 67.591 ms |

## 🤔 Understanding the Performance Patterns

### Why is websocket-rs Async slower than Python async in RR mode?

**websocket-rs (RR)**: 0.187 ms vs **websockets (RR)**: 0.165 ms (13% slower)

This seems counterintuitive, but it's due to:

1. **PyO3 FFI overhead**: Crossing Python/Rust boundary on every send/recv
2. **Dual async runtime cost**: Python asyncio + Tokio both running
3. **No concurrency benefit in RR**: Sequential operations can't utilize Rust's async advantages
4. **Pure Python async is optimized**: `websockets` is mature, well-tuned pure Python

### Why does websocket-rs Async dominate in Pipelined mode?

**websocket-rs Async**: 3.656 ms vs **picows**: 25.821 ms (7x faster)

Because Rust async excels at concurrency:

1. **True parallelism**: No GIL, can overlap send/receive operations
2. **Tokio's efficiency**: Work-stealing scheduler, zero-cost futures
3. **Batched system calls**: Can merge multiple I/O operations
4. **Memory efficiency**: Compile-time optimizations, no GC pauses

### Why is picows fastest in RR but slower in Pipelined?

**RR**: 0.056 ms (best) vs **Pipelined**: 25.821 ms (7x slower than Rust)

- **RR mode**: Event-driven callback architecture has minimal overhead per message
- **Pipelined mode**: Queue + async coordination overhead becomes significant with batches
- **Optimization focus**: picows optimized for event-driven patterns, not batch sends

## ✨ What's New in v0.3.1

### Performance Improvements

- **Error Handling Optimization**: 50x faster error path with ReadyFuture error support
  - Error processing: 10M+ errors/sec (0.1μs per error)
  - Bypasses 4 Python C API calls in asyncio error path

- **Event Loop Cache**: Per-connection event loop caching
  - Reduces redundant `get_running_loop()` calls
  - 1-2% improvement in slow path scenarios

- **API Safety**: Updated to `get_running_loop()` (Python 3.10+)
  - Better thread safety
  - Prevents event loop confusion across threads

### Test Results (v0.3.1)

Based on 1000 messages (1KB each):

| Scenario | Latency | Throughput |
|----------|---------|------------|
| Request-Response | 232.64ms | 4,300 ops/s |
| Pipelined | 105.39ms | 9,500 ops/s |
| Error Path (10k) | 1.00ms | 10M errors/s |
| Mixed (90/10) | 200.03ms | 5,000 ops/s |

### Stability

- Standard deviation: <1-2%
- Consistent performance across multiple test runs
- Optimizations complete for Actor Pattern + PyO3 architecture

## 🚀 Quick Start

### Installation

```bash
# Using uv (recommended)
uv pip install git+https://github.com/coseto6125/websocket-rs.git

# Using pip
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
- ✅ Good performance (0.2-0.3 ms)
- ✅ Drop-in replacement for `websockets.sync`
- ✅ Request-response pattern
- ❌ NOT for: Async/await integration

### Choose **websocket-rs Async** if you need:
- ✅ High-concurrency pipelining
- ✅ Batch operations (7x faster than picows)
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
