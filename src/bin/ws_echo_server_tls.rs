//! TLS variant of ws_echo_server. Loads cert.pem + key.pem from
//! tests/certs/ and serves wss:// on the given port. Uses rustls (pure
//! Rust, no OpenSSL) to avoid the global-state conflict that
//! native-tls + Python's _ssl produces in the same process.
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8821);

    // Load PEM cert + key (rustls accepts these natively via rustls-pemfile).
    let cert_pem = std::fs::read("tests/certs/cert.pem")?;
    let key_pem = std::fs::read("tests/certs/key.pem")?;

    let certs: Vec<CertificateDer> = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err("no certs in cert.pem".into());
    }

    let key: PrivateKeyDer = rustls_pemfile::private_key(&mut key_pem.as_slice())?
        .ok_or("no private key in key.pem")?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let listener = TcpListener::bind(&addr).await?;
    eprintln!("ws_echo_server_tls (rustls) listening on {addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        let _ = stream.set_nodelay(true);
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let Ok(tls_stream) = acceptor.accept(stream).await else {
                return;
            };
            let Ok(mut ws) = accept_async(tls_stream).await else {
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
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&bytes);
                if ws.send(Message::Binary(out.into())).await.is_err() {
                    break;
                }
            }
        });
    }
}
