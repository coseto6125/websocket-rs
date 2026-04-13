# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

Fastest Python WebSocket client on plain TCP — wins **12/12** benchmark cells against picows, aiohttp, websockets, and websocket-client across three server architectures. Sync **and** async APIs. Pure-Rust internals (PyO3 + rustls TLS), `~9 KB` per idle connection.

## 🎯 Why websocket-rs

| Your situation | Pick | Why |
|---|---|---|
| Plain Python script / CLI tool (no asyncio) | **ws-rs sync** | picows is async-only; 1.3–7× over websocket-client (size-dependent) |
| asyncio app, sequential RR ≤ 8 KB | **ws-rs sync** via `to_thread` | +13–28% over picows; sync bypasses asyncio overhead |
| asyncio app, pipelined / streaming | **ws-rs async** | mean RPS +14–21% over picows on best path |
| asyncio app, ≥ 1 MB payloads | **ws-rs async** ≈ **picows** | tied within 5%; server-side ceiling |
| RR latency-sensitive, payload ≥ 16 KB | **picows** | 3–5 µs lower per-message RTT (Cython cdef vs PyO3) |
| Cross-network (real server, real RTT) | **Either** | network RT dominates; library choice < 1% of total latency |

> **vs picows**
> - **ws-rs wins**: pipelined throughput **+14–21% mean** (best path); TLS small-payload sync mode **+18–37%**; native sync API (picows is async-only); idle-connection memory **9 KB vs 50 KB (5–6× lighter)**.
> - **picows wins**: single-RT RR latency by **3–5 µs at 16 KB+ payloads** (Cython direct CPython API vs PyO3 method dispatch); at 256 B–4 KB the two are essentially tied.

## 📊 Performance

### Request-Response Throughput — plain TCP (RPS, 10 s, picows-parity methodology)

Neutral Rust echo server (tokio-tungstenite), pinned cores, uvloop, 1 s discarded pre-pass per cell:

| Payload | **ws-rs sync** | **ws-rs async** | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **15.0k** | 12.8k | 13.5k | 12.0k | 9.9k | 11.7k |
| 8 KB  | **15.1k** | 13.4k | 13.2k | 11.8k | 9.6k | 10.1k |
| 100 KB | **10.8k** | 10.3k | 10.5k | 9.4k | 8.1k | 4.5k |
| 1 MB  | 4.0k | **4.2k** | **4.2k** | 3.4k | 3.0k | 509 |

ws-rs wins or ties **12/12 cells** across three server architectures (tokio-tungstenite, fastwebsockets, picows-server).

### Request-Response Throughput — TLS / wss:// (rustls)

| Payload | **ws-rs sync** | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **13.2k** | 9.3k | 9.6k | 9.3k | 8.0k | 9.7k |
| 8 KB  | **11.9k** | 8.9k | 8.8k | 8.3k | 7.2k | 8.5k |
| 100 KB | **7.1k** | 5.9k | 6.0k | 5.8k | 5.3k | 3.8k |
| 1 MB  | 1.4k | 1.4k | **1.5k** | 1.4k | 1.4k | 461 |

Sync wins TLS 256 B–100 KB by 30–60%. At 1 MB picows leads by ~7%; ws-rs ties websockets/aiohttp. (1 MB is the realistic upper bound: Cloudflare's per-frame hard cap, Azure SignalR's default.)

### Memory Footprint

`~8.9 KB` per idle connection (was ~130 KB before lazy-alloc landed in v0.7.0). 10 K idle connections: **~90 MB** vs ~1.3 GB. Notification fan-out, IoT backends, long-poll receivers all benefit.

📊 **[Full benchmarks — all 3 servers, TCP & TLS, latency distributions, compression](docs/BENCHMARKS.md)** | 📝 **[Optimization Research](docs/OPTIMIZATION_RESEARCH.md)**

## ✨ What's New in v0.7.0

- **TLS backend: `native-tls` → `rustls`** (pure-Rust, no OpenSSL). Eliminates the OpenSSL global-state conflict that crashed picows when loaded in the same process. Cross-platform wheel builds simplified.
- **`asyncio.BufferedProtocol` + ring-buffer recv**: 64 KB pipelined +15% mean vs picows.
- **Lazy buffer allocation**: per-connection memory 130 KB → 8.9 KB (−93%).
- **`on_message` callback API** for users who want to bypass the per-frame Future overhead.
- **Scheme-dispatched protocol class**: `ws://` uses BufferedProtocol; `wss://` uses plain Protocol (asyncio's SSLProtocol interacts poorly with BufferedProtocol's small TLS-record callbacks).

Full details in [CHANGELOG.md](CHANGELOG.md).

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
- [picows](https://github.com/tarasko/picows) - High-performance Python WebSocket client. Special thanks to [@tarasko](https://github.com/tarasko) for [issue #11](https://github.com/coseto6125/websocket-rs/issues/11), which prompted the cross-validation benchmark methodology (multiple servers, controlled CPU pinning, statistically meaningful iteration counts) now used throughout `tests/benchmark_*.py`. The result table you see above wouldn't exist without that nudge.

## 📚 Further Reading

- [Why Rust async is fast](https://tokio.rs/tokio/tutorial)
- [PyO3 performance guide](https://pyo3.rs/main/doc/pyo3/performance)
- [WebSocket protocol RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455)
