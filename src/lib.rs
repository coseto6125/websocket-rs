// 禁用非本地定義警告，因為這是 PyO3 宏的特性
#![allow(non_local_definitions)]

use pyo3::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyConnectionError, PyTimeoutError};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::prelude::*;
use url::Url;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;
use tokio::time::timeout;

/// WebSocket 客戶端的 Python 包裝類型
#[pyclass]
struct WebSocket {
    url: String,
    is_connected: Arc<Mutex<bool>>,
    ws_stream: Arc<Mutex<Option<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>>>,
    connect_timeout: Option<f64>,
    receive_timeout: Option<f64>,
    retry: bool,
    retry_limit: Option<usize>,
    retry_count: Arc<Mutex<usize>>,
    parsed_url: Arc<Option<Url>>, // 預先解析 URL 以避免重複解析
}

impl WebSocket {
    fn new_internal(
        url: String,
        connect_timeout: Option<f64>,
        receive_timeout: Option<f64>,
        retry: bool,
        retry_limit: Option<usize>,
    ) -> Self {
        // 預先解析 URL
        let parsed_url = match Url::parse(&url) {
            Ok(parsed) => Some(parsed),
            Err(_) => None, // 如果解析失敗，我們將在連接時再次嘗試並提供更好的錯誤訊息
        };
        
        WebSocket {
            url,
            is_connected: Arc::new(Mutex::new(false)),
            ws_stream: Arc::new(Mutex::new(None)),
            connect_timeout,
            receive_timeout,
            retry,
            retry_limit,
            retry_count: Arc::new(Mutex::new(0)),
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
            *self.is_connected.lock().await
        })
    }

    #[getter]
    fn retry_count(&self) -> usize {
        futures::executor::block_on(async {
            *self.retry_count.lock().await
        })
    }

    fn __aenter__(slf: Py<Self>, py: Python<'_>) -> PyResult<&PyAny> {
        let slf_ref = slf.borrow(py);
        let url = slf_ref.url.clone();
        let parsed_url = slf_ref.parsed_url.clone();
        let ws_stream = slf_ref.ws_stream.clone();
        let is_connected = slf_ref.is_connected.clone();
        let retry_count = slf_ref.retry_count.clone();
        let connect_timeout = slf_ref.connect_timeout;
        let retry = slf_ref.retry;
        let retry_limit = slf_ref.retry_limit;
        let slf_clone = slf.clone();

        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 使用預先解析的 URL 或重新解析
            let url_to_use = match &*parsed_url {
                Some(parsed) => parsed.clone(), // 使用預先解析的 URL
                None => Url::parse(&url)
                    .map_err(|e| PyRuntimeError::new_err(format!("Invalid URL: {}", e)))?, // 重新解析
            };
            
            let mut attempts = 0;
            let mut stream_result = None;
            
            while stream_result.is_none() {
                let connect_future = connect_async(url_to_use.clone());
                
                let connect_result = if let Some(timeout_secs) = connect_timeout {
                    match timeout(Duration::from_secs_f64(timeout_secs), connect_future).await {
                        Ok(result) => result,
                        Err(_) => Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("Connection timed out ({} seconds)", timeout_secs),
                        ).into()),
                    }
                } else {
                    connect_future.await
                };
                
                match connect_result {
                    Ok((stream, _)) => {
                        stream_result = Some(stream);
                        break;
                    },
                    Err(e) => {
                        attempts += 1;
                        if !retry || (retry_limit.is_some() && attempts >= retry_limit.unwrap()) {
                            return Err(PyConnectionError::new_err(format!(
                                "Connection failed (attempted {} times): {}", 
                                attempts, e
                            )));
                        }
                        
                        // 使用指數退避策略進行重試
                        let backoff_ms = 100 * (1 << attempts.min(10));
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
            
            // 更新狀態
            if let Some(stream) = stream_result {
                *ws_stream.lock().await = Some(stream);
                *is_connected.lock().await = true;
                *retry_count.lock().await = attempts;
            }
            
            Python::with_gil(|py| Ok(slf_clone.into_py(py)))
        })
    }

    fn __aexit__<'p>(
        &self,
        py: Python<'p>,
        _exc_type: Option<&'p PyAny>,
        _exc_value: Option<&'p PyAny>,
        _traceback: Option<&'p PyAny>,
    ) -> PyResult<&'p PyAny> {
        let is_connected = self.is_connected.clone();
        let ws_stream = self.ws_stream.clone();
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let mut stream = ws_stream.lock().await;
            
            // 嘗試優雅地關閉連接
            if let Some(ref mut ws) = *stream {
                // 忽略關閉錯誤，因為我們無論如何都會釋放資源
                let _ = ws.close(None).await;
            }
            
            *is_connected.lock().await = false;
            *stream = None;
            
            Python::with_gil(|py| Ok(false.into_py(py)))
        })
    }

    fn send<'py>(&self, py: Python<'py>, message: String) -> PyResult<&'py PyAny> {
        let is_connected = self.is_connected.clone();
        let ws_stream = self.ws_stream.clone();
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 快速檢查連接狀態，避免獲取鎖
            if !*is_connected.lock().await {
                return Err(PyRuntimeError::new_err("WebSocket is not connected"));
            }
            
            let mut stream = ws_stream.lock().await;
            let stream = stream.as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket stream is not initialized"))?;
            
            // 使用 Message::text 而不是 Message::Text 以避免不必要的克隆
            stream.send(Message::text(message)).await
                .map_err(|e| PyRuntimeError::new_err(format!("Send failed: {}", e)))?;
            
            Python::with_gil(|py| Ok(py.None()))
        })
    }

    fn receive<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let is_connected = self.is_connected.clone();
        let ws_stream = self.ws_stream.clone();
        let receive_timeout = self.receive_timeout;
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 快速檢查連接狀態，避免獲取鎖
            if !*is_connected.lock().await {
                return Err(PyRuntimeError::new_err("WebSocket is not connected"));
            }
            
            let mut stream = ws_stream.lock().await;
            let stream = stream.as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("WebSocket stream is not initialized"))?;
            
            let receive_future = stream.next();
            
            let msg = if let Some(timeout_secs) = receive_timeout {
                match timeout(Duration::from_secs_f64(timeout_secs), receive_future).await {
                    Ok(result) => result,
                    Err(_) => return Err(PyTimeoutError::new_err(
                        format!("Receive timed out ({} seconds)", timeout_secs)
                    )),
                }
            } else {
                receive_future.await
            };
            
            let msg = msg
                .ok_or_else(|| PyRuntimeError::new_err("Connection is closed"))?
                .map_err(|e| PyRuntimeError::new_err(format!("Receive failed: {}", e)))?;
            
            Python::with_gil(|py| match msg {
                Message::Text(text) => Ok(text.into_py(py)),
                Message::Binary(data) => Ok(data.into_py(py)),
                _ => Err(PyRuntimeError::new_err("Received non-text/binary message")),
            })
        })
    }

    /// 重新連接 WebSocket
    fn reconnect<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let is_connected = self.is_connected.clone();
        let ws_stream = self.ws_stream.clone();
        let retry_count = self.retry_count.clone();
        let url = self.url.clone();
        let parsed_url = self.parsed_url.clone();
        let connect_timeout = self.connect_timeout;
        let retry = self.retry;
        let retry_limit = self.retry_limit;
        
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // 關閉現有連接
            let mut stream_guard = ws_stream.lock().await;
            
            // 嘗試優雅地關閉連接
            if let Some(ref mut ws) = *stream_guard {
                // 忽略關閉錯誤，因為我們無論如何都會釋放資源
                let _ = ws.close(None).await;
            }
            
            *is_connected.lock().await = false;
            *stream_guard = None;
            drop(stream_guard);
            
            // 使用預先解析的 URL 或重新解析
            let url_to_use = match &*parsed_url {
                Some(parsed) => parsed.clone(), // 使用預先解析的 URL
                None => Url::parse(&url)
                    .map_err(|e| PyRuntimeError::new_err(format!("Invalid URL: {}", e)))?, // 重新解析
            };
            
            let mut attempts = 0;
            let mut stream_result = None;
            
            while stream_result.is_none() {
                let connect_future = connect_async(url_to_use.clone());
                
                let connect_result = if let Some(timeout_secs) = connect_timeout {
                    match timeout(Duration::from_secs_f64(timeout_secs), connect_future).await {
                        Ok(result) => result,
                        Err(_) => Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("Connection timed out ({} seconds)", timeout_secs),
                        ).into()),
                    }
                } else {
                    connect_future.await
                };
                
                match connect_result {
                    Ok((stream, _)) => {
                        stream_result = Some(stream);
                        break;
                    },
                    Err(e) => {
                        attempts += 1;
                        if !retry || (retry_limit.is_some() && attempts >= retry_limit.unwrap()) {
                            return Err(PyConnectionError::new_err(format!(
                                "Reconnection failed (attempted {} times): {}", 
                                attempts, e
                            )));
                        }
                        
                        // 使用指數退避策略進行重試
                        let backoff_ms = 100 * (1 << attempts.min(10));
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
            
            // 更新狀態
            if let Some(stream) = stream_result {
                let mut stream_guard = ws_stream.lock().await;
                *stream_guard = Some(stream);
                *is_connected.lock().await = true;
                *retry_count.lock().await = attempts;
                
                return Python::with_gil(|py| Ok(py.None()));
            }
            
            Err(PyConnectionError::new_err("Reconnection failed"))
        })
    }
}

#[pymodule]
fn websocket_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
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
