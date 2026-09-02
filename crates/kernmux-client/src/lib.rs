//! Reusable unprivileged client for the versioned Kernmux management API.

use std::{
    fmt,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use kernmux_api::v1::{
    ApiError, CreateInstanceMutation, EventPage, Generation, HostCompatibilityReport, HostSnapshot,
    ImageArtifact, ImportImageMutation, ImportOsImageMutation, Instance, InstanceId,
    InstanceLifecycleMutation, LoadInstanceMutation, LoadManagedImageMutation, Operation,
    OperationId, OperationState, OsImage, ResourcePool, ResourcePoolMutation, Response,
    StopInstanceMutation, StorageInventory, UpdateInstanceMutation,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

const DEFAULT_MAX_HEADER_BYTES: usize = 32 * 1024;
const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// One HTTP request to the local management API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Vec<u8>,
}

impl Request {
    /// Creates a bodyless GET request.
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            body: Vec::new(),
        }
    }

    /// Creates a JSON request.
    ///
    /// # Errors
    /// Returns a protocol error when the body cannot be serialized.
    pub fn json(
        method: &'static str,
        path: impl Into<String>,
        body: &impl Serialize,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            method,
            path: path.into(),
            body: serde_json::to_vec(body)
                .map_err(|_| ClientError::protocol("request serialization failed"))?,
        })
    }
}

/// Validated JSON response with its HTTP status retained for presentation.
#[derive(Clone, Debug, PartialEq)]
pub struct RawResponse {
    pub status: u16,
    pub value: Value,
}

/// Stable client-side failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Transport,
    Protocol,
    Rejected,
    OperationFailed,
    OperationCancelled,
    OperationIndeterminate,
    Timeout,
}

/// Failure before a successful typed management result is available.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientError {
    pub kind: ErrorKind,
    pub message: String,
    pub api_error: Option<Box<ApiError>>,
    pub operation: Option<Box<Operation>>,
}

impl ClientError {
    fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transport, message)
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }

    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            api_error: None,
            operation: None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ClientError {}

/// Injectable request transport used by native and gateway clients.
pub trait Transport {
    /// Executes one request and returns a bounded, validated JSON response.
    ///
    /// # Errors
    /// Returns a transport or protocol failure.
    fn execute(&self, request_id: &str, request: &Request) -> Result<RawResponse, ClientError>;
}

/// HTTP/1.1 transport over the daemon's Unix domain socket.
#[derive(Clone, Debug)]
pub struct UnixTransport {
    socket: PathBuf,
    io_timeout: Duration,
    max_header_bytes: usize,
    max_body_bytes: usize,
}

impl UnixTransport {
    #[must_use]
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            io_timeout: DEFAULT_IO_TIMEOUT,
            max_header_bytes: DEFAULT_MAX_HEADER_BYTES,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}

impl Transport for UnixTransport {
    fn execute(&self, request_id: &str, request: &Request) -> Result<RawResponse, ClientError> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|error| {
            ClientError::transport(format!(
                "failed to connect to {}: {error}",
                self.socket.display()
            ))
        })?;
        stream
            .set_read_timeout(Some(self.io_timeout))
            .map_err(|error| ClientError::transport(error.to_string()))?;
        stream
            .set_write_timeout(Some(self.io_timeout))
            .map_err(|error| ClientError::transport(error.to_string()))?;
        let headers = format!(
            "{} {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Request-Id: {}\r\n\r\n",
            request.method,
            request.path,
            request.body.len(),
            request_id,
        );
        stream
            .write_all(headers.as_bytes())
            .and_then(|()| stream.write_all(&request.body))
            .map_err(|error| ClientError::transport(format!("failed to send request: {error}")))?;
        read_response(&mut stream, self.max_header_bytes, self.max_body_bytes)
    }
}

/// Polling bounds for asynchronous host operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitPolicy {
    pub max_attempts: u32,
    pub interval: Duration,
}

impl Default for WaitPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 120,
            interval: Duration::from_millis(500),
        }
    }
}

/// Typed management client independent of CLI and UI presentation.
#[derive(Clone, Debug)]
pub struct Client<T> {
    transport: T,
}

