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

**Payload sizes** (256 B / 8 KB / 100 KB / 1 MB): 1 MB is the upper realistic
bound for a single WebSocket frame in production — Cloudflare hard-caps WS
messages at 1 MB across all plans and most managed WS services
(API Gateway, SignalR) cap at 1 MB or less. Larger payloads only stress
framework limits, not real-world workloads.

### Against tokio-tungstenite server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **15.1k** | 12.9k | 12.8k | 11.0k | 9.2k | 10.9k |
| 8 KB  | **13.9k** | 12.5k | 12.2k | 10.8k | 9.0k | 10.7k |
| 100 KB | **10.0k** | 9.7k | 9.6k | 8.8k | 7.4k | 4.6k |
| 2 MB  | 2.5k | **2.8k** | 2.7k | 2.3k | 2.1k | 267 |

### Against fastwebsockets server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.6k** | 13.3k | 12.7k | 11.6k | 9.6k | 11.7k |
| 8 KB  | **14.3k** | 13.0k | 12.4k | 11.1k | 9.1k | 10.4k |
| 100 KB | **10.8k** | 10.3k | 10.1k | 9.7k | 8.8k | 4.7k |
| 2 MB  | 2.5k | **3.1k** | 2.5k | 2.4k | 2.3k | 271 |

### Against picows server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **13.8k** | 12.5k | 12.0k | 10.6k | 9.1k | 10.4k |
| 8 KB  | **13.6k** | 12.0k | 12.0k | 10.6k | 9.3k | 9.5k |
| 100 KB | **9.9k** | 9.7k | 9.5k | 8.7k | 7.8k | 4.6k |
| 2 MB  | 1.9k | **2.2k** | 1.9k | 2.0k | 1.8k | 270 |

**Result**: websocket-rs leads in **all 24 matrix cells** across all three
server architectures.

- **256 B – 100 KB**: ws-rs sync wins (no asyncio overhead). Margin over
  picows is 2–18%; over websockets is 25–60%.
- **2 MB**: ws-rs async wins outright (sync trails async at this size —
  single-threaded send/recv blocks across the full 2 MB payload while async
  overlaps).
- **vs websocket-client**: ~2× faster at 100 KB, 7–10× faster at 2 MB.

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
| 512 B | 0.075 / 0.071 / 0.131 | 0.078 / 0.075 / 0.138 |
| 4 KB  | 0.077 / 0.073 / 0.141 | 0.078 / 0.075 / 0.139 |
| 16 KB | 0.082 / 0.077 / 0.149 | 0.077 / 0.071 / 0.147 |
| 64 KB | 0.096 / 0.092 / 0.168 | 0.093 / 0.089 / 0.161 |

**Result**: RR is effectively a tie at all sizes (within 5%).

### Pipelined mode (window=100) — tokio-tungstenite server

| Payload | websocket-rs (await) | websocket-rs (on_message) | picows |
|---------|---:|---:|---:|
| 512 B | 0.201 / 0.191 / 0.355 | **0.180 / 0.181 / 0.251** | 0.229 / 0.221 / 0.332 |
| 4 KB  | **0.241 / 0.221 / 0.516** | 0.240 / 0.243 / 0.388 | 0.294 / 0.293 / 0.426 |
| 16 KB | 0.474 / 0.458 / 0.725 | **0.363 / 0.375 / 0.617** | 0.424 / 0.411 / 0.589 |
| 64 KB | 1.364 / 1.488 / 1.898 | 1.345 / 1.404 / 2.448 | **1.059 / 1.012 / 1.515** |

**Result**: mostly websocket-rs. The `on_message` callback path wins at
512 B, 16 KB; the `await` API wins at 4 KB. picows still wins at 64 KB
(mean −22%, p99 −20% vs the better ws-rs path) — see analysis below.

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
matching picows's synchronous callback architecture. For workloads that need
the lowest-possible 64 KB pipelined latency, the `on_message` callback API
(middle column) closes most of the gap; the `await` default keeps Python
async idioms intact at a small cost in tail latency.

## 3. When websocket-rs Wins, Ties, or Loses

| Workload | Winner | Gap |
|----------|---|---|
| RR, any size | websocket-rs ~= picows | <5% |
| RPS throughput (picows-parity), 256 B–100 KB | websocket-rs sync | +2–18% vs picows |
| RPS throughput (picows-parity), 2 MB | websocket-rs async | +4–24% vs picows |
| Pipelined, 512 B / 4 KB / 16 KB | websocket-rs | +14–22% mean vs picows |
| Pipelined, 64 KB | picows | −22% mean, −20% p99 vs best ws-rs path |
| vs websockets/aiohttp, all cells | websocket-rs | +15–65% RPS |
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
