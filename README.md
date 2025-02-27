# WebSocket-RS

這是一個高性能的 WebSocket 客戶端庫，使用 Rust 實現並提供 Python 綁定。

A high-performance WebSocket client library implemented in Rust with Python bindings.

## 功能特點 | Features

- 使用 Rust 實現核心功能，提供高性能的 WebSocket 通訊
- 支援 Python 的異步操作（asyncio）
- 提供簡潔的上下文管理器介面
- 支援文字和二進制訊息
- 內建錯誤處理和連線狀態管理
- 支援 TLS 加密連線（wss://）
- 連線超時設定
- 接收超時設定
- 自動重連功能
- 重連次數限制

---

- Core functionality implemented in Rust for high-performance WebSocket communication
- Support for Python's asynchronous operations (asyncio)
- Clean context manager interface
- Support for text and binary messages
- Built-in error handling and connection state management
- Support for TLS encrypted connections (wss://)
- Connection timeout settings
- Receive timeout settings
- Automatic reconnection
- Reconnection attempt limits

## 安裝 | Installation

```bash
pip install websocket-rs
```

## 使用方法 | Usage

### 基本使用 | Basic Usage

```python
import asyncio
from websocket_rs import WebSocket

async def main():
    # 使用異步上下文管理器建立連線
    # Use async context manager to establish connection
    async with WebSocket("wss://your-websocket-server") as ws:
        # 發送訊息
        # Send message
        await ws.send("你好，WebSocket！")
        
        # 接收訊息
        # Receive message
        response = await ws.receive()
        print(f"收到回應：{response}")

if __name__ == "__main__":
    asyncio.run(main())
```

### 使用進階功能 | Advanced Features

```python
import asyncio
from websocket_rs import WebSocket

async def main():
    # 使用所有進階參數
    # Use all advanced parameters
    async with WebSocket(
        "wss://your-websocket-server",
        connect_timeout=5.0,      # 5 秒連線超時 | 5 second connection timeout
        receive_timeout=10.0,     # 10 秒接收超時 | 10 second receive timeout
        retry=True,               # 啟用自動重試 | Enable automatic retry
        retry_limit=3             # 最多重試 3 次 | Maximum 3 retry attempts
    ) as ws:
        print(f"已連線，重試次數：{ws.retry_count}")
        
        # 發送訊息
        # Send message
        await ws.send("測試訊息")
        
        # 接收訊息（最多等待 10 秒）
        # Receive message (wait up to 10 seconds)
        response = await ws.receive()
        print(f"收到回應：{response}")

if __name__ == "__main__":
    asyncio.run(main())
```

### 手動重連 | Manual Reconnection

```python
import asyncio
from websocket_rs import WebSocket

async def main():
    ws = WebSocket("wss://your-websocket-server", retry=True, retry_limit=3)
    
    # 首次連接
    # First connection
    async with ws:
        await ws.send("測試1")
        response = await ws.receive()
        print(f"收到回應：{response}")
    
    # 此時連接已關閉
    # Connection is now closed
    print(f"連接狀態：{'已連接' if ws.is_connected else '已斷開'}")
    
    # 手動重連
    # Manual reconnection
    await ws.reconnect()
    print(f"重連後狀態：{'已連接' if ws.is_connected else '已斷開'}")
    print(f"重試次數：{ws.retry_count}")
    
    # 測試重連後的功能
    # Test functionality after reconnection
    await ws.send("測試2")
    response = await ws.receive()
    print(f"重連後收到回應：{response}")

if __name__ == "__main__":
    asyncio.run(main())
```

### 錯誤處理 | Error Handling

```python
from websocket_rs import WebSocket
from asyncio import run

async def main():
    try:
        # 設定較短的連線超時，以便快速失敗
        # Set a short connection timeout for quick failure
        async with WebSocket("wss://invalid-server", connect_timeout=2.0) as ws:
            await ws.send("測試訊息")
    except Exception as e:
        print(f"發生錯誤：{e}")
        
    try:
        # 設定接收超時
        # Set receive timeout
        async with WebSocket("wss://valid-server", receive_timeout=1.0) as ws:
            # 不發送任何訊息，直接接收，預期會超時
            # Don't send any message, directly receive, expect timeout
            await ws.receive()
    except Exception as e:
        print(f"接收超時：{e}")

if __name__ == "__main__":
    run(main())
```

## 參數說明 | Parameters

`WebSocket` 類別接受以下參數：

- `url`：WebSocket 服務器 URL（必須）
- `connect_timeout`：連線超時時間（秒），預設無限制
- `receive_timeout`：接收訊息超時時間（秒），預設無限制
- `retry`：是否自動重試連線，預設 False
- `retry_limit`：重試次數上限，預設無限制

The `WebSocket` class accepts the following parameters:

- `url`: WebSocket server URL (required)
- `connect_timeout`: Connection timeout in seconds, default is unlimited
- `receive_timeout`: Message receive timeout in seconds, default is unlimited
- `retry`: Whether to automatically retry connection, default is False
- `retry_limit`: Maximum number of retry attempts, default is unlimited

## 屬性 | Properties

- `is_connected`：連線狀態
- `retry_count`：重試次數

- `is_connected`: Connection status
- `retry_count`: Number of retry attempts

## 方法 | Methods

- `send(message)`：發送訊息
- `receive()`：接收訊息
- `reconnect()`：手動重新連線

- `send(message)`: Send a message
- `receive()`: Receive a message
- `reconnect()`: Manually reconnect

## 技術細節 | Technical Details

- 使用 tokio-tungstenite 作為 WebSocket 實現
- 使用 PyO3 提供 Python 綁定
- 使用 pyo3-asyncio 實現異步支援
- 使用 tokio 作為異步運行時

- Uses tokio-tungstenite for WebSocket implementation
- Uses PyO3 for Python bindings
- Uses pyo3-asyncio for asynchronous support
- Uses tokio as the asynchronous runtime

## 開發 | Development

### 環境需求 | Requirements

- Rust 工具鏈|toolchain
- Python 3.7+
- maturin（用於建置|for building）

### 建置步驟 | Build Steps

1. 克隆儲存庫：
   ```bash
   git clone https://github.com/your-username/websocket-rs.git
   cd websocket-rs
   ```

2. 建立虛擬環境：
   ```bash
   python -m venv .venv
   source .venv/bin/activate  # Linux/macOS
   # 或
   .venv\Scripts\activate     # Windows
   ```

3. 安裝開發依賴：
   ```bash
   pip install maturin pytest pytest-asyncio
   ```

4. 建置並安裝：
   ```bash
   maturin develop
   ```

5. 運行測試：
   ```bash
   pytest
   ```

## 授權 | License

MIT License
