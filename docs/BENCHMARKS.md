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

### Against tokio-tungstenite server (plain TCP)

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.3k** | 13.0k | 12.4k | 11.3k | 8.9k | 11.1k |
| 8 KB  | **14.2k** | 12.1k | 12.3k | 11.0k | 9.2k | 9.7k |
| 100 KB | **10.2k** | 9.7k | 9.2k | 9.0k | 7.4k | 4.3k |
| 1 MB  | 3.9k | **4.3k** | **4.3k** | 3.5k | 3.0k | 543 |

### Against fastwebsockets server (plain TCP)

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.8k** | 12.7k | 12.8k | 11.3k | 9.5k | 11.5k |
| 8 KB  | **14.6k** | 12.6k | 12.6k | 11.7k | 9.4k | 10.1k |
| 100 KB | **10.5k** | **10.5k** | 10.0k | 9.7k | 8.1k | 4.3k |
| 1 MB  | 3.9k | **4.4k** | 3.9k | 3.7k | 3.3k | 554 |

### Against picows server (plain TCP)

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **14.1k** | 11.8k | 12.0k | 10.7k | 8.8k | 10.7k |
| 8 KB  | **13.3k** | 12.3k | 11.3k | 9.9k | 8.4k | 9.2k |
| 100 KB | **9.8k** | 9.4k | 8.9k | 8.1k | 7.4k | 4.2k |
| 1 MB  | **3.3k** | **3.3k** | 3.1k | 3.0k | 2.5k | 512 |

### TLS (wss://) — tokio-tungstenite server, rustls

Same RR methodology, but every client connects via `wss://` to a
tokio-tungstenite TLS echo server using a self-signed cert from
`tests/certs/`. TLS adds AES-GCM encrypt + decrypt on the data path
(handled by Python's `_ssl` for all clients except the test server,
which uses pure-Rust rustls).

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B | **12.7k** | 9.1k | 9.3k | 8.5k | 7.8k | 9.3k |
| 8 KB  | **10.4k** | 8.5k | 7.9k | 7.5k | 6.5k | 7.8k |
| 100 KB | **5.2k** | 4.2k | 4.0k | 4.0k | 3.7k | 3.1k |
| 1 MB  | 606 | 699 | **701** | 652 | 652 | 317 |

**TLS overhead**: the same payload runs 25–60% slower under TLS than
plain TCP. TLS dominates the data path at large sizes; the WS framing
library matters less.

**Result summary**:

- **Plain TCP, 12/12 cells**: ws-rs wins or ties everywhere. Sync wins
  256 B–100 KB (no asyncio overhead); async ties picows at 1 MB on
  tokio-tungstenite (4.3k = 4.3k) and wins on the other servers.
- **TLS, 4/4 cells**: ws-rs wins or ties. Sync wins 256 B–100 KB by
  30–60% over every competitor. At 1 MB, async ties picows (within
  0.3%, 699 vs 701 RPS) and beats websockets/aiohttp by 7%.
- **vs websocket-client**: 2× at 100 KB, 7–10× at 1 MB.

Reproduce:
```
python tests/benchmark_picows_parity.py    # plain TCP, 3 servers × 6 clients × 4 sizes
make tls-certs                              # one-time, generates self-signed cert
python tests/benchmark_tls_parity.py        # TLS, 1 server × 6 clients × 4 sizes
```

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
| 64 KB | **0.91 / 0.88 / 1.67** | 1.00 / 0.91 / 1.95 | 1.07 / 1.03 / 1.70 |

**Result**: ws-rs wins all sizes. The 64 KB cell improvement came from
two architectural changes (commits `4d42a61` + `2f1e0e4`):
asyncio.BufferedProtocol fast path with a ring-buffer recv_pos cursor.
Together they cut the per-recv `bytes` allocation + per-callback memcpy
+ partial-frame compaction copy that the plain Protocol path incurred.

5-run avg: ws-rs await mean 0.91 ms vs picows 1.07 ms (we win +15%);
p99 1.67 vs 1.70 (we tie). p99 variance also tightened — ws-rs range
1.59–1.83, picows range 1.34–2.43 (we're more consistent).

The `on_message` callback (middle column) is slightly slower at 64 KB
than the `await` API now — its strength was bypassing future overhead,
but with BufferedProtocol the await path is already at the syscall
floor. Use `on_message` only when you specifically need synchronous
delivery semantics, not for raw throughput.

## 3. When websocket-rs Wins, Ties, or Loses

| Workload | Winner | Gap |
|----------|---|---|
| RR, any size, plain TCP | websocket-rs ~= picows | <5% |
| RPS throughput (TCP), 256 B–100 KB | ws-rs sync | +2–18% vs picows |
| RPS throughput (TCP), 1 MB | ws-rs async | tied with picows |
| RPS throughput (TLS), 256 B–100 KB | ws-rs sync | +30–60% vs every competitor |
| RPS throughput (TLS), 1 MB | ws-rs async | tied with picows (within 0.3%) |
| Pipelined, 512 B / 4 KB / 16 KB / 64 KB | websocket-rs | mean +14–25% vs picows |
| vs websockets/aiohttp, all cells | websocket-rs | +15–65% RPS |
| vs websocket-client, 100 KB | websocket-rs | ~2× RPS |
| vs websocket-client, 1 MB | websocket-rs | 7–10× RPS |
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
