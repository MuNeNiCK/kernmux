//! Bounded HTTP/1.1 transport over a local Unix domain socket.

use std::{
    convert::Infallible,
    future::Future,
    os::unix::fs::MetadataExt,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, Response as HttpResponse, StatusCode, Uri,
    body::Incoming,
    header::{CONNECTION, UPGRADE},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kernmux_api::v1::{ApiError, ErrorCode, Response};
use tokio::net::{UnixListener, UnixStream};

use crate::security::{
    AuditAction, AuditDecision, AuditEvent, AuthorizationPolicy, LimitKind, PeerIdentity,
    RequestClass, ServiceLimiter,
};

/// Default maximum JSON request body accepted by the local API.
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Authorized route metadata resolved before request body processing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteSecurity {
    pub class: RequestClass,
    pub audit_action: AuditAction,
}

/// Fully buffered and bounded request passed to the API dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalRequest {
    pub method: Method,
    pub uri: Uri,
    pub request_id: Option<String>,
    pub body: Vec<u8>,
}

/// Transport-independent response returned by the API dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalResponse {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// Prepared handoff from authenticated HTTP to a binary local protocol.
pub struct LocalUpgrade {
    protocol: &'static str,
    handler: Box<dyn FnOnce(std::os::unix::net::UnixStream) + Send>,
}

impl LocalUpgrade {
    /// Creates an upgrade with its negotiated token and blocking stream task.
    #[must_use]
    pub fn new(
        protocol: &'static str,
        handler: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
    ) -> Self {
        Self {
            protocol,
            handler: Box::new(handler),
        }
    }
}

impl LocalResponse {
    /// Creates a JSON response from a serializable API value.
    ///
    /// # Errors
    ///
    /// Returns an internal API error when serialization fails.
    pub fn json<T: serde::Serialize>(status: StatusCode, value: &T) -> Result<Self, ApiError> {
        serde_json::to_vec(value)
            .map(|body| Self {
                status,
                content_type: "application/json",
                body,
            })
            .map_err(|_| internal_error("response serialization failed"))
    }

    /// Creates a typed JSON error envelope with the matching HTTP status.
    #[must_use]
    pub fn api_error(error: ApiError) -> Self {
        let status = status_for_error(error.code);
        Self::json(status, &Response::<()>::Error { error }).unwrap_or_else(|_| Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            content_type: "application/json",
            body: br#"{"kind":"error","error":{"code":"internal","message":"response serialization failed","retryable":false}}"#.to_vec(),
        })
    }
}

/// Product API dispatcher behind the authenticated local transport.
pub trait LocalApi: Send + Sync + 'static {
    /// Resolves route security without consuming untrusted request content.
    fn route(&self, method: &Method, path: &str) -> Option<RouteSecurity>;

    /// Handles one authenticated and authorized bounded request.
    fn handle(&self, request: LocalRequest, peer: &PeerIdentity) -> LocalResponse;

    /// Prepares a separately authorized binary protocol upgrade.
    ///
    /// # Errors
    ///
    /// Returns a typed API error when the route cannot be upgraded.
    fn prepare_upgrade(
        &self,
        _request: LocalRequest,
        _peer: &PeerIdentity,
    ) -> Result<LocalUpgrade, ApiError> {
        Err(api_error(
            ErrorCode::Unsupported,
            "binary protocol upgrade is not supported",
            false,
        ))
    }
}

/// Configuration for one local HTTP listener.
#[derive(Clone, Debug)]
pub struct TransportConfig {
    pub socket_path: PathBuf,
    pub socket_mode: u32,
    pub max_request_bytes: usize,
    pub max_header_bytes: usize,
}

impl TransportConfig {
    /// Secure defaults for a system-managed runtime directory.
    #[must_use]
    pub fn system_default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/kernmux/kernmuxd.sock"),
            socket_mode: 0o660,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_header_bytes: 32 * 1024,
        }
    }

    fn validate(&self) -> Result<(), ApiError> {
        if self.max_request_bytes == 0
            || self.max_header_bytes < 8192
            || self.socket_mode & !0o777 != 0
        {
            return Err(internal_error("local transport configuration is invalid"));
        }
        Ok(())
    }
}