/// Successful synchronous data or an accepted asynchronous mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome<D> {
    Result { generation: Generation, data: D },
    Accepted(Box<Operation>),
}

/// Every fallible method reports bounded transport, protocol, API rejection,
/// or asynchronous-operation failures through [`ClientError`].
#[allow(clippy::missing_errors_doc)]
impl<T: Transport> Client<T> {
    #[must_use]
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn execute<D: DeserializeOwned>(
        &self,
        request: &Request,
    ) -> Result<Response<D>, ClientError> {
        let raw = self.transport.execute(&generated_request_id(), request)?;
        decode_response(raw)
    }

    /// Reads the authoritative host snapshot.
    pub fn host(&self) -> Result<Outcome<HostSnapshot>, ClientError> {
        self.call(&Request::get("/1.0"))
    }

    /// Reads host/release compatibility evidence.
    pub fn compatibility(&self) -> Result<Outcome<HostCompatibilityReport>, ClientError> {
        self.call(&Request::get("/1.0/compatibility"))
    }

    /// Reads the assignable Multikernel resource pool.
    pub fn resource_pool(&self) -> Result<Outcome<ResourcePool>, ClientError> {
        self.call(&Request::get("/1.0/resource-pool"))
    }

    /// Reads all managed peer-kernel instances.
    pub fn instances(&self) -> Result<Outcome<Vec<Instance>>, ClientError> {
        self.call(&Request::get("/1.0/instances"))
    }

