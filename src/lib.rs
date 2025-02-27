// 禁用非本地定義警告，因為這是 PyO3 宏的特性
#![allow(non_local_definitions)]
#![allow(dead_code)]  // 允許未使用的代碼警告

use futures::prelude::*;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream as TungsteniteWebSocketStream};
use url::Url;

// 常數配置
const DEFAULT_CONNECT_TIMEOUT: f64 = 30.0;
const DEFAULT_RECEIVE_TIMEOUT: f64 = 30.0;
const MAX_RETRY_ATTEMPTS: usize = 5;
const INITIAL_BACKOFF_MS: u64 = 100;
const MAX_BACKOFF_EXPONENT: u32 = 10;
const BUFFER_SIZE: usize = 32768;  // 增加到 32KB 緩衝區
const INITIAL_BUFFER_SIZE: usize = 8192;  // 增加到 8KB 初始緩衝區
const BUFFER_GROWTH_FACTOR: usize = 2;  // 緩衝區增長因子

// 自定義類型別名，提高可讀性
type WebSocketStream = TungsteniteWebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type ConnectionResult = Result<WebSocketStream, WebSocketError>;

// 緩衝區管理 - 高性能版本
struct MessageBuffer {
    send_buffer: Vec<u8>,
    receive_buffer: Vec<u8>,
    last_message_size: usize,  // 跟踪上一次消息大小以優化預分配
}

impl MessageBuffer {
    #[inline(always)]
    fn new() -> Self {
        Self {
            send_buffer: Vec::with_capacity(INITIAL_BUFFER_SIZE),
            receive_buffer: Vec::with_capacity(BUFFER_SIZE),
            last_message_size: 0,
        }
    }

    #[inline(always)]
    fn prepare_send(&mut self, message: &[u8]) -> &[u8] {
        let msg_len = message.len();
        self.last_message_size = msg_len;
        
        // 只有在需要時才調整緩衝區大小
        if self.send_buffer.capacity() < msg_len {
            // 使用增長因子來減少重新分配的頻率
            let new_capacity = (msg_len * BUFFER_GROWTH_FACTOR).max(INITIAL_BUFFER_SIZE);
            self.send_buffer.reserve(new_capacity - self.send_buffer.capacity());
        }
        
        self.send_buffer.clear();
        // 使用 unsafe 來避免邊界檢查，提高性能
        unsafe {
            self.send_buffer.set_len(msg_len);
            std::ptr::copy_nonoverlapping(message.as_ptr(), self.send_buffer.as_mut_ptr(), msg_len);
        }
        
        &self.send_buffer
    }

    #[inline(always)]
    fn prepare_receive(&mut self) -> &mut Vec<u8> {
        self.receive_buffer.clear();
        &mut self.receive_buffer
    }
    
    #[inline(always)]
    fn optimize_buffer_sizes(&mut self) {
        // 根據使用模式動態調整緩衝區大小
        if self.last_message_size > 0 {
            let optimal_size = (self.last_message_size * BUFFER_GROWTH_FACTOR).min(BUFFER_SIZE);
            
            if self.send_buffer.capacity() < optimal_size / 2 || 
               self.send_buffer.capacity() > optimal_size * 2 {
                let mut new_buffer = Vec::with_capacity(optimal_size);
                std::mem::swap(&mut self.send_buffer, &mut new_buffer);
            }
        }
    }
}

// 錯誤類型
#[derive(Debug)]
enum WebSocketError {
    ConnectionFailed(String),
    Timeout(String),
    StreamNotInitialized,
    InvalidUrl(String),
}

impl std::fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::StreamNotInitialized => write!(f, "WebSocket stream is not initialized"),
            Self::InvalidUrl(msg) => write!(f, "Invalid URL: {}", msg),
        }
    }
}

impl std::error::Error for WebSocketError {}

impl From<WebSocketError> for PyErr {
    fn from(err: WebSocketError) -> PyErr {
        match err {
            WebSocketError::ConnectionFailed(msg) => PyConnectionError::new_err(msg),
            WebSocketError::Timeout(msg) => PyTimeoutError::new_err(msg),
            WebSocketError::StreamNotInitialized => PyRuntimeError::new_err("WebSocket stream is not initialized"),
            WebSocketError::InvalidUrl(msg) => PyRuntimeError::new_err(format!("Invalid URL: {}", msg)),
        }
    }
}

impl From<std::io::Error> for WebSocketError {
    fn from(err: std::io::Error) -> Self {
        WebSocketError::ConnectionFailed(err.to_string())
    }
}

impl From<tungstenite::Error> for WebSocketError {
    fn from(err: tungstenite::Error) -> Self {
        WebSocketError::ConnectionFailed(err.to_string())
    }
}

impl From<url::ParseError> for WebSocketError {
    fn from(err: url::ParseError) -> Self {
        WebSocketError::InvalidUrl(err.to_string())
    }
}

// 緩存 URL 解析結果
struct ParsedUrl {
    url: Url,
}

impl ParsedUrl {
    fn new(url_str: &str) -> Result<Self, url::ParseError> {
        Ok(Self {
            url: Url::parse(url_str)?,
        })
    }
}

