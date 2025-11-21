# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A high-performance WebSocket client implemented in Rust with Python bindings. Fully compatible with Python `websockets` API while providing significant performance improvements.

## 🚀 Quick Start

```bash
# Install
uv pip install git+https://github.com/coseto6125/websocket-rs.git

# Use in your code - Zero changes needed!
```

```python
import websocket_rs
websocket_rs.enable_monkeypatch()  # Enable acceleration

# Your existing WebSocket code runs 1.5-5x faster automatically!
import websockets.sync.client
with websockets.sync.client.connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
```

## Features

- 🔥 **1.5-5x faster than native Python** (single messages)
- 🚄 **10-17x performance boost for batch operations**
- 🔄 **100% API compatible** with Python `websockets` module
- 🎯 **Zero-code-change acceleration** via monkeypatch
- 🦀 Efficient async I/O using Rust + Tokio
- 🔒 Thread-safe with concurrent operation support
- 💾 Optimized memory usage (zero-copy optimizations)
- 🔧 Multiple patching strategies (global, context, decorator, environment variable)

## Performance Comparison

Based on actual test results (test scripts in `tests/` directory):

### Single Message Latency (RTT)

| Message Size | Python websockets | websocket-rs | Performance Gain |
|---------|------------------|--------------|------------|
| 1 KB    | 0.258 ms        | 0.155 ms     | **1.66x** |
| 8 KB    | 0.279 ms        | 0.141 ms     | **1.98x** |
| 32 KB   | 0.495 ms        | 0.141 ms     | **3.51x** |
| 64 KB   | 0.801 ms        | 0.161 ms     | **4.98x** |

### Batch Operations Throughput

| Batch Size | 1 KB | 8 KB | 32 KB | 64 KB |
|---------|------|------|-------|-------|
| Batch(10) | **8.88x** | **7.57x** | **9.92x** | **10.20x** |
| Batch(100) | **17.66x** | **11.13x** | **12.44x** | **13.76x** |

*Test environment: WSL2 Ubuntu, Python 3.13, local echo server*

## Installation

### Option 1: Using uv (Fastest & Simplest) 🚀

```bash
# Install latest version
uv pip install git+https://github.com/coseto6125/websocket-rs.git

# Or specify a version tag
uv pip install git+https://github.com/coseto6125/websocket-rs.git@v0.2.0

# Or specify a branch
uv pip install git+https://github.com/coseto6125/websocket-rs.git@main

# Add to pyproject.toml
uv add git+https://github.com/coseto6125/websocket-rs.git
```

### Option 2: Using Pre-built Wheels (from GitHub Releases)

```bash
# Install with uv directly from GitHub Release
uv pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.2.0/websocket_rs-0.2.0-cp312-abi3-linux_x86_64.whl

# Or using pip
pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.2.0/websocket_rs-0.2.0-cp312-abi3-linux_x86_64.whl

# Auto-select appropriate wheel (requires setup)
pip install websocket-rs --index-url https://github.com/coseto6125/websocket-rs/releases/download/latest/
```

### Option 3: From PyPI (if published)

```bash
# Using uv
uv pip install websocket-rs

# Using pip
pip install websocket-rs
```

### Option 4: Build from Source

