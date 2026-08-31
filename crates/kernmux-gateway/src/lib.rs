//! Authenticated, unprivileged browser gateway for the local management API.

use std::{
    collections::BTreeSet,
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request as HttpRequest, Response as HttpResponse, StatusCode,
    body::Incoming,
    header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kernmux_client::{Request, Transport};
use tokio::{net::TcpListener, sync::Semaphore};

const DEFAULT_MAX_REQUEST_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_STATIC_BYTES: u64 = 32 * 1024 * 1024;
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct GatewayConfig {
    pub bind: SocketAddr,
    pub allow_non_loopback: bool,
    pub bearer_token: String,
    pub allowed_origins: BTreeSet<String>,
    pub assets_dir: PathBuf,
    pub max_request_bytes: usize,
    pub max_connections: usize,
}

impl std::fmt::Debug for GatewayConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewayConfig")
            .field("bind", &self.bind)
            .field("allow_non_loopback", &self.allow_non_loopback)
            .field("bearer_token", &"[REDACTED]")
            .field("allowed_origins", &self.allowed_origins)
            .field("assets_dir", &self.assets_dir)
            .field("max_request_bytes", &self.max_request_bytes)
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

impl GatewayConfig {
    #[must_use]
    pub fn loopback(bearer_token: String, assets_dir: PathBuf) -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 9443)),
            allow_non_loopback: false,
            bearer_token,
            allowed_origins: BTreeSet::from([
                "http://127.0.0.1:9443".into(),
                "http://localhost:9443".into(),
            ]),
            assets_dir,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_connections: 64,
        }
    }

    /// # Errors
    /// Rejects weak credentials, empty origin policy, invalid limits, or an
    /// accidental non-loopback listener.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.bind.ip().is_loopback() && !self.allow_non_loopback {
            return Err(ConfigError("non-loopback bind requires explicit opt-in"));
        }
        if self.bearer_token.len() < 32
            || self.bearer_token.len() > 512
            || self.bearer_token.chars().any(char::is_whitespace)
        {
            return Err(ConfigError(
                "bearer credential must be 32-512 non-whitespace bytes",
            ));
        }
        if self.allowed_origins.is_empty()
            || self.allowed_origins.iter().any(|origin| {
                origin == "*"
                    || !(origin.starts_with("http://") || origin.starts_with("https://"))
                    || origin.ends_with('/')
            })
        {
            return Err(ConfigError("allowed origins must be explicit HTTP origins"));
        }
        if self.max_request_bytes == 0 || self.max_connections == 0 {
            return Err(ConfigError("gateway limits must be nonzero"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigError(pub &'static str);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone)]
pub struct Gateway<T> {
    config: Arc<GatewayConfig>,
    transport: T,
    permits: Arc<Semaphore>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Gateway<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gateway")
            .field("bind", &self.config.bind)
            .field("assets_dir", &self.config.assets_dir)
            .finish_non_exhaustive()
    }
}

impl<T> Gateway<T>
where
    T: Transport + Clone + Send + Sync + 'static,
{
    /// # Errors
    /// Returns an invalid configuration without opening a listener.
    pub fn new(config: GatewayConfig, transport: T) -> Result<Self, ConfigError> {
        config.validate()?;
        let permits = Arc::new(Semaphore::new(config.max_connections));
        Ok(Self {
            config: Arc::new(config),
            transport,
            permits,
        })
    }

    /// # Errors
    /// Returns when accepting a TCP connection fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), std::io::Error> {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(()),
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let gateway = self.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request| gateway.clone().handle(request));
                        let _ = http1::Builder::new()
                            .keep_alive(false)
                            .max_buf_size(32 * 1024)
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        }
    }

    async fn handle(
        self,
        request: HttpRequest<Incoming>,
    ) -> Result<HttpResponse<Full<Bytes>>, Infallible> {
        let Ok(_permit) = self.permits.clone().try_acquire_owned() else {
            return Ok(error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "gateway_busy",
            ));
        };
        if request.uri().path() == "/healthz" && request.method() == Method::GET {
            return Ok(text_response(
                StatusCode::OK,
                "ok\n",
                "text/plain; charset=utf-8",
            ));
        }
        if request.uri().path().starts_with("/api/") {
            return Ok(self.handle_api(request).await);
        }
        Ok(self.handle_static(request).await)
    }

    async fn handle_api(&self, request: HttpRequest<Incoming>) -> HttpResponse<Full<Bytes>> {
        if !authorized(
            request.headers().get(AUTHORIZATION),
            &self.config.bearer_token,
        ) {
            return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
        }
        let mutation = request.method() != Method::GET;
        let origin = request
            .headers()
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok());
        if origin.is_some_and(|value| !self.config.allowed_origins.contains(value))
            || (mutation && origin.is_none())
        {
            return error_response(StatusCode::FORBIDDEN, "origin_forbidden");
        }
        if request.headers().contains_key(hyper::header::UPGRADE) {
            return error_response(StatusCode::NOT_IMPLEMENTED, "console_unavailable");
        }
        let Some(path) = daemon_path(request.method(), request.uri()) else {
            return error_response(StatusCode::NOT_FOUND, "route_not_found");
        };
        let method = request.method().clone();
        let content_type_is_json = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
            });
        if request
            .headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.config.max_request_bytes)
        {
            return error_response(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large");
        }
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_request_id(value))
            .map_or_else(generated_request_id, str::to_owned);
        let body = match Limited::new(request.into_body(), self.config.max_request_bytes)
            .collect()
            .await
        {
            Ok(body) => body.to_bytes().to_vec(),
            Err(_) => return error_response(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"),
        };
        if !body.is_empty() && method == Method::GET {
            return error_response(StatusCode::BAD_REQUEST, "unexpected_request_body");
        }
        if !body.is_empty() && !content_type_is_json {
            return error_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type_required");
        }
        let Some(method) = method_name(&method) else {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        };
        let transport = self.transport.clone();
        let forwarded = Request { method, path, body };
        match tokio::task::spawn_blocking(move || transport.execute(&request_id, &forwarded)).await
        {
            Ok(Ok(response)) => json_bytes_response(response.status, &response.value),
            Ok(Err(_)) => error_response(StatusCode::BAD_GATEWAY, "daemon_unavailable"),
            Err(_) => error_response(StatusCode::BAD_GATEWAY, "gateway_failure"),
        }
    }

    async fn handle_static(&self, request: HttpRequest<Incoming>) -> HttpResponse<Full<Bytes>> {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        }
        let (relative, content_type) = match request.uri().path() {
            "/" | "/index.html" => ("index.html", "text/html; charset=utf-8"),
            "/bootstrap.js" => ("bootstrap.js", "text/javascript; charset=utf-8"),
            "/app.js" => ("app.js", "text/javascript; charset=utf-8"),
            "/app_bg.wasm" => ("app_bg.wasm", "application/wasm"),
            _ => return error_response(StatusCode::NOT_FOUND, "not_found"),
        };
        let path = self.config.assets_dir.join(relative);
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.is_file() && metadata.len() <= DEFAULT_MAX_STATIC_BYTES => {
                metadata
            }
            _ => return error_response(StatusCode::NOT_FOUND, "not_found"),
        };
        let body = if request.method() == Method::HEAD {
            Vec::new()
        } else {
            match tokio::fs::read(&path).await {
                Ok(bytes) if bytes.len() as u64 == metadata.len() => bytes,
                _ => return error_response(StatusCode::NOT_FOUND, "not_found"),
            }
        };
        secure_response(StatusCode::OK, body, content_type)
    }
}

