# WebSocket-RS 優化研究總結

## 研究目標

在 Actor Pattern 架構下,探索進一步的性能優化機會。

## 已完成的優化

### v0.3.0 優化成果

1. **Channel Buffer 優化**: 256 → 64
   - 提升: ~3%
   - 原因: 減少記憶體分配,更好的快取局部性

2. **mimalloc 記憶體分配器**
   - 提升: ~6%
   - 原因: 更高效的記憶體分配策略

**總提升**: 9% (0.244 ms → 0.222 ms)

### v0.3.1 優化成果（2025-11-25）

1. **ReadyFuture - 自定義 Future 實現**
   - 提升: ~22%
   - 原因: 繞過 asyncio.Future 創建開銷，直接返回已完成的 Future
   - 適用場景: Optimistic Send/Recv（Channel 非阻塞時）
   - 測試數據:
     - asyncio.Future: 5.604 ms (10,000 訊息)
     - ReadyFuture: 4.367 ms (10,000 訊息)
     - 節省: 1.24 ms / 10k msgs

2. **Optimistic `__anext__` - 異步迭代器快速路徑**
   - 提升: ~249x (Pipelined 場景)
   - 原因: 補完 `__anext__` 的 try_lock + try_recv 邏輯，避免每次都走 async 等待
   - 適用場景: 使用 `async for msg in ws` 的代碼
   - 測試數據:
     - 無 Optimistic 路徑: 1118.273 ms (10,000 訊息)
     - 有 Optimistic 路徑: 4.484 ms (10,000 訊息)
     - 節省: 1.11 秒 / 10k msgs

3. **ReadyFuture StopIteration 快取**
   - 提升: ~0.4%（微優化）
   - 原因: 快取 PyStopIteration 類型，避免重複查找
   - 測試數據:
     - 無快取: 4.367 ms (10,000 訊息)
     - 有快取: 4.350 ms (10,000 訊息)
     - 節省: 0.017 ms / 10k msgs

**v0.3.1 總提升**:
- Pipelined 模式（buffer 有數據）: **250x faster**（組合效果）
- Request-Response 模式: 與 v0.3.0 持平（~0.19 ms RTT）
- `async for` 現在與 `recv()` 性能相同
- 最終性能: ~4.35 ms / 10k msgs (0.435 µs/msg)

## 測試過的優化方案

### 1. #[inline] 標註
- **結果**: ❌ 慢 5.7%
- **原因**: Rust 編譯器的 inline 決策已經很優秀
- **結論**: 不採用

### 2. Cache Event Loop
- **結果**: ⚠️ 僅快 0.4%
- **原因**: Event loop 獲取本身開銷極小
- **結論**: 提升不明顯,不採用

### 3. 自定義 Tokio Runtime
- **結果**: ❌ 慢 3.6%
- **原因**: 預設配置已經針對通用場景優化
- **結論**: 不採用

### 4. Channel Buffer = 32
- **結果**: ❌ 全模式變慢
- **原因**: Buffer 過小導致背壓過早觸發
- **結論**: Buffer=64 是最佳值

### 5. Channel Buffer = 1
- **結果**: ❌ Pipelined 模式死鎖
- **原因**: 完全同步化,失去並發能力
- **結論**: 不可行

### 6. flume Channel
- **結果**: ❌ 慢 15% (0.213 → 0.244 ms)
- **原因**: tokio::mpsc 針對 tokio runtime 專門優化
- **結論**: 不採用

### 7. Batch API (send_batch)
- **測試結果**: ❌ **所有場景都更慢**
  - 完整 send+recv: 退步 53%
  - 數據顯示無論哪種測試方式都無提升

- **失敗原因**:
  1. **WebSocket 協議限制**: 本身就是逐一發送 frames,無法真正"批次"發送
  2. **Actor 實作限制**: 仍需逐一 await 每個 sink.send()
  3. **額外開銷**:
     - 創建 Vec<Command> 並包裝成 Command::Batch
     - Actor 端解包後再逐一處理
     - 這些開銷完全抵消了減少 Python→Rust 邊界跨越的收益
  4. **無並發優勢**: asyncio.gather 本身已經是高效並發

- **結論**: Batch API 是錯誤方向,Actor Pattern + WebSocket 協議的本質決定了無法從"批次發送"中獲益

