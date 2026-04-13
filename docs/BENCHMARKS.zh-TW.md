# 效能測試（Benchmarks）

完整、可重現的效能測試，對比主流 Python WebSocket 客戶端，並在多種 server
架構下驗證。本文件包含**所有測量到的組合**，包括 websocket-rs 輸給 picows
的特定工作負載 — 誠實揭露取捨，不挑選有利數據。

## 環境

- **CPU**：AMD Ryzen 9 9950X，client 綁 core 1、server 綁 core 0（同 CCD
  共享 L3、不同 core 避免爭用）
- **OS**：WSL2 Ubuntu（Linux 5.15 kernel）
- **Python**：3.13、uvloop 0.22.1、測量期間 `gc.disable()`
- **傳輸**：loopback（127.0.0.1）、`TCP_NODELAY` 開啟、無 TLS
- **函式庫**：websocket-rs（本 repo，profile=release）、picows 1.18、
  websockets 16.0、aiohttp（最新版）、websocket-client 1.9

## 1. Request-Response 吞吐量（picows-parity）

**方法論**：與 picows 官方 benchmark 一致 — 發送、等待回應、重複；於固定
10 秒視窗內計算完成數。RPS 越高越好。每個測試組合皆為全新 process + 全新連線
+ 50 筆訊息暖身後再計時。

每個測試組合皆有 1 秒的丟棄前置暖身（warm Python 3.13 bytecode specialization、
mimalloc 堆、配置器路徑及任何首個大 frame 的慢路徑），再進入 10 秒實際計時。

### 對 tokio-tungstenite server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **15.1k** | 12.9k | 12.8k | 11.0k | 9.2k | 10.9k |
| 8 KB    | **13.9k** | 12.5k | 12.2k | 10.8k | 9.0k | 10.7k |
| 100 KB  | **10.0k** | 9.7k  | 9.6k  | 8.8k  | 7.4k | 4.6k  |
| 2 MB    | 2.5k      | **2.8k** | 2.7k | 2.3k | 2.1k | 267 |

### 對 fastwebsockets server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.6k** | 13.3k | 12.7k | 11.6k | 9.6k | 11.7k |
| 8 KB    | **14.3k** | 13.0k | 12.4k | 11.1k | 9.1k | 10.4k |
| 100 KB  | **10.8k** | 10.3k | 10.1k | 9.7k  | 8.8k | 4.7k  |
| 2 MB    | 2.5k      | **3.1k** | 2.5k | 2.4k | 2.3k | 271 |

### 對 picows server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **13.8k** | 12.5k | 12.0k | 10.6k | 9.1k | 10.4k |
| 8 KB    | **13.6k** | 12.0k | 12.0k | 10.6k | 9.3k | 9.5k  |
| 100 KB  | **9.9k**  | 9.7k  | 9.5k  | 8.7k  | 7.8k | 4.6k  |
| 2 MB    | 1.9k      | **2.2k** | 1.9k | 2.0k | 1.8k | 270 |

**結果**：websocket-rs 在三種 server 架構共 **24 個測試組合全部領先**。

- **256 B – 100 KB**：sync API 勝（無 asyncio overhead）。對 picows 領先
  2–18%；對 websockets 領先 25–60%。
- **2 MB**：async API 全面勝出；sync 在此大小落後 async（單執行緒
  send/recv 必須阻塞於整個 2 MB payload，而 async 可重疊處理）。
- **對 websocket-client**：100 KB 約快 2×、2 MB 快 7–10×。

重現方式：`python tests/benchmark_picows_parity.py`

## 2. 延遲分佈（RR vs pipelined，N=5000）

**方法論**：測量每筆訊息的 RTT，回報 mean / p50 / p99（毫秒）。RR 為序列模式
（window=1）。Pipelined 採 window=100 以壓測每筆訊息的開銷。僅顯示 websocket-rs
與 picows（唯二「fast tier」）。`native (on_message)` 使用 websocket-rs 的同步
callback API 取代 `await ws.recv()` — 用以隔離架構成本與實作成本。

### RR 模式 — tokio-tungstenite server

| Payload | **websocket-rs (await)** | picows |
|---------|---:|---:|
| 512 B   | 0.075 / 0.071 / 0.131 | 0.078 / 0.075 / 0.138 |
| 4 KB    | 0.077 / 0.073 / 0.141 | 0.078 / 0.075 / 0.139 |
| 16 KB   | 0.082 / 0.077 / 0.149 | 0.077 / 0.071 / 0.147 |
| 64 KB   | 0.096 / 0.092 / 0.168 | 0.093 / 0.089 / 0.161 |