fn daemon_path(method: &Method, uri: &hyper::Uri) -> Option<String> {
    let path = uri.path().strip_prefix("/api")?;
    if !path.starts_with("/1.0") || path.contains("//") || path.contains("..") {
        return None;
    }
    if let Some(query) = uri.query()
        && (path != "/1.0/events"
            || !query.strip_prefix("after=").is_some_and(|value| {
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
            }))
    {
        return None;
    }
    let segments = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    let allowed = match segments.as_slice() {
        ["1.0"] | ["1.0", "compatibility" | "operations" | "events"] => *method == Method::GET,
        ["1.0", "resource-pool"] => matches!(*method, Method::GET | Method::PUT),
        ["1.0", "instances"] => matches!(*method, Method::GET | Method::POST),
        ["1.0", "instances", id] => {
            valid_number(id) && matches!(*method, Method::GET | Method::PATCH | Method::DELETE)
        }
        ["1.0", "instances", id, action] => {
            valid_number(id)
                && matches!(*method, Method::POST)
                && matches!(*action, "load" | "load-image" | "start" | "stop" | "unload")
        }
        ["1.0", "images"] => matches!(*method, Method::GET | Method::POST),
        ["1.0", "images", kind, id] => {
            *method == Method::GET && matches!(*kind, "kernel" | "initrd") && valid_token(id, 128)
        }
        ["1.0", "operations", id] => {
            valid_token(id, 128) && matches!(*method, Method::GET | Method::DELETE)
        }
        _ => false,
    };
    allowed.then(|| {
        uri.query()
            .map_or_else(|| path.to_owned(), |query| format!("{path}?{query}"))
    })
}