### 8. Zero-Copy 分析
- **結果**: 已是最優
- **原因**: PyO3 已使用 `PyBytes::new_with` 避免複製
- **結論**: 無進一步優化空間

### 9. ReadyFuture（v0.3.1 已實現）✅
- **結果**: ✅ **快 22.1%**
- **原因**: 繞過 4 個 Python C API 調用（get_event_loop, create_future, set_result, call_soon_threadsafe）
- **實現**:
  ```rust
  #[pyclass]
  struct ReadyFuture {
      result: Option<Py<PyAny>>,
  }
  
  #[pymethods]
  impl ReadyFuture {
      fn __await__(slf: Py<Self>) -> Py<Self> { slf }
      fn __next__(&mut self, py: Python) -> PyResult<Py<PyAny>> {
          if let Some(res) = self.result.take() {
              let stop_iter = py.get_type::<pyo3::exceptions::PyStopIteration>();
              let err = stop_iter.call1((res,))?;
              Err(PyErr::from_value(err))
          } else {
              Err(pyo3::exceptions::PyStopIteration::new_err(()))
          }
      }
  }
  ```
- **A/B 測試數據**（10 次重複，10,000 訊息）:
  - asyncio.Future: Mean 5.604 ms, Median 5.429 ms, StdDev 0.466 ms
  - ReadyFuture: Mean 4.367 ms, Median 4.488 ms, StdDev 0.380 ms
  - 提升: 22.1%，穩定性提升 18.5%
- **結論**: ✅ 已採用，顯著提升且無副作用

### 10. Optimistic `__anext__`（v0.3.1 已實現）✅  
- **結果**: ✅ **快 249 倍**（Pipelined 場景）
- **原因**: `__anext__` 原本缺少 Optimistic 路徑，每次都需要 spawn Tokio task 並 async 等待
- **實現**: 將 `recv()` 的 `try_lock() + try_recv()` 邏輯複製到 `__anext__`
- **A/B 測試數據**（10 次重複，10,000 訊息，buffer 預填充）:
  - 無 Optimistic 路徑: Mean 1118.273 ms, Median 1114.904 ms
  - 有 Optimistic 路徑: Mean 4.484 ms, Median 4.372 ms
  - 提升: 249x (99.6% 延遲降低)
- **影響**: 使 `async for` 從「不可用」變成「與 recv() 同樣快」
- **結論**: ✅ 已採用，革命性改進

### 11. parking_lot::RwLock（v0.3.1 測試）❌
- **結果**: ❌ **慢 5.6%**
- **原因**: 
  - 我們的場景是低競爭（單連接，主要讀取操作）
  - parking_lot 適合高競爭場景，在低競爭時反而有額外開銷
  - std::sync::RwLock 在無競爭時更快
- **測試數據**（10 次重複，10,000 訊息，Optimistic Recv）:
  - std::sync::RwLock: Mean 4.367 ms
  - parking_lot::RwLock: Mean 4.610 ms
  - 退步: 5.6%
- **影響範圍**: 6 個 Arc<RwLock<...>> 欄位（stream_sync, local_addr, remote_addr, subprotocol, close_code, close_reason）
- **結論**: ❌ 不採用，保留 std::sync::RwLock

### 12. ReadyFuture StopIteration 快取（v0.3.1 已實現）⚠️
- **結果**: ⚠️ **快 0.4%**（微小提升）
- **原因**: 快取 `PyStopIteration` 類型，避免每次 `__next__` 都調用 `py.get_type()`
- **實現**:
  ```rust
  static STOP_ITERATION: OnceLock<Py<PyAny>> = OnceLock::new();
  
  fn get_stop_iteration(py: Python<'_>) -> &Py<PyAny> {
      STOP_ITERATION.get_or_init(|| {
          py.get_type::<pyo3::exceptions::PyStopIteration>().into_any().unbind()
      })
  }
  
  // 在 ReadyFuture.__next__ 中使用快取
  let stop_iter = get_stop_iteration(py).bind(py);
  ```
- **測試數據**（20 次重複，10,000 訊息）:
  - 無快取: Mean 4.367 ms
  - 有快取: Mean 4.350 ms, Trimmed Mean 4.301 ms
  - 提升: 0.4% (在誤差範圍內)
- **結論**: ⚠️ 已採用（代碼簡單，無副作用，但提升極微小）

## 架構替代方案分析

### 不使用 Actor Pattern 的選擇