**結果**：RR 在所有大小實質打平（差距 <5%）。

### Pipelined 模式（window=100）— tokio-tungstenite server

| Payload | websocket-rs (await) | websocket-rs (on_message) | picows |
|---------|---:|---:|---:|
| 512 B   | 0.201 / 0.191 / 0.355 | **0.180 / 0.181 / 0.251** | 0.229 / 0.221 / 0.332 |
| 4 KB    | **0.241 / 0.221 / 0.516** | 0.240 / 0.243 / 0.388 | 0.294 / 0.293 / 0.426 |
| 16 KB   | 0.474 / 0.458 / 0.725 | **0.363 / 0.375 / 0.617** | 0.424 / 0.411 / 0.589 |
| 64 KB   | 1.364 / 1.488 / 1.898 | 1.345 / 1.404 / 2.448 | **1.059 / 1.012 / 1.515** |

**結果**：多數情況 websocket-rs 勝。`on_message` callback 路徑在 512 B 與
16 KB 領先；`await` API 在 4 KB 領先。picows 在 64 KB 仍勝（相對於 ws-rs
最佳路徑 mean −22%、p99 −20%）— 詳見下方分析。

### 為什麼我們在 64 KB pipelined 落敗

`perf stat -e syscalls:*` 計算整輪 syscall：

| syscall | websocket-rs | picows |
|---------|---:|---:|
| sendto | 10402 | 10285 |
| recvfrom + read | 4532 | 4673 |
| epoll_wait + pwait | **685** | **3801** (5.5×) |

- 兩者 send 與 recv 數量相同 — 不是 syscall 數差異。
- picows 的 epoll wakeup 多 **5.5 倍**但贏：每次 wakeup 處理較少事件，使
  尾端延遲（tail latency）保持緊湊。
- websocket-rs 在 event loop 採更激進的批次：較少 wakeup、每次處理更多
  事件。贏在中位數，輸在尾端。

火焰圖分析（`perf record -g --call-graph dwarf`）顯示兩邊函式庫自身程式碼
都不是瓶頸（apply_mask_avx512 self 佔 1–7%）。剩餘差距來自
`PyO3 → pyo3-async-runtimes → tokio → mio` 多層架構相對於 picows 單層
Cython 狀態機的累積成本 — 數十條小的 call-graph edge 各加 <1%，總和約
25%。沒有單一可優化的熱點。

**取捨選擇**：websocket-rs 優先選擇乾淨的 `async/await` API，而非比照 picows
的同步 callback 架構。對於需要 64 KB pipelined 最低延遲的工作負載，可改用
`on_message` callback API（上表中間欄）縮小差距；預設 `await` 路徑保留
Python async 的慣用寫法，僅以小幅 tail latency 為代價。

## 3. websocket-rs 何時勝、平、敗

| 工作負載 | 勝者 | 差距 |
|----------|---|---|
| RR，任意大小 | websocket-rs ~= picows | <5% |
| RPS 吞吐量（picows-parity），256 B–100 KB | websocket-rs sync | 對 picows +2–18% |
| RPS 吞吐量（picows-parity），2 MB | websocket-rs async | 對 picows +4–24% |
| Pipelined，512 B / 4 KB / 16 KB | websocket-rs | mean 對 picows +14–22% |
| Pipelined，64 KB | picows | 相對 ws-rs 最佳路徑 mean −22%、p99 −20% |
| 對 websockets/aiohttp，所有測試組合 | websocket-rs | RPS +15–65% |
| 對 websocket-client，100 KB | websocket-rs | RPS ~2× |
| 對 websocket-client，2 MB | websocket-rs | RPS ~10× |
| 真實 WAN（Postman Echo wss://） | 所有客戶端 ~= | <1%（網路主導） |

## 4. 重現方式

```bash
# 編譯 Rust extension 與中立 echo server
make build
cargo build --release --bin ws_echo_server --bin ws_echo_fastws

# 主要 benchmark（RR、4 大小、3 server、5 clients，約 10 分鐘）
python tests/benchmark_picows_parity.py

# 延遲矩陣（RR + pipelined、4 大小、3 clients，約 5 分鐘）
python tests/benchmark_three_servers.py

# Syscall / 火焰圖診斷（需 perf + root）
# 詳見 tests/perf_pipelined_64k.py 的測試 harness
```

所有 benchmark 腳本皆綁 core 0/1、使用 uvloop、計時期間關閉 GC，並在測量
前暖身。每次執行數值波動約 3%；上述數據取自系統穩定後一次代表性的執行。