    /// Reads one managed peer-kernel instance.
    pub fn instance(&self, id: InstanceId) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::get(format!("/1.0/instances/{}", id.0)))
    }

    /// Reads the immutable image catalog.
    pub fn images(&self) -> Result<Outcome<Vec<ImageArtifact>>, ClientError> {
        self.call(&Request::get("/1.0/images"))
    }

    /// Reads deployable generic Linux disk images.
    pub fn os_images(&self) -> Result<Outcome<Vec<OsImage>>, ClientError> {
        self.call(&Request::get("/1.0/os-images"))
    }

    /// Reads one deployable generic Linux disk image.
    pub fn os_image(&self, id: &str) -> Result<Outcome<OsImage>, ClientError> {
        self.call(&Request::get(format!("/1.0/os-images/{id}")))
    }

    /// Reads deployment-safe block-device inventory.
    pub fn storage_devices(&self) -> Result<Outcome<StorageInventory>, ClientError> {
        self.call(&Request::get("/1.0/storage-devices"))
    }

    /// Reads recent asynchronous operations.
    pub fn operations(&self) -> Result<Outcome<Vec<Operation>>, ClientError> {
        self.call(&Request::get("/1.0/operations"))
    }

    /// Reads one asynchronous operation.
    pub fn operation(&self, id: &OperationId) -> Result<Outcome<Operation>, ClientError> {
        self.call(&Request::get(format!("/1.0/operations/{}", id.0)))
    }

    /// Reads invalidation events after a monotonic cursor.
    pub fn events(&self, after: u64) -> Result<Outcome<EventPage>, ClientError> {
        self.call(&Request::get(format!("/1.0/events?after={after}")))
    }

    /// Replaces the Multikernel resource pool.
    pub fn set_resource_pool(
        &self,
        mutation: &ResourcePoolMutation,
    ) -> Result<Outcome<ResourcePool>, ClientError> {
        self.call(&Request::json("PUT", "/1.0/resource-pool", mutation)?)
    }

    /// Imports one generic Linux disk image from an admitted host path.
    pub fn import_os_image(
        &self,
        mutation: &ImportOsImageMutation,
    ) -> Result<Outcome<OsImage>, ClientError> {
        self.call(&Request::json("POST", "/1.0/os-images", mutation)?)
    }

    /// Creates one peer-kernel instance.
    pub fn create_instance(
        &self,
        mutation: &CreateInstanceMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json("POST", "/1.0/instances", mutation)?)
    }

    /// Replaces selected resources of one instance.
    pub fn update_instance(
        &self,
        id: InstanceId,
        mutation: &UpdateInstanceMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json(
            "PATCH",
            format!("/1.0/instances/{}", id.0),
            mutation,
        )?)
    }

    /// Loads host paths into one ready instance.
    pub fn load_instance(
        &self,
        id: InstanceId,
        mutation: &LoadInstanceMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json(
            "POST",
            format!("/1.0/instances/{}/load", id.0),
            mutation,
        )?)
    }

    /// Loads managed immutable artifacts into one ready instance.
    pub fn load_managed_image(
        &self,
        id: InstanceId,
        mutation: &LoadManagedImageMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json(
            "POST",
            format!("/1.0/instances/{}/load-image", id.0),
            mutation,
        )?)
    }

    /// Starts one loaded instance.
    pub fn start_instance(
        &self,
        id: InstanceId,
        mutation: InstanceLifecycleMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.lifecycle(id, "POST", "/start", mutation)
    }

    /// Unloads one inactive instance.
    pub fn unload_instance(
        &self,
        id: InstanceId,
        mutation: InstanceLifecycleMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.lifecycle(id, "POST", "/unload", mutation)
    }

    /// Deletes one ready instance.
    pub fn delete_instance(
        &self,
        id: InstanceId,
        mutation: InstanceLifecycleMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.lifecycle(id, "DELETE", "", mutation)
    }

    /// Stops one active instance.
    pub fn stop_instance(
        &self,
        id: InstanceId,
        mutation: &StopInstanceMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json(
            "POST",
            format!("/1.0/instances/{}/stop", id.0),
            mutation,
        )?)
    }

    /// Imports one immutable host-managed image.
    pub fn import_image(
        &self,
        mutation: &ImportImageMutation,
    ) -> Result<Outcome<ImageArtifact>, ClientError> {
        self.call(&Request::json("POST", "/1.0/images", mutation)?)
    }

    /// Requests cooperative cancellation of one queued or running operation.
    pub fn cancel_operation(&self, id: &OperationId) -> Result<Outcome<Operation>, ClientError> {
        self.call(&Request {
            method: "DELETE",
            path: format!("/1.0/operations/{}", id.0),
            body: Vec::new(),
        })
    }

    pub fn wait_operation(
        &self,
        id: &OperationId,
        policy: WaitPolicy,
    ) -> Result<Operation, ClientError> {
        if policy.max_attempts == 0 {
            return Err(ClientError::new(
                ErrorKind::Timeout,
                "operation wait has no attempts",
            ));
        }
        for attempt in 0..policy.max_attempts {
            let request = Request::get(format!("/1.0/operations/{}", id.0));
            let operation = match self.execute::<Operation>(&request)? {
                Response::Result { data, .. } => data,
                Response::Accepted { operation } => operation,
                Response::Error { error } => {
                    return Err(ClientError {
                        kind: ErrorKind::Rejected,
                        message: error.message.clone(),
                        api_error: Some(Box::new(error)),
                        operation: None,
                    });
                }
            };
            match operation.state {
                OperationState::Succeeded => return Ok(operation),
                OperationState::Failed => {
                    return Err(operation_error(ErrorKind::OperationFailed, operation));
                }
                OperationState::Cancelled => {
                    return Err(operation_error(ErrorKind::OperationCancelled, operation));
                }
                OperationState::Indeterminate | OperationState::Unknown => {
                    return Err(operation_error(
                        ErrorKind::OperationIndeterminate,
                        operation,
                    ));
                }
                OperationState::Queued | OperationState::Running => {}
            }
            if attempt + 1 < policy.max_attempts {
                thread::sleep(policy.interval);
            }
        }
        Err(ClientError::new(
            ErrorKind::Timeout,
            "operation did not finish within the polling limit",
        ))
    }

    fn lifecycle(
        &self,
        id: InstanceId,
        method: &'static str,
        suffix: &str,
        mutation: InstanceLifecycleMutation,
    ) -> Result<Outcome<Instance>, ClientError> {
        self.call(&Request::json(
            method,
            format!("/1.0/instances/{}{suffix}", id.0),
            &mutation,
        )?)
    }

    fn call<D: DeserializeOwned>(&self, request: &Request) -> Result<Outcome<D>, ClientError> {
        match self.execute(request)? {
            Response::Result { generation, data } => Ok(Outcome::Result { generation, data }),
            Response::Accepted { operation } => Ok(Outcome::Accepted(Box::new(operation))),
            Response::Error { error } => Err(ClientError {
                kind: ErrorKind::Rejected,
                message: error.message.clone(),
                api_error: Some(Box::new(error)),
                operation: None,
            }),
        }
    }
}

