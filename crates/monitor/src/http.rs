use super::registry::Registry;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const INDEX_HTML: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/index.html"));
const APP_JS: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/app.js"));
const STYLES_CSS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/styles.css"));
const MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/manifest.webmanifest"
));
const SERVICE_WORKER: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/sw.js"));

#[derive(Clone, Copy)]
struct StaticAsset {
    body: &'static [u8],
    content_type: &'static str,
}

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
        return json_response(
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
        return json_response(
            &mut stream,
            405,
            &serde_json::json!({ "error": "only GET is supported" }),
        )
        .await;
    }

    if let Some(asset) = static_asset(path) {
        return bytes_response(&mut stream, 200, asset.content_type, asset.body, "no-cache").await;
    }

    let state = registry.read().await.state(unix_seconds());
    match path {
        "/api/state" => json_response(&mut stream, 200, &state).await,
        "/api/hosts" => json_response(&mut stream, 200, &state.hosts).await,
        "/api/accounts" => json_response(&mut stream, 200, &state.accounts).await,
        "/api" => {
            json_response(
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
            json_response(
                &mut stream,
                404,
                &serde_json::json!({ "error": "not found" }),
            )
            .await
        }
    }
}

fn static_asset(path: &str) -> Option<StaticAsset> {
    match path {
        "/" | "/index.html" => Some(StaticAsset {
            body: INDEX_HTML,
            content_type: "text/html; charset=utf-8",
        }),
        "/app.js" => Some(StaticAsset {
            body: APP_JS,
            content_type: "text/javascript; charset=utf-8",
        }),
        "/styles.css" => Some(StaticAsset {
            body: STYLES_CSS,
            content_type: "text/css; charset=utf-8",
        }),
        "/manifest.webmanifest" => Some(StaticAsset {
            body: MANIFEST,
            content_type: "application/manifest+json; charset=utf-8",
        }),
        "/sw.js" => Some(StaticAsset {
            body: SERVICE_WORKER,
            content_type: "text/javascript; charset=utf-8",
        }),
        _ => None,
    }
}

async fn json_response<T: Serialize>(
    stream: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<(), BoxError> {
    let body = serde_json::to_vec(body)?;
    bytes_response(
        stream,
        status,
        "application/json; charset=utf-8",
        &body,
        "no-store",
    )
    .await
}

async fn bytes_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    cache_control: &str,
) -> Result<(), BoxError> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: {cache_control}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await?;
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::static_asset;

    #[test]
    fn embeds_pwa_assets_without_overlapping_api_routes() {
        for path in [
            "/",
            "/index.html",
            "/app.js",
            "/styles.css",
            "/manifest.webmanifest",
            "/sw.js",
        ] {
            let asset = static_asset(path).expect("static asset");
            assert!(!asset.body.is_empty());
        }
        assert!(static_asset("/api/state").is_none());
    }
}