struct WebSocketState {
    stream: Option<WebSocketStream>,
    is_connected: bool,
    retry_count: usize,
    buffer: MessageBuffer,
}

/// WebSocket 客戶端的 Python 包裝類型
#[pyclass]
struct WebSocket {
    url: String,
    state: Arc<RwLock<WebSocketState>>,
    connect_timeout: Option<f64>,
    receive_timeout: Option<f64>,
    retry: bool,
    retry_limit: Option<usize>,
    parsed_url: Arc<Option<ParsedUrl>>,
}

impl WebSocket {
    async fn connect_with_retry(&self) -> ConnectionResult {
        let url_to_use = match &*self.parsed_url {
            Some(parsed) => parsed.url.clone(),
            None => Url::parse(&self.url)
                .map_err(|e| WebSocketError::InvalidUrl(e.to_string()))?,
        };

        let mut attempts = 0;
        loop {
            let connect_future = connect_async(url_to_use.clone());
            
            let connect_result = if let Some(timeout_secs) = self.connect_timeout {
                match timeout(Duration::from_secs_f64(timeout_secs), connect_future).await {
                    Ok(result) => result.map_err(|e| WebSocketError::ConnectionFailed(e.to_string())),
                    Err(_) => Err(WebSocketError::Timeout(
                        format!("Connection timed out ({} seconds)", timeout_secs)
                    )),
                }
            } else {
                connect_future.await.map_err(|e| WebSocketError::ConnectionFailed(e.to_string()))
            };

            match connect_result {
                Ok((stream, _)) => {
                    let mut state = self.state.write().await;
                    state.retry_count = attempts;
                    return Ok(stream);
                },
                Err(e) => {
                    attempts += 1;
                    if !self.retry || (self.retry_limit.is_some() && attempts >= self.retry_limit.unwrap()) {
                        return Err(WebSocketError::ConnectionFailed(
                            format!("Failed after {} attempts: {}", attempts, e)
                        ));
                    }
                    
                    let backoff_ms = INITIAL_BACKOFF_MS * (1 << attempts.min(MAX_BACKOFF_EXPONENT as usize));
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    fn new_internal(
        url: String,
        connect_timeout: Option<f64>,
        receive_timeout: Option<f64>,
        retry: bool,
        retry_limit: Option<usize>,
    ) -> Self {
        // 預先解析 URL 以避免重複解析
        let parsed_url = ParsedUrl::new(&url).ok();
        
        WebSocket {
            url,
            state: Arc::new(RwLock::new(WebSocketState {
                stream: None,
                is_connected: false,
                retry_count: 0,
                buffer: MessageBuffer::new(),
            })),
            connect_timeout: connect_timeout.or(Some(DEFAULT_CONNECT_TIMEOUT)),
            receive_timeout: receive_timeout.or(Some(DEFAULT_RECEIVE_TIMEOUT)),
            retry,
            retry_limit: retry_limit.or(Some(MAX_RETRY_ATTEMPTS)),
            parsed_url: Arc::new(parsed_url),
        }
    }
}

#[pymethods]
impl WebSocket {
    #[new]
    #[pyo3(signature = (
        url,
        connect_timeout = None,
        receive_timeout = None,
        retry = false,
        retry_limit = None
    ))]
    fn new(
        url: String,
        connect_timeout: Option<f64>,
        receive_timeout: Option<f64>,
        retry: bool,
        retry_limit: Option<usize>,
    ) -> Self {
        Self::new_internal(url, connect_timeout, receive_timeout, retry, retry_limit)
    }

    #[getter]
    fn is_connected(&self) -> bool {
        futures::executor::block_on(async {
            self.state.read().await.is_connected
        })
    }

    #[getter]
    fn retry_count(&self) -> usize {
        futures::executor::block_on(async {
            self.state.read().await.retry_count
        })
    }

    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<&PyAny> {
        let ws = slf.borrow(py);
        let state = ws.state.clone();
        let url = ws.url.clone();
        let connect_timeout = ws.connect_timeout;
        let retry = ws.retry;
        let retry_limit = ws.retry_limit;
        drop(ws);  // 釋放借用
        
        let ws = WebSocket::new_internal(
            url,
            connect_timeout,
            None,
            retry,
            retry_limit,
        );
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let stream = ws.connect_with_retry().await
                .map_err(|e| PyConnectionError::new_err(e.to_string()))?;
            
            let mut state = state.write().await;
            state.stream = Some(stream);
            state.is_connected = true;
            state.retry_count = 0;
            
            Python::with_gil(|py| Ok(slf.into_py(py)))
        })
    }

    fn __aexit__<'p>(
        &self,
        py: Python<'p>,
        _exc_type: Option<&'p PyAny>,
        _exc_value: Option<&'p PyAny>,
        _traceback: Option<&'p PyAny>,
    ) -> PyResult<&'p PyAny> {
        let state = self.state.clone();
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let mut state = state.write().await;
            if let Some(ref mut ws) = state.stream {
                let _ = ws.close(None).await;
            }
            
            state.is_connected = false;
            state.stream = None;
            
            Python::with_gil(|py| Ok(false.into_py(py)))
        })
    }

    fn send<'py>(&self, py: Python<'py>, message: &PyAny) -> PyResult<&'py PyAny> {
        // 直接從 Python 對象獲取字節，避免不必要的轉換
        let message_bytes = if let Ok(s) = message.downcast::<PyString>() {
            s.to_str()?.as_bytes().to_vec()
        } else if let Ok(b) = message.downcast::<PyBytes>() {
            b.as_bytes().to_vec()
        } else {
            return Err(PyRuntimeError::new_err("Message must be string or bytes"));
        };
        
        let state = self.state.clone();
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 使用讀寫鎖進行更高效的訪問
            {
                let state_read = state.read().await;
                if !state_read.is_connected {
                    return Err(PyRuntimeError::new_err("WebSocket is not connected"));
                }
                
                if state_read.stream.is_none() {
                    return Err(PyRuntimeError::new_err("WebSocket stream is not initialized"));
                }
            }
            
            // 只在需要時獲取寫鎖
            let mut state_write = state.write().await;
            // 使用優化的緩衝區準備
            let send_data = state_write.buffer.prepare_send(&message_bytes).to_vec();
            
            let result = if let Some(ref mut stream) = state_write.stream {
                // 使用二進制消息以獲得更好的性能
                stream.send(Message::Binary(send_data)).await
            } else {
                return Err(PyRuntimeError::new_err("WebSocket stream is not initialized"));
            };
            
            // 優化緩衝區大小
            state_write.buffer.optimize_buffer_sizes();
            
            result.map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))?;
            
            Python::with_gil(|py| Ok(py.None()))
        })
    }

    fn receive<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let state = self.state.clone();
        let receive_timeout = self.receive_timeout;
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 使用讀寫鎖進行更高效的訪問
            {
                let state_read = state.read().await;
                if !state_read.is_connected {
                    return Err(PyRuntimeError::new_err("WebSocket is not connected"));
                }
                
                if state_read.stream.is_none() {
                    return Err(PyRuntimeError::new_err("WebSocket stream is not initialized"));
                }
            }
            
            // 獲取消息
            let msg = {
                // 只在需要時獲取寫鎖
                let mut state_write = state.write().await;
                
                if let Some(stream_ref) = state_write.stream.as_mut() {
                    // 使用 timeout 包裝接收操作
                    if let Some(timeout_secs) = receive_timeout {
                        match timeout(Duration::from_secs_f64(timeout_secs), stream_ref.next()).await {
                            Ok(Some(Ok(msg))) => msg,
                            Ok(Some(Err(e))) => return Err(PyRuntimeError::new_err(format!("Receive failed: {}", e))),
                            Ok(None) => return Err(PyRuntimeError::new_err("Connection closed")),
                            Err(_) => return Err(PyTimeoutError::new_err(
                                format!("Receive timed out ({} seconds)", timeout_secs)
                            )),
                        }
                    } else {
                        match stream_ref.next().await {
                            Some(Ok(msg)) => msg,
                            Some(Err(e)) => return Err(PyRuntimeError::new_err(format!("Receive failed: {}", e))),
                            None => return Err(PyRuntimeError::new_err("Connection closed")),
                        }
                    }
                } else {
                    return Err(PyRuntimeError::new_err("WebSocket stream is not initialized"));
                }
            };
            
            // 處理接收到的消息
            match msg {
                Message::Text(text) => {
                    // 在 GIL 內處理 Python 對象
                    Python::with_gil(|py| Ok(text.into_py(py)))
                },
                Message::Binary(data) => {
                    // 直接創建 Python 字節對象，避免額外的複製
                    Python::with_gil(|py| {
                        let py_bytes = PyBytes::new(py, &data);
                        Ok(py_bytes.into_py(py))
                    })
                },
                _ => Err(PyRuntimeError::new_err("Received non-text/binary message")),
            }
        })
    }

    fn reconnect<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let state = self.state.clone();
        let url = self.url.clone();
        let connect_timeout = self.connect_timeout;
        let retry = self.retry;
        let retry_limit = self.retry_limit;
        
        let ws = WebSocket::new_internal(
            url,
            connect_timeout,
            None,
            retry,
            retry_limit,
        );
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 關閉現有連接
            {
                let mut state_guard = state.write().await;
                if let Some(ref mut ws_conn) = state_guard.stream {
                    let _ = ws_conn.close(None).await;
                }
                
                state_guard.is_connected = false;
                state_guard.stream = None;
            } // 釋放寫鎖
            
            // 建立新連接
            let stream = ws.connect_with_retry().await
                .map_err(|e| PyConnectionError::new_err(e.to_string()))?;
            
            // 更新狀態
            let mut state = state.write().await;
            state.stream = Some(stream);
            state.is_connected = true;
            state.retry_count = 0;
            
            Python::with_gil(|py| Ok(py.None()))
        })
    }
}

#[pymodule]
fn websocket_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    // 預先初始化 tokio 運行時
    pyo3_asyncio::tokio::get_runtime();
    m.add_class::<WebSocket>()?;
    Ok(())
}

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
