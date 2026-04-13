# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

Rust 實作的高效能 WebSocket 客戶端，提供 Python 綁定。支援同步和異步 API，並可選擇與 `websockets` 函式庫兼容。

## 🎯 效能概覽

### Request-Response 吞吐量 — 純 TCP（picows-parity benchmark，10 秒，RPS）

採用 picows 官方 benchmark 方法論 — RR 模式（send → wait → recv → 重複），中立 Rust echo server（tokio-tungstenite）、綁定 CPU core、uvloop，每組測試前有 1 秒丟棄前置暖身：

| Payload | **ws-rs sync** | **ws-rs async** | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **14.0k** | 12.4k | 12.7k | 11.7k | 9.5k | 11.6k |
| 8 KB    | **14.1k** | 13.2k | 12.3k | 10.2k | 8.8k | 9.5k  |
| 100 KB  | **10.1k** | 9.5k  | 9.7k  | 9.1k  | 7.7k | 4.3k  |
| 1 MB    | 3.9k      | **4.2k** | **4.2k** | 3.5k | 3.2k | 547 |

> ws-rs 在三種 server 架構共 **12/12 純 TCP 組合全部贏或並列**。Sync API 在 256 B–100 KB 勝出（無 asyncio overhead）；Async 在 1 MB 與 picows 並列。相較 picows 領先 2–18%；相較 websockets/aiohttp 領先 15–65%；相較 websocket-client 在 ≥100 KB 時快 2–10×。

### Request-Response 吞吐量 — TLS / wss://（rustls）

同樣 RR 方法論，但每個 client 透過 `wss://` 連線到 tokio-tungstenite TLS echo server（純 Rust rustls path），3 次取平均：

| Payload | **ws-rs sync** | ws-rs async | picows | aiohttp | websockets | websocket-client |
|---------|---:|---:|---:|---:|---:|---:|
| 256 B   | **13.4k** | 9.3k  | 9.4k  | 8.9k | 7.9k | 9.6k |
| 8 KB    | **11.6k** | 8.7k  | 8.8k  | 8.1k | 7.4k | 8.6k |
| 100 KB  | **7.2k**  | 6.0k  | 6.2k  | 5.6k | 5.1k | 3.7k |
| 1 MB    | 1.4k      | 1.6k  | **1.6k** | 1.5k | 1.5k | 470 |

> Sync 在 TLS 256 B–100 KB 領先所有對手 30–60%。1 MB 時 async 與 picows 並列（差 2% 內），領先 websockets/aiohttp 7%。1 MB 是 production WS 真實上限 — Cloudflare 硬上限、Azure SignalR 預設、遠超 AWS API Gateway WS 的 32 KB。

📊 **[完整 benchmarks — 所有 server、TCP 與 TLS、延遲分佈](docs/BENCHMARKS.md)** | 📝 **[最佳化研究](docs/OPTIMIZATION_RESEARCH.md)**

### 何時使用什麼

**Request-Response 模式**（聊天應用、API 呼叫、遊戲）：
- 🥇 **picows**: 0.056 ms RTT - 適合極致低延遲需求
- 🥈 **websocket-rs Sync**: 0.244 ms RTT - 效能與簡潔性的最佳平衡
- 🥉 **websocket-client**: 0.427 ms RTT - 適合簡單的同步應用

**高併發 Pipelined 模式**（資料串流、批次處理）：
- 🥇 **websocket-rs Async**: 3.656 ms RTT - 比 picows **快 7 倍**、比 websockets **快 18 倍**
- 🥈 **picows**: 25.821 ms RTT - 批次處理表現較差
- 🥉 **websockets Async**: 67.591 ms RTT - 純 Python 的限制

### 為什麼不同模式很重要

WebSocket 應用使用兩種根本不同的通訊模式：

1. **Request-Response (RR)**：發送一筆訊息 → 等待回應 → 發送下一筆
   - 應用場景：聊天應用、API 呼叫、線上遊戲、命令回應系統
   - 特性：序列化、阻塞、無併發
   - 最佳選擇：**picows**（事件驅動 C 擴展）

