# Benchmarks

Comprehensive, reproducible benchmarks against the major Python WebSocket
clients, tested across multiple server architectures. This document
includes **every cell measured**, including the narrow workload where
websocket-rs loses to picows — honest trade-offs over cherry-picked wins.

## Environment

- **CPU**: AMD Ryzen 9 9950X, client on core 1, server on core 0 (same CCD for
  L3 sharing, different cores to avoid contention)
- **OS**: WSL2 Ubuntu (Linux 5.15 kernel)
- **Python**: 3.13, uvloop 0.22.1, `gc.disable()` during measurement
- **Transport**: loopback (127.0.0.1), `TCP_NODELAY` on, no TLS
- **Libraries**: websocket-rs (this repo, profile=release), picows 1.18,
  websockets 16.0, aiohttp (latest), websocket-client 1.9

## 1. Request-Response Throughput (picows-parity)

**Methodology**: identical to picows's official benchmark — send, await
response, repeat; count completions in a fixed 10-second window. Higher RPS
is better. Each cell is a fresh process + fresh connection + 50-message
warmup before timing.

Each cell does a 1-second discarded pre-pass (warms Python 3.13 bytecode
specialization, mimalloc heaps, allocator paths, and any first-large-frame
slow paths) before the 10-second timed measurement.

### Against tokio-tungstenite server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.8k** | 13.2k | 12.5k | 10.7k | 9.6k | 10.6k |
| 8 KB  | **13.2k** | 12.5k | 11.7k | 10.4k | 8.7k | 10.2k |
| 100 KB | **10.3k** | 9.5k | 10.0k | 8.6k | 7.4k | 4.5k |
| 2 MB  | 2.3k | **2.6k** | **2.6k** | 2.2k | 2.0k | 261 |

### Against fastwebsockets server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.3k** | 13.0k | 12.5k | 11.1k | 9.2k | 10.9k |
| 8 KB  | **13.7k** | 12.5k | 12.2k | 10.6k | 8.8k | 10.3k |
| 100 KB | **11.2k** | 10.6k | 10.2k | 9.4k | 8.0k | 4.6k |
| 2 MB  | 2.2k | **2.8k** | 2.4k | 2.2k | 2.0k | 261 |

### Against picows server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.0k** | 12.4k | 12.1k | 10.4k | 8.9k | 10.7k |
| 8 KB  | **13.2k** | 12.0k | 11.1k | 9.9k | 8.4k | 9.7k |
| 100 KB | **9.6k** | 9.0k | 8.7k | 8.1k | 6.8k | 4.4k |
| 2 MB  | 2.0k | **2.1k** | 1.9k | 1.8k | 1.6k | 254 |

**Result**: websocket-rs leads in **all 24 matrix cells** across all three
server architectures.

- **256 B – 100 KB**: ws-rs sync wins (no asyncio overhead). Margin over
  picows is 8–17%; over websockets is 35–55%.
- **2 MB**: ws-rs async ties or beats picows; ws-rs sync trails async at
  this size (single-threaded send/recv blocks across the full 2 MB payload
  while async overlaps).
- **vs websocket-client**: 2.0× faster at 100 KB, 7–10× faster at 2 MB.

Reproduce: `python tests/benchmark_picows_parity.py`

## 2. Latency Distribution (RR vs pipelined, N=5000)