| 方案 | 預估性能 | 複雜度 | 維護性 | 推薦度 |
|------|---------|--------|--------|--------|
| 直接 Mutex | ~0.17 ms | 低 | ⭐⭐⭐ | ⭐⭐⭐ |
| RwLock + Queue | ~0.20 ms | 中 | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ |
| Lockless (單執行緒) | ~0.14 ms | 高 | ⭐⭐ | ⭐ (PyO3 無法實現) |
| Split Architecture | ~0.21 ms | 低 | ⭐⭐⭐⭐ | ⭐⭐⭐ |

### 為何保留 Actor Pattern?

**Actor Pattern 提供的核心價值**:

1. **錯誤隔離**
   - Actor task 崩潰不影響主 Python 程式
   - 可以偵測並自動重啟
   - 狀態完全隔離

2. **背壓控制**
   - Channel buffer=64 自動限制記憶體
   - 防止快速發送導致 OOM
   - 內建流量控制

3. **清晰架構**
   - 訊息傳遞模式易於理解
   - 並發安全性由 Rust 保證
   - 易於維護和擴展

**移除 Actor Pattern 的代價**:
- 僅提升 23% (0.222 ms → ~0.17 ms)
- 失去錯誤隔離
- 失去背壓控制
- 需手動實現並發控制
- 維護成本大幅增加

**結論**: 性價比低,不建議移除

## 性能瓶頸分析

### PyO3 架構限制

當前架構 (0.222 ms) vs picows (0.124 ms) 的差距來源:

1. **Python↔Rust 邊界開銷** (~0.03 ms)
   - GIL acquire/release
   - PyO3 型別檢查和轉換
   - Future 創建和設置

2. **Actor Pattern 開銷** (~0.04 ms)
   - Channel send/recv
   - Task spawning
   - 訊息序列化

3. **架構差異** (~0.03 ms)
   - Tokio vs asyncio 整合
   - 多執行緒 vs 單執行緒

**PyO3 固有限制**:
```rust
// PyO3 要求所有 pyclass 必須 Send + Sync
#[pyclass]  // 強制 Send + Sync
struct Connection {
    // 無法使用 Rc<RefCell<T>> (lockless 方案)
}
```

**要達到 picows 性能需要**:
- 用 Cython 重寫 (完全不同技術棧)
- 或接受 PyO3 架構的性能trade-off

## 最終結論

### 當前狀態

#### v0.3.0
- **Async RR**: 0.222 ms
- **Pipelined (100 msgs)**: 5.715 ms
- **Sync**: 0.137 ms

#### v0.3.1（最新）
- **Async RR**: ~0.19 ms（與 v0.3.0 持平）
- **Pipelined Optimistic Recv**: 0.437 µs/msg（快 22%）
- **Async Iterator (async for)**: 0.448 µs/msg（快 249x，與 recv() 持平）
- **Sync**: 0.137 ms（不變）

### 優化成果

#### v0.3.0 優化
在 Actor Pattern 架構下:
- ✅ Channel buffer=64 優化
- ✅ mimalloc 分配器
- ✅ **總提升 9%**

#### v0.3.1 優化（新增）
進一步改進 Optimistic 路徑:
- ✅ **ReadyFuture**: 快 22%（Optimistic Send/Recv）
- ✅ **Optimistic `__anext__`**: 快 249x（Pipelined 場景，使 `async for` 實用化）
- ✅ **StopIteration 快取**: 快 0.4%（微優化，已採用）
- ❌ **parking_lot::RwLock**: 慢 5.6%（不適合低競爭場景，不採用）
- ✅ **組合效果**: Pipelined 模式下整體快 250x
- ✅ **已達到 Actor Pattern 架構 + PyO3 的實際性能極限**

### 不推薦的方向

1. **移除 Actor Pattern**
   - 提升有限 (23%)
   - 失去關鍵架構優勢
   - 維護成本過高

2. **Batch API**
   - WebSocket 協議限制
   - 無法真正批次發送
   - 實測性能退步

3. **用 Cython 重寫**
   - 完全不同技術棧
   - 失去 Rust 優勢
   - 投資報酬率低

### 推薦策略

#### v0.3.1 後的狀態

經過 ReadyFuture 和 Optimistic `__anext__` 優化後:
- ✅ **Pipelined 場景性能已達到極致**（250x 提升）
- ✅ **`async for` 現在是推薦的 API**（與 `recv()` 同樣快）
- ✅ **Request-Response 場景保持優秀**（~0.19 ms RTT）