2. **Pipelined**：發送多筆訊息不等待 → 接收所有回應
   - 應用場景：資料串流、批次操作、高吞吐量系統
   - 特性：併發、非阻塞、批次 I/O
   - 最佳選擇：**websocket-rs Async**（Rust async + Tokio）

📊 **[查看詳細效能測試](docs/BENCHMARKS.md)** - 完整效能比較與測試方法

## ✨ v0.5.0 新功能

### SOCKS5 Proxy 支援
```python
ws = await connect("wss://example.com/ws", proxy="socks5://127.0.0.1:1080")
```

### 自訂 HTTP Headers
```python
ws = await connect("wss://example.com/ws", headers={"Authorization": "Bearer token"})
```

### 安全性與品質
- 修正非同步 error/timeout 路徑的 GIL 安全問題
- 建構時驗證 proxy scheme 與 header 值
- 向後相容 — `headers`/`proxy` 為 keyword-only 參數

## 🚀 快速開始

### 安裝

```bash
# 從 PyPI 安裝（推薦）
pip install websocket-rs

# 使用 uv
uv pip install websocket-rs

# 從原始碼安裝
pip install git+https://github.com/coseto6125/websocket-rs.git

# 使用 pip
pip install git+https://github.com/coseto6125/websocket-rs.git
```

### 基本用法

```python
# 直接使用 - 同步 API
from websocket_rs.sync_client import connect

with connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    response = ws.recv()
    print(response)
```

```python
# 直接使用 - 異步 API
import asyncio
from websocket_rs.async_client import connect

async def main():
    ws = await connect("ws://localhost:8765")
    try:
        await ws.send("Hello")
        response = await ws.recv()
        print(response)
    finally:
        await ws.close()

asyncio.run(main())
```

```python
# Monkeypatch 模式（零程式碼修改）
import websocket_rs
websocket_rs.enable_monkeypatch()

# 現有使用 websockets 的程式碼現在使用 Rust 實作
import websockets.sync.client
with websockets.sync.client.connect("ws://localhost:8765") as ws:
    ws.send("Hello")
    print(ws.recv())
```

## 📖 API 文件

### 標準 API（與 Python websockets 兼容）

| 方法 | 說明 | 範例 |
|------|------|------|
| `connect(url)` | 建立並連接 WebSocket | `ws = connect("ws://localhost:8765")` |
| `send(message)` | 發送訊息（str 或 bytes） | `ws.send("Hello")` |
| `recv()` | 接收訊息 | `msg = ws.recv()` |
| `close()` | 關閉連接 | `ws.close()` |

### 連接參數

```python
connect(
    url: str,                    # WebSocket 伺服器 URL
    connect_timeout: float = 30, # 連接逾時（秒）
    receive_timeout: float = 30  # 接收逾時（秒）
)
```

## 🎯 選擇正確的實作

### 選擇 **picows** 如果你需要：
- ✅ 絕對最低延遲（<0.1 ms）
- ✅ Request-response 模式（聊天、API 呼叫）
- ✅ 團隊熟悉事件驅動回呼
- ❌ 不適合：批次/pipelined 操作

### 選擇 **websocket-rs Sync** 如果你需要：
- ✅ 簡單的阻塞 API
- ✅ 良好效能（0.2-0.3 ms）
- ✅ `websockets.sync` 的直接替代品
- ✅ Request-response 模式
- ❌ 不適合：async/await 整合

### 選擇 **websocket-rs Async** 如果你需要：
- ✅ 高併發 pipelining
- ✅ 批次操作（比 picows 快 7 倍）
- ✅ 資料串流應用
- ✅ 與 Python asyncio 整合
- ❌ 不適合：簡單的 request-response（改用 Sync）

### 選擇 **websockets（Python）** 如果你需要：
- ✅ 快速原型開發
- ✅ 成熟生態系統
- ✅ 完整文件
- ✅ 低頻通訊（<10 msg/s）
- ❌ 不適合：高效能需求

## 🔧 進階安裝

### 從 GitHub Releases 安裝（預編譯 wheels）

```bash
# 指定版本（範例為 Linux x86_64, Python 3.12+）
uv pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.5.0/websocket_rs-0.5.0-cp312-abi3-manylinux_2_34_x86_64.whl
```

### 從原始碼編譯

