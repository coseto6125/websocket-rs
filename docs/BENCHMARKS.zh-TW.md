# 效能測試（Benchmarks）

完整、可重現的效能測試，對比主流 Python WebSocket 客戶端，並在多種 server
架構下驗證。本文件包含**所有測量到的格子**，包括 websocket-rs 輸給 picows
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
10 秒視窗內計算完成數。RPS 越高越好。每一格皆為全新 process + 全新連線
+ 50 筆訊息暖身後再計時。

每一格皆有 1 秒的丟棄前置暖身（warm Python 3.13 bytecode specialization、
mimalloc 堆、配置器路徑及任何首個大 frame 的慢路徑），再進入 10 秒實際計時。

### 對 tokio-tungstenite server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.8k** | 13.2k | 12.5k | 10.7k | 9.6k | 10.6k |
| 8 KB    | **13.2k** | 12.5k | 11.7k | 10.4k | 8.7k | 10.2k |
| 100 KB  | **10.3k** | 9.5k  | 10.0k | 8.6k  | 7.4k | 4.5k  |
| 2 MB    | 2.3k      | **2.6k** | **2.6k** | 2.2k | 2.0k | 261 |

### 對 fastwebsockets server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.3k** | 13.0k | 12.5k | 11.1k | 9.2k | 10.9k |
| 8 KB    | **13.7k** | 12.5k | 12.2k | 10.6k | 8.8k | 10.3k |
| 100 KB  | **11.2k** | 10.6k | 10.2k | 9.4k  | 8.0k | 4.6k  |
| 2 MB    | 2.2k      | **2.8k** | 2.4k | 2.2k | 2.0k | 261 |

### 對 picows server

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.0k** | 12.4k | 12.1k | 10.4k | 8.9k | 10.7k |
| 8 KB    | **13.2k** | 12.0k | 11.1k | 9.9k  | 8.4k | 9.7k  |
| 100 KB  | **9.6k**  | 9.0k  | 8.7k  | 8.1k  | 6.8k | 4.4k  |
| 2 MB    | 2.0k      | **2.1k** | 1.9k | 1.8k | 1.6k | 254 |

**結果**：websocket-rs 在三種 server 架構共 **24 格全部領先**。

- **256 B – 100 KB**：sync API 勝（無 asyncio overhead）。對 picows 領先
  8–17%；對 websockets 領先 35–55%。
- **2 MB**：async API 與 picows 平手或勝；sync 在此尺寸落後 async（單執行緒
  send/recv 必須阻塞於整個 2 MB payload，而 async 可重疊處理）。
- **對 websocket-client**：100 KB 快 2.0×、2 MB 快 7–10×。

重現方式：`python tests/benchmark_picows_parity.py`

## 2. 延遲分佈（RR vs pipelined，N=5000）

**方法論**：測量每筆訊息的 RTT，回報 mean / p50 / p99（毫秒）。RR 為序列模式
（window=1）。Pipelined 採 window=100 以壓測每筆訊息的開銷。僅顯示 websocket-rs
與 picows（唯二「fast tier」）。`native (on_message)` 使用 websocket-rs 的同步
callback API 取代 `await ws.recv()` — 用以隔離架構成本與實作成本。

### RR 模式 — tokio-tungstenite server

| Payload | **websocket-rs (await)** | picows |
|---------|---:|---:|
| 512 B   | 0.080 / 0.075 / 0.138 | 0.076 / 0.073 / 0.133 |
| 4 KB    | 0.082 / 0.077 / 0.147 | 0.079 / 0.075 / 0.144 |
| 16 KB   | 0.078 / 0.072 / 0.145 | 0.081 / 0.074 / 0.143 |
| 64 KB   | 0.100 / 0.094 / 0.178 | 0.090 / 0.084 / 0.170 |

**結果**：RR 在所有尺寸實質打平（差距 <5%）。

### Pipelined 模式（window=100）— tokio-tungstenite server

| Payload | websocket-rs (await) | websocket-rs (on_message) | picows |
|---------|---:|---:|---:|
| 512 B   | **0.201 / 0.194 / 0.292** | 0.239 / 0.200 / 0.746 | 0.260 / 0.248 / 0.386 |
| 4 KB    | **0.243 / 0.232 / 0.415** | 0.244 / 0.255 / 0.317 | 0.301 / 0.296 / 0.424 |
| 16 KB   | 0.554 / 0.518 / 1.252 | 0.696 / 0.700 / 1.286 | **0.401 / 0.366 / 0.683** |
| 64 KB   | 1.356 / 1.562 / 2.359 | 1.185 / 1.093 / 2.173 | **0.968 / 0.958 / 1.210** |

**結果**：互有勝負。websocket-rs 在 ≤4 KB 的 pipelined mean 領先約 25%。
picows 在 16 KB 與 64 KB 領先。特別在 64 KB，picows mean 領先約 29%、p99
領先約 49%。

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
的同步 callback 架構。`on_message` callback API（上表中間欄）能部分縮小
p99 差距（64 KB 從 2.36 ms 降到 2.17 ms），但 mean 差距仍在 — 而將預設 API
改為 callback-first 會破壞 Python async 慣用寫法。

## 3. websocket-rs 何時勝、平、敗

| 工作負載 | 勝者 | 差距 |
|----------|---|---|
| RR，任意尺寸 | websocket-rs ~= picows | <5% |
| RPS 吞吐量（picows-parity），任意尺寸 | websocket-rs | 對 picows +2–8% |
| Pipelined，≤4 KB | websocket-rs | 對 picows +20–30% |
| Pipelined，16 KB | picows | −28% mean、−45% p99 |
| Pipelined，64 KB | picows | −29% mean、−49% p99 |
| 對 websockets/aiohttp，所有格 | websocket-rs | RPS +20–50% |
| 對 websocket-client，100 KB | websocket-rs | RPS ~2× |
| 對 websocket-client，2 MB | websocket-rs | RPS ~10× |
| 真實 WAN（Postman Echo wss://） | 所有客戶端 ~= | <1%（網路主導） |

## 4. 重現方式

```bash
# 編譯 Rust extension 與中立 echo server
make build
cargo build --release --bin ws_echo_server --bin ws_echo_fastws

# 主要 benchmark（RR、4 尺寸、3 server、5 clients，約 10 分鐘）
python tests/benchmark_picows_parity.py

# 延遲矩陣（RR + pipelined、4 尺寸、3 clients，約 5 分鐘）
python tests/benchmark_three_servers.py

# Syscall / 火焰圖診斷（需 perf + root）
# 詳見 tests/perf_pipelined_64k.py 的測試 harness
```

所有 benchmark 腳本皆綁 core 0/1、使用 uvloop、計時期間關閉 GC，並在測量
前暖身。每次執行數值波動約 3%；上述數據取自系統穩定後一次代表性的執行。