#### Prerequisites
- Python 3.12+
- Rust 1.70+ (if not installed, visit [rustup.rs](https://rustup.rs/))

#### Auto-compile with pip/uv

```bash
# pip will automatically call maturin to compile
pip install git+https://github.com/coseto6125/websocket-rs.git

# Or using uv (faster)
uv pip install git+https://github.com/coseto6125/websocket-rs.git
```

#### Manual Compilation

```bash
# 1. Clone repository
git clone https://github.com/coseto6125/websocket-rs.git
cd websocket-rs

# 2. Build with maturin
pip install maturin
maturin develop --release

# Or build wheel
maturin build --release
pip install target/wheels/*.whl
```

### Option 5: Development Environment Setup

```bash
# Using uv (recommended)
git clone https://github.com/coseto6125/websocket-rs.git
cd websocket-rs
uv venv
source .venv/bin/activate  # Linux/macOS
uv pip install maturin
maturin develop --release

# Or using traditional pip
python -m venv .venv
source .venv/bin/activate
pip install maturin
maturin develop --release
```

### Using in pyproject.toml

```toml
[project]
dependencies = [
    # Install from GitHub
    "websocket-rs @ git+https://github.com/coseto6125/websocket-rs.git@main",
    # Or from pre-built wheel URL
    "websocket-rs @ https://github.com/coseto6125/websocket-rs/releases/download/v0.2.0/websocket_rs-0.2.0-cp312-abi3-linux_x86_64.whl",
]

# Using uv
[tool.uv]
dependencies = [
    "websocket-rs @ git+https://github.com/coseto6125/websocket-rs.git",
]
```

## Usage

### 🎯 Monkeypatch - Zero Code Changes Required!

The most powerful feature of websocket-rs is the ability to accelerate existing WebSocket code without any modifications. Just import and enable monkeypatch, and all WebSocket operations automatically use our Rust implementation:

```python
# Add this at the start of your application
import websocket_rs
websocket_rs.enable_monkeypatch()

# Now ALL WebSocket code uses websocket-rs automatically!
import websockets.sync.client

# This uses websocket-rs under the hood - no code changes needed!
with websockets.sync.client.connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
    # Enjoying 1.5-5x speedup with zero code changes!
```

#### Why Use Monkeypatch?

- **Zero Migration Cost**: No need to rewrite existing code
- **Instant Performance**: Get 1.5-5x speedup immediately
- **Risk-Free Testing**: Easy to enable/disable for A/B testing
- **Library Compatible**: Works with any code using `websockets` or `websocket-client`
- **Production Ready**: Can be enabled via environment variable for different deployments

#### Monkeypatch Methods

1. **Global Enable (Recommended)**
```python
import websocket_rs
websocket_rs.enable_monkeypatch()  # Enable for entire application
```

2. **Environment Variable**
```bash
export WEBSOCKET_RS_AUTO_PATCH=1
python your_app.py  # Automatically uses websocket-rs
```

3. **Context Manager (Temporary)**
```python
from websocket_rs.patch import patch_context

with patch_context():
    # WebSocket libraries use websocket-rs here
    import websockets.sync.client
    ws = websockets.sync.client.connect(url)

# Original implementation restored outside
```

4. **Decorator (Function-level)**
```python
from websocket_rs.auto_patch import use_websocket_rs

@use_websocket_rs
def my_function():
    # This function uses websocket-rs for all WebSocket operations
    import websockets.sync.client
    with websockets.sync.client.connect(url) as ws:
        ws.send("Fast!")
```

5. **Manual Patching**
```python
from websocket_rs.patch import patch_all, unpatch_all

patch_all()  # Apply patches
# ... your code ...
unpatch_all()  # Restore originals
```

### Basic Usage (Same as Python websockets)

```python
from websocket_rs import WebSocket

# Synchronous usage
ws = WebSocket("ws://localhost:8765")
ws.connect()
ws.send("Hello, World!")
response = ws.recv()
print(response)
ws.close()

# Using Context Manager
with WebSocket("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
```

### Batch Operations (High-Performance Extension)

```python
from websocket_rs import WebSocket

ws = WebSocket("ws://localhost:8765")
ws.connect()

# Batch send - 10-17x faster than sending one by one
messages = ["msg1", "msg2", "msg3", "msg4", "msg5"]
ws.send_batch(messages)

# Batch receive
responses = ws.receive_batch(5)
print(responses)

ws.close()
```

### As Drop-in Replacement

```python
# Original code
# from websockets.sync.client import connect

# Change to
from websocket_rs import WebSocket as connect

# Rest of the code remains unchanged
with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    response = ws.recv()
```

### Graceful Fallback Strategy

```python
# Use high-performance version if available, fallback to native
try:
    from websocket_rs import WebSocket
    use_rust = True
except ImportError:
    from websockets.sync.client import connect as WebSocket
    use_rust = False
    print("Using fallback Python implementation")

# Code remains the same
ws = WebSocket("ws://localhost:8765")
if use_rust:
    ws.connect()  # Rust version requires explicit connect
# ... rest of the code is identical
```

## API Documentation

### Standard API (Compatible with Python websockets)

| Method | Description | Example |
|------|------|------|
| `WebSocket(url)` | Create WebSocket client | `ws = WebSocket("ws://localhost:8765")` |
| `connect()` | Establish connection | `ws.connect()` |
| `send(message)` | Send message (str or bytes) | `ws.send("Hello")` |
| `recv()` | Receive message | `msg = ws.recv()` |
| `receive()` | Alias for recv | `msg = ws.receive()` |
| `close()` | Close connection | `ws.close()` |
| `is_connected` | Check connection status (property) | `if ws.is_connected:` |

### Extended API (Performance Optimizations)

| Method | Description | Example |
|------|------|------|
| `send_batch(messages)` | Send multiple messages in batch | `ws.send_batch(["msg1", "msg2"])` |
| `receive_batch(count)` | Receive specified number of messages | `msgs = ws.receive_batch(10)` |

### Parameters

```python
WebSocket(
    url: str,                    # WebSocket server URL
    connect_timeout: float = 30, # Connection timeout (seconds)
    receive_timeout: float = 30  # Receive timeout (seconds)
)
```

## Running Tests and Benchmarks

```bash
# Run API compatibility tests
python tests/test_compatibility.py

# Run performance benchmarks
python tests/benchmark_optimized.py

# Run latency tests
python tests/benchmark_latency.py
```

## Development

### Project Structure

```
websocket-rs/
├── src/
│   └── lib.rs              # Rust implementation
├── tests/
│   ├── benchmark_optimized.py  # Performance tests
│   ├── benchmark_latency.py    # Latency tests
│   └── test_compatibility.py   # API compatibility tests
├── Cargo.toml              # Rust dependencies
├── pyproject.toml          # Python packaging config
└── README.md
```

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

# Development mode compilation (fast iteration)
maturin develop

# Release mode compilation (best performance)
maturin develop --release

# Watch mode (auto-recompile on file changes)
maturin develop --release --watch
```

### Building for Different Python Versions

```bash
# Specify Python interpreter
maturin build --release --interpreter python3.12
maturin build --release --interpreter python3.13
maturin build --release --interpreter python3.14

# Build universal wheel (abi3)
maturin build --release --abi3-py312
```

## Performance Optimization Principles

1. **Rust + Tokio**: Zero-cost abstractions + efficient async runtime
2. **Reduced FFI Overhead**: Batch operations minimize cross-language calls
3. **Memory Optimization**: Copy-on-Write reduces copying
4. **RwLock**: Allows multiple concurrent reads
5. **Dedicated Thread Pool**: Doesn't block Python main thread

## Use Cases

### ✅ Recommended Scenarios
- High-frequency trading systems
- Real-time data stream processing
- Large message transmission (>8KB)
- Batch message processing
- Low-latency applications

### ⚠️ May Not Be Necessary
- Simple low-frequency communication
- Small messages with very low frequency
- Prototype development phase

## Troubleshooting

### Compilation Issues

```bash
# Check Rust version
rustc --version  # Requires >= 1.70

# Clean and rebuild
cargo clean
maturin develop --release

# Use verbose mode for details
maturin develop --release -v
```

### Runtime Issues

- **TimeoutError**: Increase `connect_timeout` parameter
- **Module not found**: Ensure `maturin develop` has been executed
- **Connection refused**: Check if server is running

## Contributing

Contributions welcome! Please ensure:

1. All tests pass
2. API compatibility is maintained
3. Performance is verified with benchmarks
4. Documentation is updated

## License

MIT License - See [LICENSE](LICENSE)

## Acknowledgments

- [PyO3](https://github.com/PyO3/pyo3) - Rust Python bindings
- [Tokio](https://tokio.rs/) - Async runtime
- [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) - WebSocket implementation