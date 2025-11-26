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