/// Bound local service with peer authorization and bounded admission.
pub struct LocalHttpServer<A> {
    listener: UnixListener,
    config: TransportConfig,
    policy: AuthorizationPolicy,
    limiter: ServiceLimiter,
    api: Arc<A>,
    socket_identity: (u64, u64),
}

impl<A> std::fmt::Debug for LocalHttpServer<A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalHttpServer")
            .field("socket_path", &self.config.socket_path)
            .field("max_request_bytes", &self.config.max_request_bytes)
            .field("max_header_bytes", &self.config.max_header_bytes)
            .finish_non_exhaustive()
    }
}

impl<A> LocalHttpServer<A>
where
    A: LocalApi,
{
    /// Binds a new Unix socket without replacing an existing path.
    ///
    /// The parent runtime directory must already exist with ownership managed
    /// by the service manager. Refusing to unlink an existing path avoids
    /// disrupting a live daemon.
    ///
    /// # Errors
    ///
    /// Returns an I/O or configuration error when the listener cannot be made
    /// ready securely.
    pub fn bind(
        config: TransportConfig,
        policy: AuthorizationPolicy,
        limiter: ServiceLimiter,
        api: Arc<A>,
    ) -> Result<Self, TransportError> {
        config.validate().map_err(TransportError::Configuration)?;
        let listener = UnixListener::bind(&config.socket_path).map_err(TransportError::Bind)?;
        std::fs::set_permissions(
            &config.socket_path,
            std::fs::Permissions::from_mode(config.socket_mode),
        )
        .map_err(|error| {
            let _ = std::fs::remove_file(&config.socket_path);
            TransportError::Permissions(error)
        })?;
        let metadata = std::fs::metadata(&config.socket_path).map_err(|error| {
            let _ = std::fs::remove_file(&config.socket_path);
            TransportError::Permissions(error)
        })?;
        Ok(Self {
            listener,
            config,
            policy,
            limiter,
            api,
            socket_identity: (metadata.dev(), metadata.ino()),
        })
    }

    /// Socket path owned by this listener.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.config.socket_path
    }

    /// Serves connections until shutdown resolves.
    ///
    /// # Errors
    ///
    /// Returns when accepting from the local listener fails.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), TransportError> {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => return Ok(()),
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted.map_err(TransportError::Accept)?;
                    self.accept(stream);
                }
            }
        }
    }

    fn accept(&self, stream: UnixStream) {
        let Ok(permit) = self.limiter.acquire(LimitKind::Connection) else {
            return;
        };
        let Ok(peer) = PeerIdentity::from_socket(&stream) else {
            return;
        };
        let api = Arc::clone(&self.api);
        let policy = self.policy.clone();
        let max_request_bytes = self.config.max_request_bytes;
        let max_header_bytes = self.config.max_header_bytes;
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                handle_request(
                    request,
                    Arc::clone(&api),
                    policy.clone(),
                    peer.clone(),
                    max_request_bytes,
                )
            });
            let mut builder = http1::Builder::new();
            builder.max_buf_size(max_header_bytes);
            let _permit = permit;
            if let Err(error) = builder
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await
            {
                tracing::debug!(%error, "local HTTP connection closed with protocol error");
            }
        });
    }
}

impl<A> Drop for LocalHttpServer<A> {
    fn drop(&mut self) {
        let matches_bound_socket = std::fs::metadata(&self.config.socket_path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.socket_identity);
        if matches_bound_socket {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }
    }
}

