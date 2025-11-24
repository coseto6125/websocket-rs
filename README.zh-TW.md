# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Rust 實作的高效能 WebSocket 客戶端，提供 Python 綁定。支援同步和異步 API，並可選擇與 `websockets` 函式庫兼容。

**[English](README.md)** | **繁體中文**

## 🎯 效能概覽

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

## 📊 完整效能測試結果

**測試環境**：WSL2 Ubuntu、Python 3.13、localhost echo server、每次測試 1000 筆訊息

### Request-Response 模式（真實使用場景）

**小訊息（512 bytes）：**

| 實作 | 發送 (C→S) | 接收 (S→C) | RTT | 相對最快 |
|------|-----------|-----------|-----|---------|
| **picows (RR)** | 0.005 ms | 0.005 ms | **0.010 ms** | 🏆 1.0x |
| websocket-client | 0.065 ms | 0.057 ms | 0.123 ms | 慢 12.3 倍 |
| **websocket-rs Sync** | 0.057 ms | 0.075 ms | 0.133 ms | 慢 13.3 倍 |
| websockets (RR) | 0.082 ms | 0.081 ms | 0.165 ms | 慢 16.5 倍 |
| **websocket-rs (RR)** | 0.098 ms | 0.088 ms | 0.187 ms | 慢 18.7 倍 |
| websockets (Sync) | 0.084 ms | 0.109 ms | 0.194 ms | 慢 19.4 倍 |

**大訊息（65536 bytes）：**

| 實作 | 發送 (C→S) | 接收 (S→C) | RTT | 相對最快 |
|------|-----------|-----------|-----|---------|
| **picows (RR)** | 0.032 ms | 0.023 ms | **0.056 ms** | 🏆 1.0x |
| **websocket-rs Sync** | 0.143 ms | 0.100 ms | 0.244 ms | 慢 4.4 倍 |
| **websocket-rs (RR)** | 0.130 ms | 0.168 ms | 0.298 ms | 慢 5.3 倍 |
| websocket-client | 0.252 ms | 0.174 ms | 0.427 ms | 慢 7.6 倍 |
| websockets (RR) | 0.467 ms | 0.481 ms | 0.949 ms | 慢 17 倍 |
| websockets (Sync) | 0.464 ms | 0.546 ms | 1.011 ms | 慢 18 倍 |

**關鍵發現：**
- **picows 在 RR 模式無敵**：比其他方案快 4-18 倍
- **websocket-rs Sync**：最佳的 Rust 選擇（比 picows 慢 4.4 倍，但 API 更簡單）
- **websocket-rs Async 比 Python async 慢**：因為 PyO3 橋接 + 雙 runtime 的 13% 額外開銷

### Pipelined 模式（高併發）

**大訊息（65536 bytes，滑動窗口）：**

| 實作 | 發送 (C→S) | 接收 (S→C) | RTT | 相對最快 |
|------|-----------|-----------|-----|---------|
| **websocket-rs Async** | 2.846 ms | 0.810 ms | **3.656 ms** | 🏆 1.0x |
| picows | 25.444 ms | 0.377 ms | 25.821 ms | 慢 7.1 倍 |
| websockets (Async) | 32.609 ms | 34.981 ms | 67.591 ms | 慢 18.5 倍 |

**關鍵發現：**
- **websocket-rs Async 在 pipelined 模式稱霸**：比 picows 快 7 倍、比 websockets 快 18 倍
- **Rust async 在併發場景發光**：無 GIL、Tokio 排程器、零成本 futures
- **picows 在批次處理吃力**：事件驅動架構未針對 pipelined 發送優化

### 不同訊息大小的效能變化

**Request-Response 模式：**

| 訊息大小 | picows (RR) | websocket-rs Sync | websocket-rs (RR) | websockets (RR) |
|---------|-------------|-------------------|-------------------|-----------------|
| 512 B | 0.010 ms | 0.133 ms | 0.187 ms | 0.165 ms |
| 1 KB | 0.010 ms | 0.142 ms | 0.190 ms | 0.161 ms |
| 2 KB | 0.011 ms | 0.141 ms | 0.195 ms | 0.185 ms |
| 4 KB | 0.011 ms | 0.144 ms | 0.201 ms | 0.213 ms |
| 8 KB | 0.012 ms | 0.145 ms | 0.198 ms | 0.258 ms |
| 16 KB | 0.014 ms | 0.224 ms | 0.281 ms | 0.332 ms |
| 32 KB | 0.040 ms | 0.212 ms | 0.261 ms | 0.639 ms |
| 64 KB | 0.056 ms | 0.244 ms | 0.298 ms | 0.949 ms |

**Pipelined 模式：**

| 訊息大小 | websocket-rs Async | picows | websockets (Async) |
|---------|-------------------|--------|-------------------|
| 512 B | 0.803 ms | 5.104 ms | 2.037 ms |
| 1 KB | 0.929 ms | 4.764 ms | 2.682 ms |
| 4 KB | 1.139 ms | 5.456 ms | 6.413 ms |
| 16 KB | 2.691 ms | 10.826 ms | 27.038 ms |
| 64 KB | 3.656 ms | 25.821 ms | 67.591 ms |

## 🤔 理解效能模式

### 為什麼 websocket-rs Async 在 RR 模式比 Python async 慢？

**websocket-rs (RR)**：0.187 ms vs **websockets (RR)**：0.165 ms（慢 13%）

這看似違反直覺，但原因是：

1. **PyO3 FFI 開銷**：每次 send/recv 都要跨越 Python/Rust 邊界
2. **雙 async runtime 成本**：Python asyncio + Tokio 同時運行
3. **RR 模式無併發優勢**：序列化操作無法利用 Rust async 的優勢
4. **純 Python async 已優化**：`websockets` 成熟、經過良好調校

### 為什麼 websocket-rs Async 在 Pipelined 模式稱霸？

**websocket-rs Async**：3.656 ms vs **picows**：25.821 ms（快 7 倍）

因為 Rust async 在併發場景表現出色：

1. **真正的並行**：無 GIL，可以重疊 send/receive 操作
2. **Tokio 的效率**：工作竊取排程器、零成本 futures
3. **批次系統調用**：可以合併多個 I/O 操作
4. **記憶體效率**：編譯期優化、無 GC 暫停

### 為什麼 picows 在 RR 最快但在 Pipelined 較慢？

**RR**：0.056 ms（最快）vs **Pipelined**：25.821 ms（比 Rust 慢 7 倍）

- **RR 模式**：事件驅動回呼架構每筆訊息開銷極小
- **Pipelined 模式**：Queue + async 協調開銷在批次處理時變得顯著
- **優化焦點**：picows 針對事件驅動模式優化，而非批次發送

## 🚀 快速開始

### 安裝

```bash
# 使用 uv（推薦）
uv pip install git+https://github.com/coseto6125/websocket-rs.git

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
# 指定版本
uv pip install https://github.com/coseto6125/websocket-rs/releases/download/v0.3.0/websocket_rs-0.3.0-cp312-abi3-linux_x86_64.whl
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

# 執行完整效能測試（RR + Pipelined）
python tests/benchmark_server_timestamp.py
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
- [picows](https://github.com/tarasko/picows) - 高效能 Python WebSocket 客戶端

## 📚 延伸閱讀

- [為什麼 Rust async 快](https://tokio.rs/tokio/tutorial)
- [PyO3 效能指南](https://pyo3.rs/main/doc/pyo3/performance)
- [WebSocket 協議 RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455)
