//! fastwebsockets (Deno/Cloudflare) echo server — third neutral baseline.
//!
//! Uses a different frame codec / handshake stack than tokio-tungstenite so
//! client-side numbers can be cross-validated against two independent pure-Rust
//! servers. Payload layout matches the other benches (=IddI header + data) so
//! the same Python parse_response works.

use fastwebsockets::{upgrade, OpCode, WebSocketError};
use http_body_util::Empty;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use tokio::net::TcpListener;

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);
    loop {
        let frame = ws.read_frame().await?;
        match frame.opcode {
            OpCode::Close => break,
            OpCode::Text | OpCode::Binary => {
                let payload: &[u8] = &frame.payload;
                let mid: u32 = if payload.len() >= 4 {
                    u32::from_le_bytes(payload[..4].try_into().unwrap())
                } else {
                    0
                };
                let mut out = Vec::with_capacity(24 + payload.len());
                out.extend_from_slice(&mid.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                out.extend_from_slice(payload);
                let reply = fastwebsockets::Frame::binary(fastwebsockets::Payload::Owned(out));
                ws.write_frame(reply).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn server_upgrade(
    mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, WebSocketError> {
    let (response, fut) = upgrade::upgrade(&mut req)?;
    tokio::task::spawn(async move {
        if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
            eprintln!("ws handler err: {e}");
        }
    });
    Ok(response)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8822);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .build()?;
    rt.block_on(async move {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        eprintln!("fastwebsockets echo listening on 127.0.0.1:{port}");
        loop {
            let (stream, _) = listener.accept().await?;
            let _ = stream.set_nodelay(true);
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let conn_fut = http1::Builder::new()
                    .serve_connection(io, service_fn(server_upgrade))
                    .with_upgrades();
                if let Err(e) = conn_fut.await {
                    eprintln!("conn err: {e:?}");
                }
            });
        }
    })
}