**需求**：
- Python 3.12+
- Rust 1.70+（[rustup.rs](https://rustup.rs/)）

```bash
git clone https://github.com/coseto6125/websocket-rs.git
cd websocket-rs
pip install maturin
maturin develop --release
```

### 在 pyproject.toml 中使用

```toml
[project]
dependencies = [
    "websocket-rs @ git+https://github.com/coseto6125/websocket-rs.git@main",
]
```

## 🧪 執行測試和效能測試

```bash
# 執行 API 兼容性測試
python tests/test_compatibility.py

# 執行 picows-parity RPS benchmark（5 clients × 3 server 架構 × 4 大小，約 10 分鐘）
python tests/benchmark_picows_parity.py

# 執行延遲分佈 benchmark（RR + pipelined，mean/p50/p99）
python tests/benchmark_three_servers.py
```

## 🛠️ 開發

### 使用 uv 進行本地開發（推薦）

```bash
# 安裝 uv（如果尚未安裝）
curl -LsSf https://astral.sh/uv/install.sh | sh

# 設定開發環境
make install  # 建立 venv 並安裝依賴

# 建置和測試
make dev      # 開發模式建置
make test     # 執行測試
make bench    # 執行效能測試

# 或手動使用 uv
uv venv
source .venv/bin/activate
uv pip install -e ".[dev]"
maturin develop --release
```

### 傳統開發（pip）

```bash
# 安裝開發依賴
pip install maturin pytest websockets

# 開發模式（快速迭代）
maturin develop

# Release 模式（最佳效能）
maturin develop --release

# Watch 模式（自動重新編譯）
maturin develop --release --watch
```

## 📐 技術架構

### 為什麼用 Rust 實作 WebSocket？

1. **零成本抽象**：Rust 的 async/await 編譯為高效的狀態機
2. **Tokio runtime**：工作竊取排程器，針對 I/O 密集任務優化
3. **無 GIL**：併發操作的真正並行
4. **記憶體安全**：無 segfault、資料競爭或記憶體洩漏

### 效能權衡

**Request-Response 模式：**
- ❌ 每次呼叫都有 PyO3 FFI 開銷
- ❌ 雙 runtime 協調（asyncio + Tokio）
- ✅ 仍與純 Python sync 競爭
- ✅ 大訊息時優於 Python async

**Pipelined 模式：**
- ✅ FFI 開銷在批次中攤銷
- ✅ Tokio 的併發優勢發揮
- ✅ 無 GIL 阻塞
- ✅ 顯著快於所有 Python 替代方案

## 🐛 疑難排解

### 編譯問題

```bash
# 檢查 Rust 版本
rustc --version  # 需要 >= 1.70

# 清理並重新建置
cargo clean
maturin develop --release

# 詳細模式
maturin develop --release -v
```

### 執行時問題

- **TimeoutError**：增加 `connect_timeout` 參數
- **Module not found**：先執行 `maturin develop`
- **Connection refused**：檢查伺服器是否運行
- **效能不如預期**：確保使用 `--release` 建置

## 🤝 貢獻

歡迎貢獻！請確保：

1. 所有測試通過
2. API 兼容性維持
3. 包含效能測試
4. 更新文件

## 📄 授權

MIT License - 見 [LICENSE](LICENSE)

## 🙏 致謝

- [PyO3](https://github.com/PyO3/pyo3) - Rust Python 綁定
- [Tokio](https://tokio.rs/) - Async runtime
- [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) - WebSocket 實作
- [websockets](https://github.com/python-websockets/websockets) - Python WebSocket 函式庫
- [picows](https://github.com/tarasko/picows) - 高效能 Python WebSocket 客戶端。特別感謝 [@tarasko](https://github.com/tarasko) 在 [issue #11](https://github.com/coseto6125/websocket-rs/issues/11) 提出的建議，促成了本專案 `tests/benchmark_*.py` 採用的交叉驗證測試方法（多 server 架構、CPU 綁核、有統計意義的 iteration 數）。上方那張表沒有那個提醒就不會存在。

## 📚 延伸閱讀

- [為什麼 Rust async 快](https://tokio.rs/tokio/tutorial)
- [PyO3 效能指南](https://pyo3.rs/main/doc/pyo3/performance)
- [WebSocket 協議 RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455)
