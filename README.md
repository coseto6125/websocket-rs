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
| asyncio app, ≥ 1 MB payloads | **ws-rs async** | +15–20% over picows (RR, two servers, 7-round median; v0.7.3 zero-copy recv) |
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
| 1 MB  | 4.0k | **4.2k** | 3.4k | 3.4k | 3.0k | 509 |

> The legacy multi-client table above predates picows 2.1.1 and the v0.7.3 zero-copy path; the async-vs-picows section below supersedes its `ws-rs async` / `picows` columns. Its 1 MB `picows` cell is corrected here (was tied at 4.2k, now 3.4k on picows 2.1.1); the aiohttp / websockets / websocket-client columns are the older single-round figures pending a decision-grade re-measure.

ws-rs wins or ties **12/12 cells** across three server architectures (tokio-tungstenite, fastwebsockets, picows-server).

#### Apples-to-apples: async vs picows (v0.7.3, decision-grade)

Both are `asyncio.Protocol` peers, so this is the fair head-to-head. Measured against **picows 2.1.1** (latest) on an AMD Ryzen 9 9950X / WSL2, single-connection plain-TCP RR loopback, one discarded warm-up round + **7 interleaved scored rounds**, pooled median. Cross-checked on two independent servers so the result isn't tied to one server implementation:

| Payload | tokio-tungstenite server | picows-exact-echo server | verdict |
|---------|---:|---:|---|
| 256 B | 100.4% | ~parity | tie |
| 8 KB | 102.5% | 102.4% | +2.4% |
| 100 KB | 102.6% | 100.7% | ~parity–small win |
| 1 MB | **120.0%** | **115.5%** | **+15–20%** |
| **geomean** | **+6.1%** | **+5.2%** | ws-rs ahead |

The 1 MB win is the direct payoff of the v0.7.3 zero-copy receive path. Scope: these numbers are specific to this machine and a request-response workload — they do **not** cover TLS, pipelining, multiple concurrent connections, or streaming throughput.

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

## ✨ What's New in v0.7.3

- **Faster sync client**: both directions of the synchronous client drop their per-message payload copy — large-message throughput up 7–28% (plain 1 MiB recv +9.1%, TLS 1 MiB recv +27.9%, plain 1 MiB send +7.2%; medians on an idle host).
- **Signal-safe sync I/O**: blocking read/write now follow PEP 475 — signals during a blocking `recv()` no longer surface `Interrupted system call` errors, Ctrl-C still raises `KeyboardInterrupt`, and `receive_timeout` deadlines hold across retries.
- **Internals**: peer-close handling unified behind one seam with all transport effects outside internal borrows; the sync-send buffer owner's safety invariants are now compiler-enforced.

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

For the canonical async `websocket_rs.connect`, `connect_timeout` defaults to
10 seconds when omitted or set to `None`. `receive_timeout` defaults to `None`,
which waits indefinitely for each receive operation.

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

## ⚡ Event Loop Selection

`websocket-rs` uses the standard asyncio Protocol API and works with any asyncio-compatible event loop. Choice depends on your workload — measured on this repo (Linux, Ryzen 9950X, N=50000 messages, 512B payload, 3-run average):

| Workload | uvloop mean | rloop mean | Winner |
|---|---|---|---|
| Pipelined (single conn, window=100) | 0.366 ms | 0.365 ms | tie |
| RR (single conn, serial send/recv)  | 0.084 ms | **0.077 ms** | **rloop −8.0%** |
| on_message callback                 | 0.390 ms | **0.374 ms** | **rloop −4.2%** |
| 100 concurrent connections          | **8.21 ms** | 9.28 ms | **uvloop −13.1%** |

**Recommendation:**
- **Single connection, high-frequency** (trading feed, chat client) → `rloop`
- **Many concurrent connections** (scrapers, load generators, server-side) → `uvloop`
- **Default / mixed / cross-platform** → `uvloop` (mature, wider OS support)

```python
# rloop (Linux only)
import asyncio, rloop
asyncio.set_event_loop_policy(rloop.EventLoopPolicy())

# uvloop (Linux/macOS)
import uvloop
uvloop.install()
```

Reproduce the numbers: `uv run python tests/bench_rloop_vs_uvloop.py --runs 3`.

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