fn method_name(method: &Method) -> Option<&'static str> {
    match *method {
        Method::GET => Some("GET"),
        Method::POST => Some("POST"),
        Method::PUT => Some("PUT"),
        Method::PATCH => Some("PATCH"),
        Method::DELETE => Some("DELETE"),
        _ => None,
    }
}

fn authorized(header: Option<&hyper::header::HeaderValue>, expected: &str) -> bool {
    let supplied = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    constant_time_equal(supplied.as_bytes(), expected.as_bytes())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..512 {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn valid_number(value: &str) -> bool {
    !value.is_empty() && value.len() <= 10 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn valid_request_id(value: &str) -> bool {
    valid_token(value, 128)
}

fn generated_request_id() -> String {
    format!(
        "gateway-{}-{}",
        std::process::id(),
        REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn json_bytes_response(status: u16, value: &serde_json::Value) -> HttpResponse<Full<Bytes>> {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    match serde_json::to_vec(&value) {
        Ok(bytes) => secure_response(status, bytes, "application/json"),
        Err(_) => error_response(StatusCode::BAD_GATEWAY, "invalid_daemon_response"),
    }
}

fn error_response(status: StatusCode, code: &str) -> HttpResponse<Full<Bytes>> {
    let body = serde_json::to_vec(&serde_json::json!({
        "kind": "error",
        "error": { "code": code, "message": status.canonical_reason().unwrap_or("request failed"), "retryable": false }
    })).unwrap_or_default();
    secure_response(status, body, "application/json")
}

fn text_response(
    status: StatusCode,
    body: &str,
    content_type: &'static str,
) -> HttpResponse<Full<Bytes>> {
    secure_response(status, body.as_bytes().to_vec(), content_type)
}

fn secure_response(
    status: StatusCode,
    body: Vec<u8>,
    content_type: &'static str,
) -> HttpResponse<Full<Bytes>> {
    HttpResponse::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header("cache-control", "no-store")
        .header("content-security-policy", "default-src 'self'; connect-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")
        .header("referrer-policy", "no-referrer")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| HttpResponse::new(Full::new(Bytes::new())))
}

/// # Errors
/// Rejects non-regular, group/world-accessible, unreadable, or weak secrets.
pub fn read_token_file(path: &Path) -> Result<String, std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "token file must be an owner-only regular file",
        ));
    }
    let token = std::fs::read_to_string(path)?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    if token.len() < 32 || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "token file contains an invalid credential",
        ));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        sync::Mutex,
    };

    use kernmux_client::{ClientError, RawResponse};

    use super::*;

    #[derive(Clone, Default)]
    struct MockTransport {
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Transport for MockTransport {
        fn execute(
            &self,
            _request_id: &str,
            request: &Request,
        ) -> Result<RawResponse, ClientError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(RawResponse {
                status: 200,
                value: serde_json::json!({"kind":"result","generation":3,"data":{}}),
            })
        }
    }

    fn config() -> GatewayConfig {
        GatewayConfig::loopback("a".repeat(48), PathBuf::from("/nonexistent"))
    }

    #[test]
    fn defaults_are_loopback_and_non_loopback_requires_opt_in() {
        assert!(config().validate().is_ok());
        let mut exposed = config();
        exposed.bind = SocketAddr::from(([0, 0, 0, 0], 9443));
        assert_eq!(
            exposed.validate(),
            Err(ConfigError("non-loopback bind requires explicit opt-in"))
        );
        exposed.allow_non_loopback = true;
        assert!(exposed.validate().is_ok());
    }

    #[test]
    fn authorization_and_origin_policy_fail_closed() {
        let token = "x".repeat(48);
        let valid = hyper::header::HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
        assert!(authorized(Some(&valid), &token));
        assert!(!authorized(None, &token));
        assert!(!authorized(Some(&valid), &format!("{token}x")));
        let mut wildcard = config();
        wildcard.allowed_origins = BTreeSet::from(["*".into()]);
        assert!(wildcard.validate().is_err());
    }

    #[test]
    fn route_allowlist_excludes_console_and_unknown_paths() {
        assert_eq!(
            daemon_path(&Method::GET, &"/api/1.0/events?after=1".parse().unwrap()),
            Some("/1.0/events?after=1".into())
        );
        assert!(daemon_path(&Method::GET, &"/api/1.0?x=1".parse().unwrap()).is_none());
        assert!(
            daemon_path(
                &Method::POST,
                &"/api/1.0/instances/4/start".parse().unwrap()
            )
            .is_some()
        );
        assert!(
            daemon_path(
                &Method::GET,
                &"/api/1.0/instances/4/console".parse().unwrap()
            )
            .is_none()
        );
        assert!(daemon_path(&Method::GET, &"/api/1.0/../secret".parse().unwrap()).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_boundary_enforces_auth_origin_size_and_routes() {
        let transport = MockTransport::default();
        let observed = transport.requests.clone();
        let mut gateway_config = config();
        gateway_config.max_request_bytes = 8;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let gateway = Gateway::new(gateway_config, transport).unwrap();
        let server = tokio::spawn(gateway.serve(listener, async {
            let _ = stop_rx.await;
        }));

        assert_status(
            &exchange(
                address,
                "GET /api/1.0 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ),
            401,
        );
        assert_status(
            &exchange(
                address,
                &authorized_request("GET", "/api/1.0", Some("http://evil.invalid"), ""),
            ),
            403,
        );
        assert_status(
            &exchange(
                address,
                &authorized_request("GET", "/api/1.0/instances/4/console", None, ""),
            ),
            404,
        );
        assert_status(
            &exchange(
                address,
                &authorized_request(
                    "POST",
                    "/api/1.0/instances",
                    Some("http://127.0.0.1:9443"),
                    "123456789",
                ),
            ),
            413,
        );
        assert_status(
            &exchange(address, &authorized_request("GET", "/api/1.0", None, "")),
            200,
        );
        assert_status(
            &exchange(
                address,
                &authorized_request(
                    "POST",
                    "/api/1.0/instances",
                    Some("http://127.0.0.1:9443"),
                    "{}",
                ),
            ),
            200,
        );

        {
            let requests = observed.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].path, "/1.0");
            assert_eq!(requests[1].method, "POST");
            assert_eq!(requests[1].body, b"{}");
        }
        let _ = stop_tx.send(());
        server.await.unwrap().unwrap();
    }

    fn authorized_request(method: &str, path: &str, origin: Option<&str>, body: &str) -> String {
        let origin = origin.map_or_else(String::new, |value| format!("Origin: {value}\r\n"));
        format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            "a".repeat(48),
            body.len(),
        )
    }

    fn exchange(address: SocketAddr, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    fn assert_status(response: &[u8], expected: u16) {
        let head = std::str::from_utf8(response).unwrap();
        assert!(head.starts_with(&format!("HTTP/1.1 {expected} ")), "{head}");
    }
}
