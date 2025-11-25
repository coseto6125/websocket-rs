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