async fn handle_request<A>(
    request: Request<Incoming>,
    api: Arc<A>,
    policy: AuthorizationPolicy,
    peer: PeerIdentity,
    max_request_bytes: usize,
) -> Result<HttpResponse<Full<Bytes>>, Infallible>
where
    A: LocalApi,
{
    let Some(route) = api.route(request.method(), request.uri().path()) else {
        return Ok(error_response(
            StatusCode::NOT_FOUND,
            api_error(ErrorCode::NotFound, "API route was not found", false),
        ));
    };
    let request_id = match request_id(request.headers()) {
        Ok(request_id) => request_id,
        Err(error) => {
            audit(&peer, route, AuditDecision::Failed, None);
            return Ok(error_response(StatusCode::BAD_REQUEST, error));
        }
    };
    if let Err(error) = policy.authorize(&peer.actor, route.class) {
        audit(&peer, route, AuditDecision::Denied, request_id.as_deref());
        return Ok(error_response(StatusCode::FORBIDDEN, error));
    }
    if route.class == RequestClass::Console {
        return handle_upgrade_request(request, api, peer, route, request_id).await;
    }
    let (parts, body) = request.into_parts();
    let collected = Limited::new(body, max_request_bytes).collect().await;
    let body = if let Ok(body) = collected {
        body.to_bytes().to_vec()
    } else {
        audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
        return Ok(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            api_error(
                ErrorCode::InvalidRequest,
                "request body exceeds its limit",
                false,
            ),
        ));
    };
    let request = LocalRequest {
        method: parts.method,
        uri: parts.uri,
        request_id: request_id.clone(),
        body,
    };
    let peer_for_handler = peer.clone();
    let handled = tokio::task::spawn_blocking(move || api.handle(request, &peer_for_handler)).await;
    if let Ok(response) = handled {
        audit(&peer, route, AuditDecision::Allowed, request_id.as_deref());
        Ok(to_http_response(response))
    } else {
        audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
        Ok(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            internal_error("API request handler terminated unexpectedly"),
        ))
    }
}

async fn handle_upgrade_request<A>(
    mut request: Request<Incoming>,
    api: Arc<A>,
    peer: PeerIdentity,
    route: RouteSecurity,
    request_id: Option<String>,
) -> Result<HttpResponse<Full<Bytes>>, Infallible>
where
    A: LocalApi,
{
    let requested_protocol = request
        .headers()
        .get(UPGRADE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_owned);
    let connection_upgrades = request
        .headers()
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    if requested_protocol.is_none() || !connection_upgrades {
        audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
        return Ok(error_response(
            StatusCode::UPGRADE_REQUIRED,
            api_error(
                ErrorCode::InvalidRequest,
                "console route requires an HTTP protocol upgrade",
                false,
            ),
        ));
    }
    let on_upgrade = hyper::upgrade::on(&mut request);
    let local_request = LocalRequest {
        method: request.method().clone(),
        uri: request.uri().clone(),
        request_id: request_id.clone(),
        body: Vec::new(),
    };
    let peer_for_handler = peer.clone();
    let prepared =
        tokio::task::spawn_blocking(move || api.prepare_upgrade(local_request, &peer_for_handler))
            .await;
    let upgrade = match prepared {
        Ok(Ok(upgrade)) if requested_protocol.as_deref() == Some(upgrade.protocol) => upgrade,
        Ok(Ok(_)) => {
            audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
            return Ok(error_response(
                StatusCode::BAD_REQUEST,
                api_error(
                    ErrorCode::InvalidRequest,
                    "console upgrade protocol is unsupported",
                    false,
                ),
            ));
        }
        Ok(Err(error)) => {
            audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
            return Ok(error_response(status_for_error(error.code), error));
        }
        Err(_) => {
            audit(&peer, route, AuditDecision::Failed, request_id.as_deref());
            return Ok(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                internal_error("console attachment handler terminated unexpectedly"),
            ));
        }
    };
    audit(&peer, route, AuditDecision::Allowed, request_id.as_deref());
    let protocol = upgrade.protocol;
    tokio::spawn(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        let Ok(parts) = upgraded.downcast::<TokioIo<UnixStream>>() else {
            return;
        };
        if !parts.read_buf.is_empty() {
            return;
        }
        let Ok(stream) = parts.io.into_inner().into_std() else {
            return;
        };
        if stream.set_nonblocking(false).is_err() {
            return;
        }
        tokio::task::spawn_blocking(move || (upgrade.handler)(stream));
    });
    Ok(HttpResponse::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(CONNECTION, "upgrade")
        .header(UPGRADE, protocol)
        .body(Full::new(Bytes::new()))
        .expect("static upgrade response must be valid"))
}