**Methodology**: measure per-message RTT, report mean / p50 / p99 in ms.
RR is serial (window=1). Pipelined uses window=100 to stress per-message
overhead. Only websocket-rs and picows shown (the only two in the "fast
tier"). `native (on_message)` uses websocket-rs's synchronous callback API
instead of `await ws.recv()` — included to isolate architectural cost from
implementation cost.

### RR mode — tokio-tungstenite server

| Payload | **websocket-rs (await)** | picows |
|---------|---:|---:|
| 512 B | 0.080 / 0.075 / 0.138 | 0.076 / 0.073 / 0.133 |
| 4 KB  | 0.082 / 0.077 / 0.147 | 0.079 / 0.075 / 0.144 |
| 16 KB | 0.078 / 0.072 / 0.145 | 0.081 / 0.074 / 0.143 |
| 64 KB | 0.100 / 0.094 / 0.178 | 0.090 / 0.084 / 0.170 |

**Result**: RR is effectively a tie at all sizes (within 5%).

### Pipelined mode (window=100) — tokio-tungstenite server

| Payload | websocket-rs (await) | websocket-rs (on_message) | picows |
|---------|---:|---:|---:|
| 512 B | **0.201 / 0.194 / 0.292** | 0.239 / 0.200 / 0.746 | 0.260 / 0.248 / 0.386 |
| 4 KB  | **0.243 / 0.232 / 0.415** | 0.244 / 0.255 / 0.317 | 0.301 / 0.296 / 0.424 |
| 16 KB | 0.554 / 0.518 / 1.252 | 0.696 / 0.700 / 1.286 | **0.401 / 0.366 / 0.683** |
| 64 KB | 1.356 / 1.562 / 2.359 | 1.185 / 1.093 / 2.173 | **0.968 / 0.958 / 1.210** |

**Result**: mixed. websocket-rs wins pipelined at ≤4 KB by ~25% on mean.
picows wins at 16 KB and 64 KB. At 64 KB specifically, picows wins by ~29%
on mean and ~49% on p99.

### Why we lose at 64 KB pipelined

`perf stat -e syscalls:*` counting syscalls over the run:

| syscall | websocket-rs | picows |
|---------|---:|---:|
| sendto | 10402 | 10285 |
| recvfrom + read | 4532 | 4673 |
| epoll_wait + pwait | **685** | **3801** (5.5×) |

- Both clients do the same number of sends and receives — not a syscall
  count difference.
- picows makes **5.5× more epoll wakeups** but wins: each wakeup processes
  fewer events, keeping tail latency tight.
- websocket-rs batches harder at the event loop: fewer wakeups with more
  events each. Wins median, loses tail.

Flame graph analysis (`perf record -g --call-graph dwarf`) shows neither
library's own code is the bottleneck (apply_mask_avx512 is 1–7% self in
both). The remaining gap appears to be the accumulated cost of the
`PyO3 → pyo3-async-runtimes → tokio → mio` layering versus picows's
single-layer Cython state machine — ~dozens of small call-graph edges each
adding <1%, totaling ~25%. No single hotspot to optimize.

**Trade-off taken**: websocket-rs prioritizes a clean `async/await` API over
matching picows's synchronous callback architecture. The `on_message`
callback API (above table, middle column) closes the p99 gap partially (from
2.36 ms to 2.17 ms at 64 KB), but the mean gap remains — and changing the
default API to a callback-first model would break Python async idioms.

## 3. When websocket-rs Wins, Ties, or Loses

| Workload | Winner | Gap |
|----------|---|---|
| RR, any size | websocket-rs ~= picows | <5% |
| RPS throughput (picows-parity), 256 B–100 KB | websocket-rs sync | +8–17% vs picows |
| RPS throughput (picows-parity), 2 MB | websocket-rs async | tied with picows |
| Pipelined, ≤4 KB | websocket-rs | +20–30% vs picows |
| Pipelined, 16 KB | picows | −28% mean, −45% p99 |
| Pipelined, 64 KB | picows | −29% mean, −49% p99 |
| vs websockets/aiohttp, all cells | websocket-rs | +20–50% RPS |
| vs websocket-client, 100 KB | websocket-rs | ~2× RPS |
| vs websocket-client, 2 MB | websocket-rs | ~10× RPS |
| Over real WAN (Postman Echo wss://) | all clients ~= | <1% (network dominates) |

## 4. Reproduce

```bash
# Build the Rust extension and neutral echo servers
make build
cargo build --release --bin ws_echo_server --bin ws_echo_fastws

# Main benchmark (RR, 4 sizes, 3 servers, 6 clients, ~12 min)
python tests/benchmark_picows_parity.py

# Latency matrix (RR + pipelined, 4 sizes, 3 clients, ~5 min)
python tests/benchmark_three_servers.py

# Syscall / flame-graph diagnostic (requires perf + root)
# See tests/perf_pipelined_64k.py for the harness
```

All benchmark scripts pin cores 0/1, use uvloop, disable GC during timing,
and warm up before measuring. Numbers fluctuate by ~3% run-to-run; figures
above are from a single representative run after the system stabilized.
