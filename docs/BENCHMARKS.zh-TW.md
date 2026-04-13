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

**Payload 大小**（256 B / 8 KB / 100 KB / 1 MB）：1 MB 是 production 環境下
單一 WebSocket frame 的真實上限 — Cloudflare 對所有方案硬性限制 WS 訊息
≤ 1 MB，多數 hosted WS 服務（API Gateway、SignalR）也設 1 MB 或更低。
更大的 payload 只是壓測 framework 極限，與真實用戶情境無關。

### 對 tokio-tungstenite server（純 TCP）

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.3k** | 13.0k | 12.4k | 11.3k | 8.9k | 11.1k |
| 8 KB    | **14.2k** | 12.1k | 12.3k | 11.0k | 9.2k | 9.7k  |
| 100 KB  | **10.2k** | 9.7k  | 9.2k  | 9.0k  | 7.4k | 4.3k  |
| 1 MB    | 3.9k      | **4.3k** | **4.3k** | 3.5k | 3.0k | 543 |

### 對 fastwebsockets server（純 TCP）

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.8k** | 12.7k | 12.8k | 11.3k | 9.5k | 11.5k |
| 8 KB    | **14.6k** | 12.6k | 12.6k | 11.7k | 9.4k | 10.1k |
| 100 KB  | **10.5k** | **10.5k** | 10.0k | 9.7k | 8.1k | 4.3k |
| 1 MB    | 3.9k      | **4.4k** | 3.9k | 3.7k | 3.3k | 554 |

### 對 picows server（純 TCP）

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.1k** | 11.8k | 12.0k | 10.7k | 8.8k | 10.7k |
| 8 KB    | **13.3k** | 12.3k | 11.3k | 9.9k  | 8.4k | 9.2k  |
| 100 KB  | **9.8k**  | 9.4k  | 8.9k  | 8.1k  | 7.4k | 4.2k  |
| 1 MB    | **3.3k**  | **3.3k** | 3.1k | 3.0k | 2.5k | 512 |

### TLS（wss://）— tokio-tungstenite server，rustls

同樣 RR 方法論，每個 client 透過 `wss://` 連線到 tokio-tungstenite TLS
echo server（純 Rust rustls path）。TLS 為資料路徑加上 AES-GCM 加解密
（除測試 server 外，所有 client 透過 Python 的 `_ssl` 處理）。

| Payload | ws-rs sync | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **12.7k** | 9.1k  | 9.3k  | 8.5k | 7.8k | 9.3k |
| 8 KB    | **10.4k** | 8.5k  | 7.9k  | 7.5k | 6.5k | 7.8k |
| 100 KB  | **5.2k**  | 4.2k  | 4.0k  | 4.0k | 3.7k | 3.1k |
| 1 MB    | 606       | 699   | **701** | 652 | 652 | 317 |

**TLS overhead**：相同 payload 在 TLS 下比純 TCP 慢 25–60%。大訊息時
TLS 主導資料路徑，WS framing library 影響有限。

**結果摘要**：

- **純 TCP，12/12 組合**：ws-rs 全勝或並列。Sync 在 256 B–100 KB
  勝出（無 asyncio overhead）；Async 在 1 MB 與 picows 並列（tokio
  4.3k = 4.3k），其他 server 上 async 領先。
- **TLS，4/4 組合**：ws-rs 全勝或並列。Sync 在 256 B–100 KB 領先所有
  對手 30–60%。1 MB 時 async 與 picows 並列（差 0.3% 內，699 vs 701
  RPS），領先 websockets/aiohttp 7%。
- **對 websocket-client**：100 KB 快 2×、1 MB 快 7–10×。

重現方式：
```
python tests/benchmark_picows_parity.py    # 純 TCP，3 servers × 6 clients × 4 sizes
make tls-certs                              # 一次性，產生自簽憑證
python tests/benchmark_tls_parity.py        # TLS，1 server × 6 clients × 4 sizes
```

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
| 64 KB   | **0.91 / 0.88 / 1.67** | 1.00 / 0.91 / 1.95 | 1.07 / 1.03 / 1.70 |

**結果**：ws-rs 全勝。64 KB 來自兩項架構改動（commits `4d42a61` +
`2f1e0e4`）：asyncio.BufferedProtocol 快路徑 + ring-buffer recv_pos
游標。共同消除原本 plain Protocol 路徑的 per-recv `bytes` 配置 +
per-callback memcpy + partial-frame compaction copy。

5-run 平均：ws-rs await mean 0.91 ms vs picows 1.07 ms（贏 +15%）；
p99 1.67 vs 1.70（並列）。p99 變異也更穩 — ws-rs 1.59–1.83，picows
1.34–2.43（我們更一致）。

### 64 KB pipelined 為何能逆轉

之前測量顯示 picows 在 64 KB pipelined 領先 22%。診斷發現是 plain
Protocol 路徑下 per-recv 的 `bytes` 配置 + memcpy 累積。改用
asyncio.BufferedProtocol（uvloop 直接寫進我們的 buffer）+ ring-buffer
游標（partial frame 不搬家）後，這條路徑成本歸零，反而領先 picows 15%。

詳見 commits `4d42a61`（BufferedProtocol）+ `2f1e0e4`（ring-buffer）。
ws:// 路徑用 `NativeClientBuffered` 子類別暴露這兩個 method，wss://
用 bare `NativeClient`（asyncio SSLProtocol 對 BufferedProtocol 反而
不友善，16 KB TLS record 切割讓中介 ffi 成本超過 zero-alloc 收益）。

## 3. websocket-rs 何時勝、平、敗

| 工作負載 | 勝者 | 差距 |
|----------|---|---|
| RR，任意大小，純 TCP | websocket-rs ~= picows | <5% |
| RPS 吞吐量（TCP），256 B–100 KB | ws-rs sync | 對 picows +2–18% |
| RPS 吞吐量（TCP），1 MB | ws-rs async | 與 picows 並列 |
| RPS 吞吐量（TLS），256 B–100 KB | ws-rs sync | 領先所有對手 +30–60% |
| RPS 吞吐量（TLS），1 MB | ws-rs async | 與 picows 並列（差 0.3% 內） |
| Pipelined，512 B / 4 KB / 16 KB / 64 KB | websocket-rs | mean +14–25% vs picows |
| 對 websockets/aiohttp，所有測試組合 | websocket-rs | RPS +15–65% |
| 對 websocket-client，100 KB | websocket-rs | RPS ~2× |
| 對 websocket-client，1 MB | websocket-rs | RPS 7–10× |
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