#### 進一步優化空間（低優先級）

1. **Per-Connection Loop Cache**（待測試）
   - 快取每個連接的 event loop 實例
   - 預期提升: 1-2%
   - 風險: 低
   - 建議: 可作為 v0.3.2 候選

2. **接受當前性能**，專注於:
   - ✅ 功能穩定性
   - ✅ 錯誤處理完善性
   - ✅ 文檔和範例完整性
   - ✅ 生態系統整合

## 測試方法

### 基準測試

```python
import asyncio
import time
from websocket_rs.async_client import connect

async def benchmark():
    ws = await connect("ws://localhost:9001")

    # Request-Response 模式
    times = []
    for _ in range(1000):
        start = time.perf_counter()
        await ws.send(b"test")
        await ws.recv()
        times.append((time.perf_counter() - start) * 1000)

    print(f"平均: {sum(times)/len(times):.3f} ms")
    print(f"中位數: {sorted(times)[500]:.3f} ms")
    print(f"P95: {sorted(times)[950]:.3f} ms")

    await ws.close()

asyncio.run(benchmark())
```

### Pipelined 測試

```python
async def benchmark_pipelined():
    ws = await connect("ws://localhost:9001")
    batch_size = 100

    start = time.perf_counter()

    # 並發發送
    await asyncio.gather(*[ws.send(b"x" * 512) for _ in range(batch_size)])

    # 接收回應
    for _ in range(batch_size):
        await ws.recv()

    elapsed = (time.perf_counter() - start) * 1000
    print(f"Pipelined {batch_size}: {elapsed:.3f} ms")
```

## v0.3.1 最新優化成果（2025-11-25）

### 已實現的優化

1. **Per-Connection Event Loop Cache**
   - 快取每個連線的 event loop 實例
   - 減少重複的 `get_running_loop()` 調用
   - 提升: 1-2%（慢路徑場景）

2. **ReadyFuture 錯誤路徑優化**
   - 擴展 ReadyFuture 支援錯誤回傳
   - 繞過 asyncio 的 4 次 Python C API 調用
   - 錯誤處理速度: 10M+ errors/sec
   - 單次錯誤延遲: 0.1μs
   - 提升: 比 asyncio 快 ~50x

3. **修正 get_event_loop → get_running_loop**
   - 使用 Python 3.10+ 推薦的 API
   - 更安全的 event loop 獲取
   - 避免跨執行緒問題

### 性能測試結果

基於 1000 次訊息測試（1KB 訊息大小）：

| 測試場景 | 平均延遲 | 吞吐量 |
|---------|---------|--------|
| Request-Response | 232.64ms | 4,300 ops/s |
| Pipelined | 105.39ms | 9,500 ops/s |
| 錯誤路徑（10k 次） | 1.00ms | 10M errors/s |
| 混合場景（90/10） | 200.03ms | 5,000 ops/s |

### 總結

v0.3.1 改進：
- ✅ 錯誤處理性能提升 ~50x
- ✅ Event loop 查詢優化
- ✅ 穩定性改善（標準差 <1-2%）
- ✅ Actor Pattern + PyO3 架構下的性能優化完成

## v0.4.0 優化成果（2025-11-26）

### Pure Sync Client - 移除 Async Overhead

#### 問題分析

原有的 Sync Client 實作使用 Tokio runtime wrapper：
```rust
// 舊實作（v0.3.x）
fn recv(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
    py.allow_threads(|| {
        let rt = get_runtime();           // Tokio runtime
        rt.block_on(async {               // block_on 開銷
            let guard = stream.lock().await;  // AsyncMutex
            let msg = ws.next().await;    // async 操作
            // ...
        })
    })
}
```

**效能瓶頸**：
1. Tokio runtime overhead (~15 μs)
2. `block_on()` 調度開銷 (~5-10 μs)
3. AsyncMutex lock overhead
4. 強制透過 async 路徑，即使是 sync API

#### 解決方案

使用 `tungstenite`（非 async 版本）實作真正的 Pure Sync Client：