fn operation_error(kind: ErrorKind, operation: Operation) -> ClientError {
    ClientError {
        kind,
        message: operation.error.as_ref().map_or_else(
            || format!("operation entered terminal state {:?}", operation.state),
            |error| error.message.clone(),
        ),
        api_error: operation.error.clone().map(Box::new),
        operation: Some(Box::new(operation)),
    }
}

fn decode_response<T: DeserializeOwned>(raw: RawResponse) -> Result<Response<T>, ClientError> {
    let response: Response<T> = serde_json::from_value(raw.value)
        .map_err(|_| ClientError::protocol("response envelope is invalid"))?;
    match (&response, raw.status) {
        (Response::Result { .. } | Response::Accepted { .. }, 200..=299)
        | (Response::Error { .. }, 400..=599) => Ok(response),
        _ => Err(ClientError::protocol(
            "HTTP status and response envelope disagree",
        )),
    }
}

fn generated_request_id() -> String {
    format!(
        "kernmux-client-{}-{}",
        std::process::id(),
        REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn read_response(
    stream: &mut impl Read,
    max_header_bytes: usize,
    max_body_bytes: usize,
) -> Result<RawResponse, ClientError> {
    let mut received = Vec::new();
    let header_end = loop {
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if received.len() >= max_header_bytes {
            return Err(ClientError::protocol("response headers exceed their limit"));
        }
        let mut chunk = [0_u8; 4096];
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ClientError::transport(format!("failed to read response: {error}")))?;
        if read == 0 {
            return Err(ClientError::protocol("response ended before its headers"));
        }
        received.extend_from_slice(&chunk[..read]);
    };
    if header_end > max_header_bytes {
        return Err(ClientError::protocol("response headers exceed their limit"));
    }
    let head = parse_response_head(&received[..header_end - 4])?;
    if head.content_length > max_body_bytes {
        return Err(ClientError::protocol("response body exceeds its limit"));
    }
    let mut body = received.split_off(header_end);
    if body.len() > head.content_length {
        return Err(ClientError::protocol(
            "response contains bytes beyond its declared body",
        ));
    }
    let received_body_bytes = body.len();
    body.resize(head.content_length, 0);
    if let Err(error) = stream.read_exact(&mut body[received_body_bytes..]) {
        return Err(if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ClientError::protocol("response ended before its declared body")
        } else {
            ClientError::transport(format!("failed to read response body: {error}"))
        });
    }
    let mut extra = [0_u8; 1];
    match stream.read(&mut extra) {
        Ok(0) => {}
        Ok(_) => {
            return Err(ClientError::protocol(
                "response contains bytes beyond its declared body",
            ));
        }
        Err(error) => {
            return Err(ClientError::transport(format!(
                "failed to finish response: {error}"
            )));
        }
    }
    let value = serde_json::from_slice(&body)
        .map_err(|_| ClientError::protocol("response body is not valid JSON"))?;
    Ok(RawResponse {
        status: head.status,
        value,
    })
}

struct ResponseHead {
    status: u16,
    content_length: usize,
}