fn audit(
    peer: &PeerIdentity,
    route: RouteSecurity,
    decision: AuditDecision,
    audit_id: Option<&str>,
) {
    AuditEvent {
        actor: peer.actor.clone(),
        peer_pid: peer.pid,
        action: route.audit_action,
        resource: None,
        decision,
        operation_id: None,
        audit_id: audit_id.map(str::to_owned),
    }
    .emit();
}

fn request_id(headers: &hyper::HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get("x-request-id") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .ok()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .ok_or_else(|| api_error(ErrorCode::InvalidRequest, "request ID is invalid", false))?;
    Ok(Some(value.to_owned()))
}

fn to_http_response(response: LocalResponse) -> HttpResponse<Full<Bytes>> {
    HttpResponse::builder()
        .status(response.status)
        .header(hyper::header::CONTENT_TYPE, response.content_type)
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(response.body)))
        .expect("static response headers must be valid")
}

fn error_response(status: StatusCode, error: ApiError) -> HttpResponse<Full<Bytes>> {
    let envelope = Response::<()>::Error { error };
    let response = LocalResponse::json(status, &envelope).unwrap_or_else(|_| LocalResponse {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        content_type: "application/json",
        body: br#"{"kind":"error","error":{"code":"internal","message":"response serialization failed","retryable":false}}"#.to_vec(),
    });
    to_http_response(response)
}

fn status_for_error(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::PreconditionFailed => StatusCode::PRECONDITION_FAILED,
        ErrorCode::Unsupported => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::BackendUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::Internal | ErrorCode::Unknown => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn api_error(code: ErrorCode, message: &str, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        current_generation: None,
        diagnostics: Vec::new(),
    }
}

fn internal_error(message: &str) -> ApiError {
    api_error(ErrorCode::Internal, message, false)
}

/// Failure to configure or operate the local HTTP listener.
#[derive(Debug)]
pub enum TransportError {
    Configuration(ApiError),
    Bind(std::io::Error),
    Permissions(std::io::Error),
    Accept(std::io::Error),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(error) => error.message.fmt(formatter),
            Self::Bind(error) => write!(formatter, "failed to bind local API socket: {error}"),
            Self::Permissions(error) => {
                write!(formatter, "failed to secure local API socket: {error}")
            }
            Self::Accept(error) => write!(formatter, "failed to accept local API peer: {error}"),
        }
    }
}