```rust
// 新實作（v0.4.0）
use tungstenite::{connect, Message, WebSocket};
use std::net::TcpStream;

fn recv(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
    py.allow_threads(|| {
        let ws = self.ws.as_mut()?;
        let msg = ws.read()?;  // 直接 blocking read，無 async
        match msg {
            Message::Binary(data) => Ok(data),
            // ...
        }
    })
    .map(|data| PyBytes::new(py, &data).into())
}
```

**架構變更**：
- ❌ 移除 Tokio runtime dependency（sync client）
- ❌ 移除 AsyncMutex
- ❌ 移除 `block_on()` 開銷
- ✅ 直接使用 `std::net::TcpStream`
- ✅ 真正的 blocking I/O

### 效能測試結果

基於 `tests/benchmark_server_timestamp.py`（1000 訊息，512 bytes）：

#### Sync Client 對比

| 實作 | 發送 (C→S) | 接收 (S→C) | RTT | vs baseline |
|-----|-----------|-----------|-----|-------------|
| websockets (Sync) | 0.087 ms | 0.113 ms | 0.201 ms | 0.67x |
| websocket-client | 0.072 ms | 0.062 ms | 0.135 ms | 1.00x (baseline) |
| **websocket-rs Sync (v0.3.x)** | 0.059 ms | 0.079 ms | 0.140 ms | 0.96x |
| **websocket-rs Sync (v0.4.0)** | **0.054 ms** | **0.048 ms** | **0.103 ms** | **1.31x** ⚡ |

**vs websocket-client**：
- 發送快 1.33x (0.054 vs 0.072 ms)
- 接收快 1.29x (0.048 vs 0.062 ms)
- RTT 快 1.31x (0.103 vs 0.135 ms)

**vs v0.3.x Sync**：
- 發送快 1.09x (0.054 vs 0.059 ms)
- 接收快 1.65x (0.048 vs 0.079 ms)
- RTT 快 1.36x (0.103 vs 0.140 ms)

#### 不同訊息大小的表現

| 訊息大小 | websocket-client RTT | websocket-rs v0.4.0 RTT | 提升 |
|---------|---------------------|------------------------|------|
| 512 B   | 0.135 ms           | 0.103 ms               | 1.31x |
| 1024 B  | 0.136 ms           | 0.102 ms               | 1.33x |
| 2048 B  | 0.138 ms           | 0.104 ms               | 1.33x |
| 4096 B  | 0.131 ms           | 0.104 ms               | 1.26x |
| 8192 B  | 0.140 ms           | -                      | -    |

**結論**：在所有訊息大小下都保持 **1.3x** 的效能優勢。

### 延遲分解分析

#### websocket-client (0.135 ms)
```
純 Python socket.recv()
  → 純 Python WebSocket 解析
  → 直接返回 bytes
```

#### websocket-rs v0.3.x Sync (0.140 ms)
```
總延遲: 140 μs
├─ Tokio runtime: ~15 μs (11%)
├─ block_on: ~5 μs (4%)
├─ AsyncMutex: ~5 μs (4%)
├─ WebSocket 協議: ~20 μs (14%)
├─ 系統呼叫: ~5 μs (4%)
├─ Rust → Python: ~0.1 μs (0.07%)
└─ 其他: ~90 μs (64%)
```

#### websocket-rs v0.4.0 Pure Sync (0.103 ms)
```
總延遲: 103 μs
├─ WebSocket 協議: ~20 μs (19%)
├─ 系統呼叫: ~5 μs (5%)
├─ Rust → Python: ~0.1 μs (0.1%)
└─ 其他: ~78 μs (76%)
```

**移除的開銷**：
- ❌ Tokio runtime: 15 μs
- ❌ block_on: 5 μs
- ❌ AsyncMutex: 5 μs
- **總節省**: ~25 μs (18%)

### 架構對比

#### v0.3.x Sync（Async-based）
```rust
tokio-tungstenite (async)
  → Tokio Runtime
  → block_on()
  → AsyncMutex
  → Python
```

**優點**：
- 與 async 生態系統兼容
- 可共享 Tokio runtime

**缺點**：
- 不必要的 async overhead
- 效能不如 pure sync

#### v0.4.0 Pure Sync
```rust
tungstenite (sync)
  → std::net::TcpStream
  → blocking I/O
  → Python
```

**優點**：
- ✅ 零 async overhead
- ✅ 最簡單的資料路徑
- ✅ 最佳 sync 效能

**缺點**：
- 無（sync 場景下）

### API 變更

