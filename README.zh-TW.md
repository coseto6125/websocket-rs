# WebSocket-RS 🚀

[![Tests](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/test.yml)
[![Release](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml/badge.svg)](https://github.com/coseto6125/websocket-rs/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [繁體中文](README.zh-TW.md)

Rust 實作的高效能 WebSocket 客戶端，提供 Python 綁定。支援同步和異步 API，並可選擇與 `websockets` 函式庫兼容。

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

📊 **[查看詳細效能測試](docs/BENCHMARKS.md)** - 完整效能比較與測試方法

## ✨ v0.4.1 新功能

### 變更
- 修正所有套件檔案的版號同步
- 清理專案結構（移除過時的 python/ 目錄）
- 改進 .gitignore 規則提高精確度
- 將 Cargo.lock 加入版本控制
- 建立獨立的 BENCHMARKS.md 存放詳細效能資料

### v0.4.0 重點

**純同步客戶端** - 使用 `tungstenite`（非 async）重新實作：
- Request-Response RTT：**0.128 ms**（原為 0.244 ms，**快 1.9 倍**）
- 比 websocket-client **快 1.85 倍**
- 比 websockets **快 6.2 倍**

**架構設計**：
- 同步客戶端：純阻塞 I/O（簡單腳本、CLI 工具）
- 異步客戶端：Tokio runtime（高併發、事件驅動）

**向後相容**：
- 100% API 相容
- 無需修改程式碼

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
