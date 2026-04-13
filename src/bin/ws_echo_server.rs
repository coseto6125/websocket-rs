//! Pure-Rust WS echo server for benchmarking, in the spirit of tarasko's C++ beast server.
//!
//! Echo protocol injects server-side timestamps into the payload head so clients can
//! measure one-way send/recv latency. Wire format per reply:
//!   [msg_id: u32 LE][t_recv: f64 LE][t_send: f64 LE][orig_len: u32 LE][orig_payload...]

//! Stamp format kept identical to the Python server (=IddI+payload) so all benchmarks
//! share one parse_response. Timestamps are zero (client-side t_recv - t_send wouldn't
//! line up across clocks anyway — benchmarks use client-measured RTT).
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8820);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let listener = TcpListener::bind(&addr).await?;
    eprintln!("ws_echo_server listening on {addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        tokio::spawn(async move {
            let Ok(mut ws) = accept_async(stream).await else {
                return;
            };
            while let Some(Ok(msg)) = ws.next().await {
                let bytes = match msg {
                    Message::Binary(b) => b,
                    Message::Text(t) => bytes::Bytes::from(t.as_str().as_bytes().to_vec()),
                    Message::Close(_) => {
                        let _ = ws.close(None).await;
                        break;
                    }
                    _ => continue,
                };
                let mid: u32 = if bytes.len() >= 4 {
                    u32::from_le_bytes(bytes[..4].try_into().unwrap())
                } else {
                    0
                };
                let mut out = Vec::with_capacity(24 + bytes.len());
                out.extend_from_slice(&mid.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes()); // t_recv placeholder
                out.extend_from_slice(&0f64.to_le_bytes()); // t_send placeholder
                out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&bytes);
                if ws.send(Message::Binary(out.into())).await.is_err() {
                    break;
                }
            }
        });
    }
}