#### 使用者視角（無變更）
```python
# v0.3.x 和 v0.4.0 API 完全相同
with websocket_rs.sync.client.connect(uri) as ws:
    ws.send(b"hello")
    data = ws.recv()
```

#### 實作變更
- **v0.3.x**: `tokio-tungstenite` + `AsyncMutex` + `block_on`
- **v0.4.0**: `tungstenite` + 直接 blocking I/O

**向後兼容**：100%

### 同時提供 Sync 和 Async

v0.4.0 同時擁有兩個最佳實作：

#### 1. Pure Sync Client (NEW)
```python
# 適用場景：簡單腳本、工具、阻塞式操作
with websocket_rs.sync.client.connect(uri) as ws:
    data = ws.recv()  # 最快的 sync 實作
```

**效能**：0.103 ms RTT (比 websocket-client 快 1.31x)

#### 2. Async Client (保留)
```python
# 適用場景：高併發、事件驅動、async/await
ws = await websocket_rs.async_client.connect(uri)
data = await ws.recv()  # 支援併發
```

**效能**：
- Request-Response: ~0.19 ms RTT
- Pipelined: ~0.44 μs/msg

### 技術細節

#### Dependencies 變更

```toml
# Cargo.toml
[dependencies]
# Async client
tokio-tungstenite = { version = "0.28", features = ["native-tls"] }
tokio = { version = "1", features = ["full"] }

# Sync client (NEW)
tungstenite = { version = "0.24", features = ["native-tls"] }
```

#### 實作差異

**連線建立**：
```rust
// Async
let (ws, _) = tokio_tungstenite::connect_async(url).await?;

// Sync (v0.4.0)
let (ws, _) = tungstenite::connect(url)?;
```

**訊息接收**：
```rust
// Async
let msg = ws.next().await;

// Sync (v0.4.0)
let msg = ws.read()?;
```

**超時處理**：
```rust
// Async
tokio::time::timeout(duration, ws.next()).await

// Sync (v0.4.0)
stream.set_read_timeout(Some(duration))?;
ws.read()  // 會自動超時
```

### 測試驗證

#### 功能測試
- ✅ Context manager (`with` statement)
- ✅ 迭代器 (`for msg in ws`)
- ✅ Ping/Pong
- ✅ 超時處理
- ✅ 錯誤處理
- ✅ 地址資訊獲取

#### 效能測試
- ✅ 比 websocket-client 快 1.31x
- ✅ 比 v0.3.x Sync 快 1.36x
- ✅ 所有訊息大小下表現穩定

### v0.4.0 總結

#### 主要成就

1. **Pure Sync Client 實作**
   - ✅ 移除 Tokio runtime overhead
   - ✅ 真正的 blocking I/O
   - ✅ 成為最快的 Python WebSocket sync 實作

2. **效能提升**
   - ✅ RTT 從 0.140 ms → 0.103 ms (26% 提升)
   - ✅ 比 websocket-client 快 1.31x
   - ✅ 比 websockets 快 1.95x

3. **API 穩定性**
   - ✅ 向後兼容 100%
   - ✅ 使用者無需修改程式碼
   - ✅ 功能完整性保持

#### 架構決策

**為何同時保留 Sync 和 Async？**

| 場景 | 推薦實作 | 理由 |
|-----|---------|------|
| 簡單腳本 | Sync | 最快、最簡單 |
| CLI 工具 | Sync | 無需 event loop |
| 高併發 | Async | 支援 thousands 連線 |
| 事件驅動 | Async | 與 asyncio 整合 |

#### 實作狀態

v0.4.0 完成後：
- Sync client: 使用 `tungstenite` (non-async),直接 blocking I/O
- Async client: 使用 `tokio-tungstenite`,支援併發與 pipelined 操作
- 兩種場景各有專門實作

## v0.5.0 優化研究（2026-03-25）

### 測試環境

- Python 3.13, websockets 15.0.1, rustc 1.94.0 (LLVM 20)
- WSL2 Linux 5.15, localhost echo server
- 測試方法: 500 次 send/recv roundtrip, 32 bytes payload, trimmed mean of 7 runs

### 已實施的微觀優化

1. **`pyo3::intern!` 快取 Python 字串查找**
   - 適用: `complete_future`, `fail_future`, `create_future`, `get_asyncio`, `get_cached_event_loop`
   - 原理: `intern!` 將字串比較從 hash lookup 降為 pointer equality
   - 影響: 每次 slow-path recv/send 少數次 `getattr`/`call_method` 調用加速

