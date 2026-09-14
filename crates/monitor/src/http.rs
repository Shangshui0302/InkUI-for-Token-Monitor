use super::discovery::DiscoveryCache;
use super::registry::{NetworkView, Registry};
use codex_monitor_protocol::{validate_pairing_code, ControlRequest, ControlResult};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::time::{timeout, Duration};

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
const ICON_SVG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/icon.svg"));
const APP_ICON_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/app-icon.png"
));
const APP_ICON_192_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/app-icon-192.png"
));
const CAT_SPRITES_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/cat-sprites.png"
));

#[derive(Clone, Copy)]
struct StaticAsset {
    body: &'static [u8],
    content_type: &'static str,
}

pub(crate) struct ControlCommand {
    pub request: ControlRequest,
    pub respond_to: oneshot::Sender<ControlResult>,
}

pub(crate) struct PairCommand {
    pub node_id: String,
    pub pairing_code: String,
    pub respond_to: oneshot::Sender<Result<(), String>>,
}

pub async fn serve(
    bind: SocketAddr,
    registry: Arc<RwLock<Registry>>,
    read_token: Option<String>,
    control_token: Option<String>,
    control_tx: mpsc::Sender<ControlCommand>,
    discoveries: Arc<Mutex<DiscoveryCache>>,
    network_id: String,
    pair_tx: mpsc::Sender<PairCommand>,
    alias: Arc<RwLock<String>>,
    alias_path: std::path::PathBuf,
    pairing_code: String,
) -> Result<(), BoxError> {
    let listener = TcpListener::bind(bind).await?;
    eprintln!("codex-monitor HTTP API listening on {bind}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let registry = Arc::clone(&registry);
        let read_token = read_token.clone();
        let control_token = control_token.clone();
        let control_tx = control_tx.clone();
        let discoveries = Arc::clone(&discoveries);
        let network_id = network_id.clone();
        let pair_tx = pair_tx.clone();
        let alias = Arc::clone(&alias);
        let alias_path = alias_path.clone();
        let pairing_code = pairing_code.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(
                stream,
                registry,
                read_token.as_deref(),
                control_token.as_deref(),
                &control_tx,
                discoveries,
                &network_id,
                &pair_tx,
                alias,
                &alias_path,
                &pairing_code,
            )
            .await
            {
                eprintln!("HTTP request from {peer} failed: {error}");
            }
        });
    }
}