impl std::error::Error for TransportError {}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream as StdUnixStream,
    };

    use kernmux_api::v1::Generation;
    use tempfile::tempdir;
    use tokio::sync::oneshot;

    use crate::security::{AuthorizationPolicy, ServiceLimits};

    use super::*;

    #[derive(Debug)]
    struct TestApi;

    impl LocalApi for TestApi {
        fn route(&self, method: &Method, path: &str) -> Option<RouteSecurity> {
            match (method, path) {
                (&Method::GET, "/1.0") => Some(RouteSecurity {
                    class: RequestClass::ReadOnly,
                    audit_action: AuditAction::ReadInventory,
                }),
                (&Method::POST, "/1.0/mutate") => Some(RouteSecurity {
                    class: RequestClass::Mutation,
                    audit_action: AuditAction::MutateLifecycle,
                }),
                (&Method::POST, "/1.0/console") => Some(RouteSecurity {
                    class: RequestClass::Console,
                    audit_action: AuditAction::AttachConsole,
                }),
                _ => None,
            }
        }

        fn handle(&self, request: LocalRequest, _peer: &PeerIdentity) -> LocalResponse {
            LocalResponse::json(
                StatusCode::OK,
                &Response::Result {
                    generation: Generation(1),
                    data: request.body.len(),
                },
            )
            .unwrap()
        }

        fn prepare_upgrade(
            &self,
            _request: LocalRequest,
            _peer: &PeerIdentity,
        ) -> Result<LocalUpgrade, ApiError> {
            Ok(LocalUpgrade::new("test-binary-v1", |mut stream| {
                stream.write_all(b"binary-ready").unwrap();
            }))
        }
    }

    #[test]
    fn request_ids_are_bounded_log_safe_tokens() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("x-request-id", "request-1_ok.example".parse().unwrap());
        assert_eq!(
            request_id(&headers).unwrap().as_deref(),
            Some("request-1_ok.example")
        );
        headers.insert("x-request-id", "bad value".parse().unwrap());
        assert!(request_id(&headers).is_err());
        headers.insert("x-request-id", "x".repeat(129).parse().unwrap());
        assert!(request_id(&headers).is_err());
    }

    #[tokio::test]
    async fn serves_bounded_http_and_refuses_to_replace_existing_socket() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("api.sock");
        let config = TransportConfig {
            socket_path: socket.clone(),
            socket_mode: 0o600,
            max_request_bytes: 4,
            max_header_bytes: 8192,
        };
        let limiter = ServiceLimiter::new(ServiceLimits {
            connections: 2,
            mutations: 1,
            consoles: 1,
        })
        .unwrap();
        let server = LocalHttpServer::bind(
            config.clone(),
            AuthorizationPolicy::deny_by_default().with_unprivileged_reads(true),
            limiter.clone(),
            Arc::new(TestApi),
        )
        .unwrap();
        assert!(
            LocalHttpServer::bind(
                config,
                AuthorizationPolicy::deny_by_default(),
                limiter,
                Arc::new(TestApi),
            )
            .is_err()
        );

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(server.run(async move {
            let _ = shutdown_rx.await;
        }));
        let response = tokio::task::spawn_blocking(move || {
            let mut client = StdUnixStream::connect(socket).unwrap();
            client
                .write_all(b"GET /1.0 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            response
        })
        .await
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"generation\":1"));

        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn oversized_body_is_rejected_before_dispatch() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("api.sock");
        let server = LocalHttpServer::bind(
            TransportConfig {
                socket_path: socket.clone(),
                socket_mode: 0o600,
                max_request_bytes: 4,
                max_header_bytes: 8192,
            },
            AuthorizationPolicy::deny_by_default()
                .with_operator_uid(rustix::process::getuid().as_raw()),
            ServiceLimiter::new(ServiceLimits {
                connections: 1,
                mutations: 1,
                consoles: 1,
            })
            .unwrap(),
            Arc::new(TestApi),
        )
        .unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(server.run(async move {
            let _ = shutdown_rx.await;
        }));
        let response = tokio::task::spawn_blocking(move || {
            let mut client = StdUnixStream::connect(socket).unwrap();
            client
                .write_all(b"POST /1.0/mutate HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345")
                .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            response
        })
        .await
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large"));

        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn hands_an_authorized_http_upgrade_to_a_binary_stream() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("api.sock");
        let server = LocalHttpServer::bind(
            TransportConfig {
                socket_path: socket.clone(),
                socket_mode: 0o600,
                max_request_bytes: 4,
                max_header_bytes: 8192,
            },
            AuthorizationPolicy::deny_by_default()
                .with_operator_uid(rustix::process::getuid().as_raw()),
            ServiceLimiter::new(ServiceLimits {
                connections: 1,
                mutations: 1,
                consoles: 1,
            })
            .unwrap(),
            Arc::new(TestApi),
        )
        .unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(server.run(async move {
            let _ = shutdown_rx.await;
        }));
        let response = tokio::task::spawn_blocking(move || {
            let mut client = StdUnixStream::connect(socket).unwrap();
            client
                .write_all(b"POST /1.0/console HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: test-binary-v1\r\n\r\n")
                .unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).unwrap();
            response
        })
        .await
        .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 101 Switching Protocols"));
        assert!(response.ends_with(b"binary-ready"));

        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}