2. **Async `send` 改用 `cast` + `to_str`**
   - 替換: `message.extract::<String>()` → `message.cast::<PyString>().to_str()?.to_owned()`
   - 原理: 避免 `extract` 的額外 UTF-8 validation + copy
   - 影響: 每次 send 省一次完整 string copy

3. **Sync `recv` 零拷貝**
   - 替換: `text.to_string()` → 直接 move（`RecvResult` enum）
   - 原理: tungstenite 回傳的 `String`/`Vec<u8>` 直接 move,不做中間拷貝
   - 影響: 大訊息時顯著（16KB 時 Sync 快 3.2x vs websockets）

4. **Sync `send` 消除雙重分配**
   - 替換: `to_string_lossy().to_string()` → `to_str()?.to_owned()`
   - 原理: 避免 `Cow<str>` 中間分配

5. **Sync `recv` timeout 偵測改用 `ErrorKind`**
   - 替換: `e.to_string().contains("timed out")` → `matches!(io_err.kind(), TimedOut | WouldBlock)`
   - 原理: 避免 error path 的 string 格式化 + 子字串搜尋

6. **地址存儲改用 `(String, u16)` tuple**
   - 替換: `String`（如 `"127.0.0.1:8765"`）→ `(String, u16)`
   - 原理: getter 不再每次做 `rsplit_once` + `parse`

7. **背景 task 過濾 Ping/Pong**
   - 原理: Ping/Pong 不送進 channel,避免無意義的 channel 喚醒 + GIL 獲取

8. **Defer Arc::clone 到 slow path**
   - 原理: `recv`/`__anext__` 的 fast path 直接借用 `&self`,不 clone `close_code`/`close_reason` Arc

9. **`ping`/`pong` 改用 `ready_fast`**
   - 替換: `ready_ok`（3 次 Python 跨語言調用）→ `ready_fast`（純 Rust ReadyFuture）
   - 原理: 省掉 `get_running_loop` + `create_future` + `set_result`

10. **`WsConnectResult::Direct` 加 `Box`**
    - 原理: 修 clippy `large_enum_variant`（320 bytes → 8 bytes）,減少棧使用

### v0.5.0 效能測試結果

基於 500 次 roundtrip, 32 bytes, trimmed mean of 7:

| 實作               | 時間       | per-op    | vs websockets |
|-------------------|-----------|-----------|---------------|
| websocket-rs Sync | 50.9 ms   | 101.9 µs  | **1.81x 快**  |
| websockets Sync   | 92.3 ms   | 184.5 µs  | 1.00x         |
| websockets Async  | 91.0 ms   | 182.1 µs  | 1.00x         |
| websocket-rs Async| 109.3 ms  | 218.6 µs  | 0.83x (慢 17%)|

**大訊息 Sync 效能（16KB payload）:**
- websocket-rs: **3.2x** 快於 websockets

### 測試過的架構替代方案

#### 13. `pyo3_async_runtimes::tokio::future_into_py` 直接橋接 ❌

- **結果**: ❌ **慢 84%**（41.5ms vs 22.5ms / 100 roundtrips）
- **架構**: 砍掉 mpsc channel + 背景 task,send/recv 直接用 `future_into_py` 操作 WebSocket stream
- **失敗原因**:
  1. `future_into_py` 每次調用建 3-4 個 Python 物件（`asyncio.Future`, `PyDoneCallback`, `Cancellable`）
  2. 多層 `R::spawn()`（外層 + 內層 + `spawn_blocking`）
  3. 無條件 `add_done_callback` 附加
  4. 完全失去 ReadyFuture 快速路徑
- **結論**: ❌ `future_into_py` 的固定開銷 > mpsc channel 的開銷

#### 14. PyO3 Native `Coroutine` + `tokio::spawn` ❌

- **結果**: ❌ **慢 43%**（32.3ms vs 22.5ms / 100 roundtrips）
- **架構**: 使用 PyO3 0.27 的 `pyo3::impl_::coroutine::new_coroutine` + `experimental-async` feature
- **優於方案 13 的原因**:
  - Coroutine 是 lazy 的 — 如果 future 立即完成,不建 `asyncio.Future`
  - 少了 `PyDoneCallback` 和多層 spawn 開銷