async fn handle(
    mut stream: TcpStream,
    registry: Arc<RwLock<Registry>>,
    read_token: Option<&str>,
    control_token: Option<&str>,
    control_tx: &mpsc::Sender<ControlCommand>,
    discoveries: Arc<Mutex<DiscoveryCache>>,
    network_id: &str,
    pair_tx: &mpsc::Sender<PairCommand>,
    alias: Arc<RwLock<String>>,
    alias_path: &std::path::Path,
    pairing_code: &str,
) -> Result<(), BoxError> {
    let Some(request) = timeout(Duration::from_secs(5), read_request(&mut stream)).await?? else {
        return Ok(());
    };
    let Some(request_line) = request.head.lines().next() else {
        return json_response(
            &mut stream,
            400,
            &serde_json::json!({ "error": "bad request" }),
        )
        .await;
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method == "OPTIONS" && (path == "/api" || path.starts_with("/api/")) {
        return preflight_response(&mut stream).await;
    }
    if method != "GET" && method != "POST" && method != "PATCH" {
        return json_response(
            &mut stream,
            405,
            &serde_json::json!({ "error": "only GET and POST are supported" }),
        )
        .await;
    }

    if method == "GET" {
        if let Some(asset) = static_asset(path) {
            return bytes_response(&mut stream, 200, asset.content_type, asset.body, "no-cache")
                .await;
        }
    }
    if method == "POST" && path == "/api/v1/control" {
        return handle_control(&mut stream, &request, control_token, control_tx).await;
    }
    if method == "GET" && path == "/api/v1/pairing-code" {
        return handle_pairing_code(&mut stream, &request, control_token, pairing_code).await;
    }
    if method == "POST" && path == "/api/v1/pair" {
        return handle_pair(&mut stream, &request, control_token, pair_tx).await;
    }
    if method == "PATCH" && path == "/api/v1/identity" {
        return handle_identity_patch(
            &mut stream,
            &request,
            control_token,
            registry,
            alias,
            alias_path,
        )
        .await;
    }
    if method != "GET" {
        return json_response(
            &mut stream,
            405,
            &serde_json::json!({ "error": "unsupported POST path" }),
        )
        .await;
    }
    if path == "/api" || path.starts_with("/api/") {
        if let Some(expected) = read_token {
            if !authorized(&request.head, expected) {
                return unauthorized_response(&mut stream).await;
            }
        }
    }
    let since = match parse_since(query) {
        Ok(value) => value,
        Err(_) => {
            return json_response(
                &mut stream,
                400,
                &serde_json::json!({ "error": "invalid since cursor" }),
            )
            .await;
        }
    };

    let registry = registry.read().await;
    let now = unix_seconds();
    let state: NetworkView = registry.state(now);
    match path {
        "/api/state" => json_response(&mut stream, 200, &state).await,
        "/api/hosts" => json_response(&mut stream, 200, &state.hosts).await,
        "/api/accounts" => json_response(&mut stream, 200, &state.accounts).await,
        "/api/v1/identity" => json_response(&mut stream, 200, &registry.identity()).await,
        "/api/v1/peers" => json_response(&mut stream, 200, &registry.peers(now)).await,
        "/api/v1/discoveries" => {
            drop(registry);
            let mut discoveries = discoveries.lock().await;
            let devices = discoveries.views(now, network_id);
            let available = discoveries.available();
            let reason = discoveries.reason().map(str::to_string);
            let body = match reason {
                Some(reason) => {
                    serde_json::json!({"available": available, "reason": reason, "devices": devices})
                }
                None => serde_json::json!({"available": available, "devices": devices}),
            };
            json_response(&mut stream, 200, &body).await
        }
        "/api/v1/state" => match since {
            Some(since) if since == state.cursor => {
                not_modified_response(&mut stream, state.cursor).await
            }
            _ => json_response(&mut stream, 200, &state).await,
        },
        "/api/v1/health" => json_response(&mut stream, 200, &registry.health(now)).await,
        "/api" => {
            json_response(
                &mut stream,
                200,
                &serde_json::json!({
                    "service": "codex-monitor",
                    "api": [
                        "/api/state",
                        "/api/hosts",
                        "/api/accounts",
                        "/api/v1/identity",
                        "/api/v1/peers",
                        "/api/v1/discoveries",
                        "/api/v1/state",
                        "/api/v1/health",
                        "/api/v1/control",
                        "/api/v1/pair",
                        "/api/v1/pairing-code"
                    ]
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

async fn handle_pairing_code(
    stream: &mut TcpStream,
    request: &HttpRequest,
    token: Option<&str>,
    pairing_code: &str,
) -> Result<(), BoxError> {
    let Some(expected) = token else {
        return json_response(
            stream,
            403,
            &serde_json::json!({"error":"pairing-code access is disabled without a control token"}),
        )
        .await;
    };
    if !authorized(&request.head, expected) {
        return control_unauthorized_response(stream).await;
    }
    json_response(
        stream,
        200,
        &serde_json::json!({"pairingCode": pairing_code}),
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairRequest {
    node_id: String,
    pairing_code: String,
}

async fn handle_pair(
    stream: &mut TcpStream,
    request: &HttpRequest,
    token: Option<&str>,
    tx: &mpsc::Sender<PairCommand>,
) -> Result<(), BoxError> {
    let Some(expected) = token else {
        return json_response(
            stream,
            403,
            &serde_json::json!({"error":"pairing is disabled without a control token"}),
        )
        .await;
    };
    if !authorized(&request.head, expected) {
        return control_unauthorized_response(stream).await;
    }
    let body: PairRequest = match serde_json::from_slice(&request.body) {
        Ok(v) => v,
        Err(e) => {
            return json_response(
                stream,
                400,
                &serde_json::json!({"error":format!("invalid pair request: {e}")}),
            )
            .await
        }
    };
    if body.node_id.is_empty() || body.node_id.len() > 128 {
        return json_response(
            stream,
            400,
            &serde_json::json!({"error":"nodeId must be 1-128 bytes"}),
        )
        .await;
    }
    if let Err(reason) = validate_pairing_code(&body.pairing_code) {
        return json_response(
            stream,
            400,
            &serde_json::json!({"error":format!("pairingCode {reason}")}),
        )
        .await;
    }
    let node_id = body.node_id.clone();
    let (respond_to, response) = oneshot::channel();
    if tx
        .send(PairCommand {
            node_id: body.node_id,
            pairing_code: body.pairing_code,
            respond_to,
        })
        .await
        .is_err()
    {
        return json_response(
            stream,
            503,
            &serde_json::json!({"error":"pairing service is unavailable"}),
        )
        .await;
    }
    match timeout(Duration::from_secs(20), response).await {
        Ok(Ok(Ok(()))) => {
            json_response(
                stream,
                200,
                &serde_json::json!({"nodeId":node_id,"status":"connected"}),
            )
            .await
        }
        Ok(Ok(Err(error))) if error == "discovery not found" => {
            json_response(stream, 404, &serde_json::json!({"error":error})).await
        }
        Ok(Ok(Err(error))) if error == "network id mismatch" => {
            json_response(stream, 409, &serde_json::json!({"error":error})).await
        }
        Ok(Ok(Err(error))) => json_response(stream, 409, &serde_json::json!({"error":error})).await,
        Ok(Err(_)) => {
            json_response(
                stream,
                503,
                &serde_json::json!({"error":"pairing service closed"}),
            )
            .await
        }
        Err(_) => {
            json_response(
                stream,
                504,
                &serde_json::json!({"error":"pairing timed out"}),
            )
            .await
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AliasRequest {
    alias: String,
}

async fn handle_identity_patch(
    stream: &mut TcpStream,
    request: &HttpRequest,
    token: Option<&str>,
    registry: Arc<RwLock<Registry>>,
    alias: Arc<RwLock<String>>,
    path: &std::path::Path,
) -> Result<(), BoxError> {
    let Some(expected) = token else {
        return json_response(
            stream,
            403,
            &serde_json::json!({"error":"identity management is disabled without a control token"}),
        )
        .await;
    };
    if !authorized(&request.head, expected) {
        return control_unauthorized_response(stream).await;
    }
    let body: AliasRequest = match serde_json::from_slice(&request.body) {
        Ok(v) => v,
        Err(e) => {
            return json_response(
                stream,
                400,
                &serde_json::json!({"error":format!("invalid identity request: {e}")}),
            )
            .await
        }
    };
    let value = body.alias.trim();
    if !valid_alias(value) {
        return json_response(
            stream,
            400,
            &serde_json::json!({"error":"alias must be 1-64 bytes without control characters"}),
        )
        .await;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, value.as_bytes())?;
    *alias.write().await = value.to_string();
    let mut registry = registry.write().await;
    let identity = registry.identity();
    registry.set_identity(
        identity.node_id.unwrap_or_default(),
        value.to_string(),
        identity.network_id,
        identity.capabilities,
    );
    json_response(stream, 200, &registry.identity()).await
}

fn valid_alias(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
}

struct HttpRequest {
    head: String,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> Result<Option<HttpRequest>, BoxError> {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;
    let mut bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("HTTP request exceeds maximum size".into());
        }
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end - 4])?.to_string();
    let content_length = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>())
        })
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES || header_end + content_length > MAX_REQUEST_BYTES {
        return Err("HTTP request body exceeds maximum size".into());
    }
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err("HTTP request body ended early".into());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("HTTP request exceeds maximum size".into());
        }
    }
    Ok(Some(HttpRequest {
        head,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }))
}

async fn handle_control(
    stream: &mut TcpStream,
    request: &HttpRequest,
    control_token: Option<&str>,
    control_tx: &mpsc::Sender<ControlCommand>,
) -> Result<(), BoxError> {
    let Some(expected) = control_token else {
        return json_response(
            stream,
            403,
            &serde_json::json!({ "error": "control is disabled" }),
        )
        .await;
    };
    if !authorized(&request.head, expected) {
        return control_unauthorized_response(stream).await;
    }
    let control: ControlRequest = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => {
            return json_response(
                stream,
                400,
                &serde_json::json!({ "error": format!("invalid control request: {error}") }),
            )
            .await;
        }
    };
    let now = unix_seconds();
    if let Err(error) = control.validate(now) {
        return json_response(stream, 400, &serde_json::json!({ "error": error })).await;
    }
    let request_id = control.request_id.clone();
    let (respond_to, response) = oneshot::channel();
    if control_tx
        .send(ControlCommand {
            request: control,
            respond_to,
        })
        .await
        .is_err()
    {
        return json_response(
            stream,
            503,
            &serde_json::json!({ "error": "local collector is unavailable" }),
        )
        .await;
    }
    let result = match timeout(Duration::from_secs(15), response).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => ControlResult {
            request_id,
            ok: false,
            message: "local collector closed before handling the request".to_string(),
        },
        Err(_) => ControlResult {
            request_id,
            ok: false,
            message: "local control request timed out".to_string(),
        },
    };
    let status = if result.ok { 200 } else { 409 };
    json_response(stream, status, &result).await
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
        "/icon.svg" => Some(StaticAsset {
            body: ICON_SVG,
            content_type: "image/svg+xml",
        }),
        "/app-icon.png" => Some(StaticAsset {
            body: APP_ICON_PNG,
            content_type: "image/png",
        }),
        "/app-icon-192.png" => Some(StaticAsset {
            body: APP_ICON_192_PNG,
            content_type: "image/png",
        }),
        "/cat-sprites.png" => Some(StaticAsset {
            body: CAT_SPRITES_PNG,
            content_type: "image/png",
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
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
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

async fn not_modified_response(stream: &mut TcpStream, cursor: u64) -> Result<(), BoxError> {
    let header = format!(
        "HTTP/1.1 304 Not Modified\r\nX-Codex-Monitor-Cursor: {cursor}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(header.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn unauthorized_response(stream: &mut TcpStream) -> Result<(), BoxError> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": "read token required" }))?;
    let header = format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nWWW-Authenticate: Bearer\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn control_unauthorized_response(stream: &mut TcpStream) -> Result<(), BoxError> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": "control token required" }))?;
    let header = format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nWWW-Authenticate: Bearer\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn preflight_response(stream: &mut TcpStream) -> Result<(), BoxError> {
    stream
        .write_all(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PATCH, OPTIONS\r\nAccess-Control-Allow-Headers: Accept, Authorization, Content-Type, X-Codex-Monitor-Token, X-Codex-Monitor-Control-Token\r\nAccess-Control-Max-Age: 600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
    stream.shutdown().await?;
    Ok(())
}

fn authorized(request: &str, expected: &str) -> bool {
    request
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            let value = value.trim();
            (name.eq_ignore_ascii_case("authorization")
                && value
                    .strip_prefix("Bearer ")
                    .is_some_and(|token| token == expected))
                || (name.eq_ignore_ascii_case("x-codex-monitor-token") && value == expected)
                || (name.eq_ignore_ascii_case("x-codex-monitor-control-token") && value == expected)
        })
}

fn parse_since(query: &str) -> Result<Option<u64>, BoxError> {
    let Some(value) = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(key, value)| (key == "since").then_some(value))
    else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "invalid since cursor".into())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{parse_since, static_asset, valid_alias};
    use crate::discovery::DiscoveryCache;
    use crate::registry::Registry;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{mpsc, Mutex, RwLock};

    async fn raw_request(
        request: &str,
        registry: Arc<RwLock<Registry>>,
        read_token: Option<&str>,
        control_token: Option<&str>,
        alias_path: &Path,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request = request.as_bytes().to_vec();
        let alias_path = alias_path.to_path_buf();
        let read_token = read_token.map(str::to_string);
        let control_token = control_token.map(str::to_string);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (control_tx, _control_rx) = mpsc::channel(1);
            let (pair_tx, _pair_rx) = mpsc::channel(1);
            super::handle(
                stream,
                registry,
                read_token.as_deref(),
                control_token.as_deref(),
                &control_tx,
                Arc::new(Mutex::new(DiscoveryCache::default())),
                "test-network",
                &pair_tx,
                Arc::new(RwLock::new("old".to_string())),
                &alias_path,
                "Ab3dE7qK",
            )
            .await
            .unwrap();
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap();
        String::from_utf8(response).unwrap()
    }

    #[test]
    fn embeds_pwa_assets_without_overlapping_api_routes() {
        for path in [
            "/",
            "/index.html",
            "/app.js",
            "/styles.css",
            "/manifest.webmanifest",
            "/sw.js",
            "/icon.svg",
            "/app-icon.png",
            "/app-icon-192.png",
            "/cat-sprites.png",
        ] {
            let asset = static_asset(path).expect("static asset");
            assert!(!asset.body.is_empty());
        }
        assert_eq!(
            static_asset("/cat-sprites.png").unwrap().content_type,
            "image/png"
        );
        assert!(static_asset("/api/state").is_none());
        let small_icon = static_asset("/app-icon-192.png").unwrap();
        assert_eq!(small_icon.content_type, "image/png");
        assert_eq!(&small_icon.body[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            u32::from_be_bytes(small_icon.body[16..20].try_into().unwrap()),
            192
        );
        assert_eq!(
            u32::from_be_bytes(small_icon.body[20..24].try_into().unwrap()),
            192
        );
    }

    #[test]
    fn parses_optional_state_cursor() {
        assert_eq!(parse_since("foo=bar").unwrap(), None);
        assert_eq!(parse_since("since=42").unwrap(), Some(42));
        assert_eq!(parse_since("foo=bar&since=7").unwrap(), Some(7));
        assert!(parse_since("since=nope").is_err());
    }

    #[test]
    fn accepts_read_token_headers() {
        assert!(super::authorized(
            "GET /api/v1/state HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n",
            "secret"
        ));
        assert!(super::authorized(
            "GET /api/v1/state HTTP/1.1\r\nX-Codex-Monitor-Token: secret\r\n\r\n",
            "secret"
        ));
        assert!(!super::authorized(
            "GET /api/v1/state HTTP/1.1\r\nAuthorization: Bearer wrong\r\n\r\n",
            "secret"
        ));
    }

    #[test]
    fn alias_validation_rejects_empty_long_and_control_text() {
        assert!(valid_alias("Cozy Otter abcd"));
        assert!(!valid_alias(""));
        assert!(!valid_alias(&"x".repeat(65)));
        assert!(!valid_alias("bad\nname"));
    }

    #[tokio::test]
    async fn management_routes_enforce_control_token_and_persist_alias() {
        let root = std::env::temp_dir().join(format!(
            "codex-monitor-http-auth-test-{}-{}",
            std::process::id(),
            super::unix_seconds()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let alias_path = root.join("monitor.alias");
        let registry = Arc::new(RwLock::new(Registry::default()));
        registry.write().await.set_identity(
            "node-stable".into(),
            "old".into(),
            Some("test-network".into()),
            vec![],
        );

        let discoveries = raw_request(
            "GET /api/v1/discoveries HTTP/1.1\r\nAuthorization: Bearer read\r\n\r\n",
            Arc::clone(&registry),
            Some("read"),
            Some("control"),
            &alias_path,
        )
        .await;
        assert!(discoveries.starts_with("HTTP/1.1 200"));

        for request in [
            "POST /api/v1/pair HTTP/1.1\r\nAuthorization: Bearer read\r\nContent-Length: 2\r\n\r\n{}",
            "PATCH /api/v1/identity HTTP/1.1\r\nAuthorization: Bearer read\r\nContent-Length: 2\r\n\r\n{}",
            "GET /api/v1/pairing-code HTTP/1.1\r\nAuthorization: Bearer read\r\n\r\n",
        ] {
            let response = raw_request(request, Arc::clone(&registry), Some("read"), Some("control"), &alias_path).await;
            assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        }
        let disabled = raw_request(
            "GET /api/v1/pairing-code HTTP/1.1\r\n\r\n",
            Arc::clone(&registry),
            Some("read"),
            None,
            &alias_path,
        )
        .await;
        assert!(disabled.starts_with("HTTP/1.1 403"));
        let code = raw_request(
            "GET /api/v1/pairing-code HTTP/1.1\r\nAuthorization: Bearer control\r\n\r\n",
            Arc::clone(&registry),
            Some("read"),
            Some("control"),
            &alias_path,
        )
        .await;
        assert!(code.starts_with("HTTP/1.1 200"));
        assert!(code.contains(r#"{"pairingCode":"Ab3dE7qK"}"#));

        let invalid_pair_body = r#"{"nodeId":"node-a","pairingCode":"246810"}"#;
        let invalid_pair = raw_request(
            &format!("POST /api/v1/pair HTTP/1.1\r\nAuthorization: Bearer control\r\nContent-Length: {}\r\n\r\n{invalid_pair_body}", invalid_pair_body.len()),
            Arc::clone(&registry), Some("read"), Some("control"), &alias_path,
        ).await;
        assert!(invalid_pair.starts_with("HTTP/1.1 400"), "{invalid_pair}");
        assert!(invalid_pair.contains("pairingCode must be exactly 8 ASCII letters and digits"));

        let body = r#"{"alias":"Cozy Otter abcd"}"#;
        let rename = raw_request(
            &format!("PATCH /api/v1/identity HTTP/1.1\r\nAuthorization: Bearer control\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
            Arc::clone(&registry), Some("read"), Some("control"), &alias_path,
        ).await;
        assert!(rename.starts_with("HTTP/1.1 200"), "{rename}");
        assert!(rename.contains(r#""nodeId":"node-stable""#));
        assert_eq!(
            std::fs::read_to_string(&alias_path).unwrap(),
            "Cozy Otter abcd"
        );
        assert_eq!(
            registry.read().await.identity().node_id.as_deref(),
            Some("node-stable")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
