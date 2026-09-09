use super::registry::Registry;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub async fn serve(bind: SocketAddr, registry: Arc<RwLock<Registry>>) -> Result<(), BoxError> {
    let listener = TcpListener::bind(bind).await?;
    eprintln!("codex-monitor HTTP API listening on {bind}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            if let Err(error) = handle(stream, registry).await {
                eprintln!("HTTP request from {peer} failed: {error}");
            }
        });
    }
}

async fn handle(mut stream: TcpStream, registry: Arc<RwLock<Registry>>) -> Result<(), BoxError> {
    let mut buffer = [0u8; 8 * 1024];
    let length = stream.read(&mut buffer).await?;
    if length == 0 {
        return Ok(());
    }
    let request = std::str::from_utf8(&buffer[..length])?;
    let Some(request_line) = request.lines().next() else {
        return response(
            &mut stream,
            400,
            &serde_json::json!({ "error": "bad request" }),
        )
        .await;
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    if method != "GET" {
        return response(
            &mut stream,
            405,
            &serde_json::json!({ "error": "only GET is supported" }),
        )
        .await;
    }

    let state = registry.read().await.state(unix_seconds());
    match path {
        "/api/state" => response(&mut stream, 200, &state).await,
        "/api/hosts" => response(&mut stream, 200, &state.hosts).await,
        "/api/accounts" => response(&mut stream, 200, &state.accounts).await,
        "/" => {
            response(
                &mut stream,
                200,
                &serde_json::json!({
                    "service": "codex-monitor",
                    "api": ["/api/state", "/api/hosts", "/api/accounts"]
                }),
            )
            .await
        }
        _ => {
            response(
                &mut stream,
                404,
                &serde_json::json!({ "error": "not found" }),
            )
            .await
        }
    }
}

async fn response<T: Serialize>(
    stream: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<(), BoxError> {
    let body = serde_json::to_vec(body)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await?;
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