- **仍然失敗的原因**:
  - `Coroutine::poll` 跑在 Python event loop 執行緒上,無 tokio context
  - 必須用 `handle.spawn(future)` 把 future 送回 tokio runtime
  - 這個 `spawn` 本身有 ~10µs 跨執行緒 task queue 開銷
  - 依然失去 ReadyFuture 快速路徑
- **結論**: ❌ 比 `future_into_py` 好,但仍不如 channel + ReadyFuture

#### 15. `std::sync::RwLock` 取代 `parking_lot::RwLock`（rustc 1.94）⚠️

- **結果**: ⚠️ **差距在誤差範圍內**
- **測試數據**（trimmed mean of 7, 500 roundtrips, rustc 1.94）:

| 版本                    | Async    | Sync     |
|------------------------|----------|----------|
| parking_lot + #[inline]| 109.3 ms | 50.4 ms  |
| std::sync - #[inline]  | 111.2 ms | 50.8 ms  |

- **與 v0.3.1 結論的差異**: v0.3.1 時 parking_lot 慢 5.6%,但在 v0.5.0 + rustc 1.94 下差距消失
- **可能原因**: rustc 1.92+ 改進了 `std::sync` 實作; v0.5.0 的其他優化改變了 hot path profile
- **結論**: ⚠️ 差異不顯著,保持 parking_lot 不動（避免無謂改動風險）

#### 16. rustc 1.90 → 1.94（LLVM 19 → 20）升級 ⚠️

- **結果**: ⚠️ **差距在誤差範圍內**
- **測試數據**（per-op, 同一 codebase）:

| rustc 版本 | Sync per-op |
|-----------|-------------|
| 1.90      | ~98 µs      |
| 1.94      | ~101 µs     |

- **結論**: ⚠️ LLVM 20 的 codegen 改進未在此場景顯現,差異 < 3% 在 noise 範圍

### 架構瓶頸最終分析

#### 為什麼 Async 比 Python websockets 慢 17%?

```
websockets (Python):
  Python event loop → 直接操作 socket → 回傳結果
  全程同一執行緒,零跨執行緒開銷

websocket-rs (Rust):
  Python event loop → mpsc channel → tokio background task → socket I/O
  socket I/O → tokio task → mpsc channel → call_soon_threadsafe → Python
  每次 recv slow path 付出 ~36µs "過橋稅"
```

**recv slow path 開銷分解**:
- mpsc channel recv: ~5 µs
- `call_soon_threadsafe`: ~10 µs（event loop lock + 喚醒 selector）
- `create_future` + `set_result`: ~10 µs（Python 物件建立）
- GIL acquire: ~5 µs
- 其他: ~6 µs

**為什麼 fast path 不夠?**

Request-Response 模式下,`recv()` 被呼叫時 server 回應尚未到達,`try_lock + try_recv` 幾乎 100% miss。
Fast path 只在 Pipelined 模式（buffer 有數據）才能命中。

#### 結論: 不走 tokio 的替代方案

| 方案                        | 可行性 | 預估效果      | 開發成本 |
|----------------------------|--------|--------------|---------|
| 預取模式（send 後自動等 recv）  | 中     | 10-20% 改善   | 中      |
| eventfd 取代 call_soon_threadsafe | 中   | 5-10% 改善   | 高      |
| Rust 跑在 Python event loop 上    | 低   | 可能追平      | 巨大    |
| Unix domain socket 支援           | 高   | 網路層 ~30µs  | 低      |

**跨機器場景下 bridge 開銷可忽略**: 網路延遲（1-100ms）遠大於 bridge 開銷（~36µs）,
在真實部署中 websocket-rs Async 與 Python websockets 體感相同。

### v0.5.0 最終結論

**Sync**: 已達到極致,穩定 **1.8x** 快於 websockets。大訊息時高達 **3.2x**。

**Async**: 慢 17% 是 PyO3 + tokio 跨執行緒架構的固有限制。
已測試 3 種替代架構（future_into_py / PyO3 Coroutine / std::sync）均無法改善。
Actor Pattern + channel + ReadyFuture 是當前 PyO3 架構下的最優解。

**推薦策略**: 接受 Async 的架構限制,發揮 Sync 的效能優勢。
在跨機器場景下 Async 的 17% 差距被網路延遲掩蓋,實際體感無差異。