fn parse_response_head(bytes: &[u8]) -> Result<ResponseHead, ClientError> {
    let header_text = std::str::from_utf8(bytes)
        .map_err(|_| ClientError::protocol("response headers are not valid UTF-8"))?;
    let mut lines = header_text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("HTTP/1.1"))
                .then(|| fields.next())??
                .parse::<u16>()
                .ok()
        })
        .ok_or_else(|| ClientError::protocol("response status line is invalid"))?;
    if status == 101 {
        return Err(ClientError::protocol("unexpected protocol upgrade"));
    }
    let mut content_length = None;
    let mut content_type_is_json = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| ClientError::protocol("response header is invalid"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ClientError::protocol("chunked responses are not supported"));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ClientError::protocol(
                    "response has duplicate content length",
                ));
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ClientError::protocol("response content length is invalid"))?,
            );
        }
        if name.eq_ignore_ascii_case("content-type") {
            if content_type_is_json.is_some() {
                return Err(ClientError::protocol("response has duplicate content type"));
            }
            content_type_is_json =
                Some(
                    value.trim().split(';').next().is_some_and(|media_type| {
                        media_type.eq_ignore_ascii_case("application/json")
                    }),
                );
        }
    }
    if content_type_is_json != Some(true) {
        return Err(ClientError::protocol(
            "response content type is not application/json",
        ));
    }
    Ok(ResponseHead {
        status,
        content_length: content_length
            .ok_or_else(|| ClientError::protocol("response content length is missing"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, io::Cursor, sync::Mutex};

    struct MockTransport {
        responses: Mutex<VecDeque<RawResponse>>,
    }

    impl Transport for MockTransport {
        fn execute(
            &self,
            _request_id: &str,
            _request: &Request,
        ) -> Result<RawResponse, ClientError> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ClientError::transport("no mock response"))
        }
    }

    fn operation_response(state: &str) -> RawResponse {
        RawResponse {
            status: 200,
            value: serde_json::json!({
                "kind": "result",
                "generation": 4,
                "data": {
                    "id": "op-1",
                    "kind": "start_instance",
                    "state": state,
                    "expected_generation": 3,
                    "created_at": "2026-08-31T00:00:00Z"
                }
            }),
        }
    }

    fn response(body: &[u8], content_type: &str) -> Cursor<Vec<u8>> {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        Cursor::new([head.into_bytes(), body.to_vec()].concat())
    }

    #[test]
    fn accepts_one_complete_json_response() {
        let mut input = response(
            br#"{"kind":"result","generation":3,"data":[]}"#,
            "application/json; charset=utf-8",
        );
        let parsed =
            read_response(&mut input, DEFAULT_MAX_HEADER_BYTES, DEFAULT_MAX_BODY_BYTES).unwrap();
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.value["generation"], 3);
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_framing() {
        let body = br#"{"kind":"result"}"#;
        let mut truncated = response(body, "application/json").into_inner();
        truncated.pop();
        assert_eq!(
            read_response(&mut Cursor::new(truncated), 32 * 1024, 1024)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
        let mut trailing = response(body, "application/json").into_inner();
        trailing.push(b'x');
        assert_eq!(
            read_response(&mut Cursor::new(trailing), 32 * 1024, 1024)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
    }

    #[test]
    fn rejects_unsupported_response_shapes() {
        let mut wrong_type = response(br"{}", "text/plain");
        assert_eq!(
            read_response(&mut wrong_type, 32 * 1024, 1024)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
        let mut chunked = Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".to_vec());
        assert_eq!(
            read_response(&mut chunked, 32 * 1024, 1024)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
    }

    #[test]
    fn operation_wait_reaches_success_within_a_bound() {
        let client = Client::new(MockTransport {
            responses: Mutex::new(VecDeque::from([
                operation_response("running"),
                operation_response("succeeded"),
            ])),
        });
        let operation = client
            .wait_operation(
                &OperationId("op-1".into()),
                WaitPolicy {
                    max_attempts: 2,
                    interval: Duration::ZERO,
                },
            )
            .unwrap();
        assert_eq!(operation.state, OperationState::Succeeded);
    }

    #[test]
    fn operation_wait_distinguishes_terminal_and_timeout_states() {
        let failed = Client::new(MockTransport {
            responses: Mutex::new(VecDeque::from([operation_response("failed")])),
        })
        .wait_operation(
            &OperationId("op-1".into()),
            WaitPolicy {
                max_attempts: 1,
                interval: Duration::ZERO,
            },
        )
        .unwrap_err();
        assert_eq!(failed.kind, ErrorKind::OperationFailed);

        let timeout = Client::new(MockTransport {
            responses: Mutex::new(VecDeque::from([operation_response("running")])),
        })
        .wait_operation(
            &OperationId("op-1".into()),
            WaitPolicy {
                max_attempts: 1,
                interval: Duration::ZERO,
            },
        )
        .unwrap_err();
        assert_eq!(timeout.kind, ErrorKind::Timeout);
    }
}
