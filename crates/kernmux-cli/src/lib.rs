//! Unprivileged automation client for the versioned Kernmux management API.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use kernmux_api::v1::{
    CreateInstanceMutation, Generation, ImageKind, ImportImageMutation, InstanceId,
    InstanceLifecycleMutation, LoadInstanceMutation, LoadManagedImageMutation,
    ResourcePoolMutation, StopInstanceMutation, UpdateInstanceMutation,
};
use serde::Serialize;
use serde_json::Value;

const DEFAULT_SOCKET: &str = "/run/kernmux/kernmuxd.sock";
const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Stable process exit categories for scripts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    Usage = 2,
    Transport = 3,
    Protocol = 4,
    RequestRejected = 5,
    Authorization = 6,
    Service = 7,
}

impl ExitCode {
    /// Numeric process status.
    #[must_use]
    pub const fn value(self) -> u8 {
        self as u8
    }
}

/// Executes one CLI invocation over injected output streams.
pub fn run(
    arguments: impl IntoIterator<Item = OsString>,
    mut stdout: impl Write,
    mut stderr: impl Write,
) -> ExitCode {
    match execute(arguments) {
        Ok(Output::Help(text)) => {
            let _ = writeln!(stdout, "{text}");
            ExitCode::Success
        }
        Ok(Output::Response {
            value,
            pretty,
            code,
        }) => {
            let result = if pretty {
                serde_json::to_writer_pretty(&mut stdout, &value)
            } else {
                serde_json::to_writer(&mut stdout, &value)
            };
            if result.is_err() || writeln!(stdout).is_err() {
                let _ = writeln!(stderr, "failed to write command output");
                ExitCode::Protocol
            } else {
                code
            }
        }
        Err(error) => {
            let _ = writeln!(stderr, "{}", error.message);
            error.code
        }
    }
}

enum Output {
    Help(String),
    Response {
        value: Value,
        pretty: bool,
        code: ExitCode,
    },
}

#[derive(Debug)]
struct CliError {
    code: ExitCode,
    message: String,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: ExitCode::Usage,
            message: format!("{}\n\n{}", message.into(), usage()),
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        Self {
            code: ExitCode::Transport,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self {
            code: ExitCode::Protocol,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
struct Invocation {
    socket: PathBuf,
    pretty: bool,
    request_id: String,
    request: ApiRequest,
}

#[derive(Debug)]
struct ApiRequest {
    method: &'static str,
    path: String,
    body: Vec<u8>,
}

fn execute(arguments: impl IntoIterator<Item = OsString>) -> Result<Output, CliError> {
    let args = arguments
        .into_iter()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| CliError::usage("arguments must be valid Unicode"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some(program_args) = args.get(1..) else {
        return Ok(Output::Help(usage()));
    };
    if program_args.is_empty() || program_args == ["--help"] || program_args == ["-h"] {
        return Ok(Output::Help(usage()));
    }
    if program_args == ["--version"] || program_args == ["-V"] {
        return Ok(Output::Help(format!(
            "kernmuxctl {} (API v{})",
            env!("CARGO_PKG_VERSION"),
            kernmux_api::API_MAJOR_VERSION
        )));
    }
    let invocation = parse_invocation(program_args)?;
    let response = send_request(
        &invocation.socket,
        &invocation.request_id,
        &invocation.request,
    )?;
    let mut code = response_exit_code(response.status, &response.value)?;
    if invocation.request.path == "/1.0/compatibility"
        && response.value.pointer("/data/compatible") == Some(&Value::Bool(false))
    {
        code = ExitCode::RequestRejected;
    }
    Ok(Output::Response {
        value: response.value,
        pretty: invocation.pretty,
        code,
    })
}

fn parse_invocation(args: &[String]) -> Result<Invocation, CliError> {
    let mut socket = PathBuf::from(DEFAULT_SOCKET);
    let mut pretty = false;
    let mut request_id = None;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--socket" => {
                index += 1;
                socket = args
                    .get(index)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| CliError::usage("--socket requires a path"))?;
            }
            "--pretty" => pretty = true,
            "--request-id" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError::usage("--request-id requires a value"))?;
                validate_request_id(value)?;
                request_id = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown global option {value}")));
            }
            _ => break,
        }
        index += 1;
    }
    let request = parse_command(&args[index..])?;
    Ok(Invocation {
        socket,
        pretty,
        request_id: request_id.unwrap_or_else(generated_request_id),
        request,
    })
}

fn parse_command(args: &[String]) -> Result<ApiRequest, CliError> {
    let Some((resource, tail)) = args.split_first() else {
        return Err(CliError::usage("a command is required"));
    };
    match resource.as_str() {
        "host" => parse_host(tail),
        "pool" => parse_pool(tail),
        "instance" => parse_instance(tail),
        "image" => parse_image(tail),
        "operation" => parse_operation(tail),
        "events" => parse_events(tail),
        _ => Err(CliError::usage(format!("unknown resource {resource}"))),
    }
}

fn parse_image(args: &[String]) -> Result<ApiRequest, CliError> {
    let Some((action, tail)) = args.split_first() else {
        return Err(CliError::usage("image action is required"));
    };
    match action.as_str() {
        "list" if tail.is_empty() => Ok(get("/1.0/images")),
        "show" => {
            if tail.len() != 2 {
                return Err(CliError::usage("image show requires KIND and ID"));
            }
            let (_, kind_path) = image_kind(&tail[0])?;
            let id = artifact_id(&tail[1])?;
            Ok(get(&format!("/1.0/images/{kind_path}/{id}")))
        }
        "import" => {
            let options =
                Options::parse(tail, &["generation", "kind", "source", "expected-id"], &[])?;
            options.no_positionals()?;
            let (kind, _) = image_kind(options.required("kind")?)?;
            let expected_id = options.value("expected-id").map(artifact_id).transpose()?;
            json_request(
                "POST",
                "/1.0/images",
                &ImportImageMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    kind,
                    source_path: options.required("source")?.to_owned(),
                    expected_id: expected_id.map(str::to_owned),
                },
            )
        }
        _ => Err(CliError::usage("unknown image action")),
    }
}

fn image_kind(value: &str) -> Result<(ImageKind, &'static str), CliError> {
    match value {
        "kernel" => Ok((ImageKind::Kernel, "kernel")),
        "initrd" => Ok((ImageKind::Initrd, "initrd")),
        _ => Err(CliError::usage("image kind must be kernel or initrd")),
    }
}

fn artifact_id(value: &str) -> Result<&str, CliError> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(value)
    } else {
        Err(CliError::usage(
            "image ID must be canonical sha256:<64 lowercase hex digits>",
        ))
    }
}

fn parse_host(args: &[String]) -> Result<ApiRequest, CliError> {
    match args {
        [action] if action == "show" => Ok(get("/1.0")),
        [action] if action == "preflight" => Ok(get("/1.0/compatibility")),
        _ => Err(CliError::usage("host action must be show or preflight")),
    }
}

fn parse_pool(args: &[String]) -> Result<ApiRequest, CliError> {
    let Some((action, tail)) = args.split_first() else {
        return Err(CliError::usage("pool action is required"));
    };
    match action.as_str() {
        "show" if tail.is_empty() => Ok(get("/1.0/resource-pool")),
        "set" => {
            let options = Options::parse(tail, &["generation", "cpus", "memory"], &[])?;
            options.no_positionals()?;
            json_request(
                "PUT",
                "/1.0/resource-pool",
                &ResourcePoolMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    cpu_hardware_ids: cpu_list(options.required("cpus")?)?,
                    memory_bytes: byte_size(options.required("memory")?)?,
                },
            )
        }
        "release" => {
            let options = Options::parse(tail, &["generation"], &[])?;
            options.no_positionals()?;
            json_request(
                "PUT",
                "/1.0/resource-pool",
                &ResourcePoolMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    cpu_hardware_ids: Vec::new(),
                    memory_bytes: 0,
                },
            )
        }
        _ => Err(CliError::usage("unknown pool action")),
    }
}

fn parse_instance(args: &[String]) -> Result<ApiRequest, CliError> {
    let Some((action, tail)) = args.split_first() else {
        return Err(CliError::usage("instance action is required"));
    };
    match action.as_str() {
        "list" if tail.is_empty() => Ok(get("/1.0/instances")),
        "show" => {
            let id = only_id(tail)?;
            Ok(get(&format!("/1.0/instances/{id}")))
        }
        "create" => {
            let options =
                Options::parse(tail, &["generation", "id", "name", "cpus", "memory"], &[])?;
            options.no_positionals()?;
            json_request(
                "POST",
                "/1.0/instances",
                &CreateInstanceMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    id: InstanceId(number(options.required("id")?, "instance ID")?),
                    name: options.required("name")?.to_owned(),
                    cpu_hardware_ids: cpu_list(options.required("cpus")?)?,
                    memory_bytes: byte_size(options.required("memory")?)?,
                },
            )
        }
        "update" => {
            let options = Options::parse(
                tail,
                &["generation", "cpus", "memory", "devices"],
                &["dry-run"],
            )?;
            let id = options.one_positional("instance ID")?;
            let cpus = options.value("cpus").map(cpu_list).transpose()?;
            let memory = options.value("memory").map(byte_size).transpose()?;
            let devices = options.value("devices").map(string_list).transpose()?;
            if cpus.is_none() && memory.is_none() && devices.is_none() {
                return Err(CliError::usage(
                    "instance update requires --cpus, --memory, or --devices",
                ));
            }
            json_request(
                "PATCH",
                &format!("/1.0/instances/{}", number(id, "instance ID")?),
                &UpdateInstanceMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    cpu_hardware_ids: cpus,
                    memory_bytes: memory,
                    device_ids: devices,
                    dry_run: options.flag("dry-run"),
                },
            )
        }
        "load" | "load-image" => parse_instance_load(action, tail),
        "start" | "unload" | "delete" => {
            let options = Options::parse(tail, &["generation"], &[])?;
            let id = number(options.one_positional("instance ID")?, "instance ID")?;
            let generation = generation(options.required("generation")?)?;
            let (method, suffix) = match action.as_str() {
                "start" => ("POST", "/start"),
                "unload" => ("POST", "/unload"),
                "delete" => ("DELETE", ""),
                _ => unreachable!(),
            };
            json_request(
                method,
                &format!("/1.0/instances/{id}{suffix}"),
                &InstanceLifecycleMutation {
                    expected_generation: generation,
                },
            )
        }
        "stop" => {
            let options = Options::parse(tail, &["generation"], &["force"])?;
            let id = number(options.one_positional("instance ID")?, "instance ID")?;
            json_request(
                "POST",
                &format!("/1.0/instances/{id}/stop"),
                &StopInstanceMutation {
                    expected_generation: generation(options.required("generation")?)?,
                    force: options.flag("force"),
                },
            )
        }
        _ => Err(CliError::usage("unknown instance action")),
    }
}

fn parse_instance_load(action: &str, args: &[String]) -> Result<ApiRequest, CliError> {
    let options = Options::parse(args, &["generation", "kernel", "initrd", "cmdline"], &[])?;
    let id = number(options.one_positional("instance ID")?, "instance ID")?;
    let expected_generation = generation(options.required("generation")?)?;
    let command_line = options.value("cmdline").map(str::to_owned);
    if action == "load" {
        json_request(
            "POST",
            &format!("/1.0/instances/{id}/load"),
            &LoadInstanceMutation {
                expected_generation,
                kernel_path: options.required("kernel")?.to_owned(),
                initrd_path: options.value("initrd").map(str::to_owned),
                command_line,
            },
        )
    } else {
        json_request(
            "POST",
            &format!("/1.0/instances/{id}/load-image"),
            &LoadManagedImageMutation {
                expected_generation,
                kernel_id: artifact_id(options.required("kernel")?)?.to_owned(),
                initrd_id: options
                    .value("initrd")
                    .map(artifact_id)
                    .transpose()?
                    .map(str::to_owned),
                command_line,
            },
        )
    }
}

fn parse_operation(args: &[String]) -> Result<ApiRequest, CliError> {
    let Some((action, tail)) = args.split_first() else {
        return Err(CliError::usage("operation action is required"));
    };
    match action.as_str() {
        "list" if tail.is_empty() => Ok(get("/1.0/operations")),
        "show" => Ok(get(&format!(
            "/1.0/operations/{}",
            only_token(tail, "operation ID")?
        ))),
        "cancel" => Ok(ApiRequest {
            method: "DELETE",
            path: format!("/1.0/operations/{}", only_token(tail, "operation ID")?),
            body: Vec::new(),
        }),
        _ => Err(CliError::usage("unknown operation action")),
    }
}

fn parse_events(args: &[String]) -> Result<ApiRequest, CliError> {
    let options = Options::parse(args, &["after"], &[])?;
    options.no_positionals()?;
    let after = options
        .value("after")
        .map_or(Ok(0), |value| number_u64(value, "event cursor"))?;
    Ok(get(&format!("/1.0/events?after={after}")))
}

#[derive(Debug)]
struct Options {
    positionals: Vec<String>,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Options {
    fn parse(args: &[String], value_names: &[&str], flag_names: &[&str]) -> Result<Self, CliError> {
        let values_allowed = value_names.iter().copied().collect::<BTreeSet<_>>();
        let flags_allowed = flag_names.iter().copied().collect::<BTreeSet<_>>();
        let mut result = Self {
            positionals: Vec::new(),
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
        };
        let mut index = 0;
        while let Some(argument) = args.get(index) {
            if let Some(name) = argument.strip_prefix("--") {
                if values_allowed.contains(name) {
                    index += 1;
                    let value = args
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| CliError::usage(format!("--{name} requires a value")))?;
                    if result
                        .values
                        .insert(name.to_owned(), value.clone())
                        .is_some()
                    {
                        return Err(CliError::usage(format!(
                            "--{name} was specified more than once"
                        )));
                    }
                } else if flags_allowed.contains(name) {
                    if !result.flags.insert(name.to_owned()) {
                        return Err(CliError::usage(format!(
                            "--{name} was specified more than once"
                        )));
                    }
                } else {
                    return Err(CliError::usage(format!("unknown option --{name}")));
                }
            } else {
                result.positionals.push(argument.clone());
            }
            index += 1;
        }
        Ok(result)
    }

    fn required(&self, name: &str) -> Result<&str, CliError> {
        self.value(name)
            .ok_or_else(|| CliError::usage(format!("--{name} is required")))
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    fn no_positionals(&self) -> Result<(), CliError> {
        if self.positionals.is_empty() {
            Ok(())
        } else {
            Err(CliError::usage("unexpected positional argument"))
        }
    }

    fn one_positional(&self, label: &str) -> Result<&str, CliError> {
        if self.positionals.len() == 1 {
            Ok(&self.positionals[0])
        } else {
            Err(CliError::usage(format!("exactly one {label} is required")))
        }
    }
}

fn send_request(
    socket: &Path,
    request_id: &str,
    request: &ApiRequest,
) -> Result<ApiResponse, CliError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        CliError::transport(format!(
            "failed to connect to {}: {error}",
            socket.display()
        ))
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| CliError::transport(error.to_string()))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| CliError::transport(error.to_string()))?;
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
        .map_err(|error| CliError::transport(format!("failed to send request: {error}")))?;
    read_response(&mut stream)
}

#[derive(Debug)]
struct ApiResponse {
    status: u16,
    value: Value,
}

fn read_response(stream: &mut impl Read) -> Result<ApiResponse, CliError> {
    let mut received = Vec::new();
    let header_end = loop {
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if received.len() >= MAX_RESPONSE_HEADER_BYTES {
            return Err(CliError::protocol("response headers exceed their limit"));
        }
        let mut chunk = [0_u8; 4096];
        let read = stream
            .read(&mut chunk)
            .map_err(|error| CliError::transport(format!("failed to read response: {error}")))?;
        if read == 0 {
            return Err(CliError::protocol("response ended before its headers"));
        }
        received.extend_from_slice(&chunk[..read]);
    };
    if header_end > MAX_RESPONSE_HEADER_BYTES {
        return Err(CliError::protocol("response headers exceed their limit"));
    }
    let head = parse_response_head(&received[..header_end - 4])?;
    let length = head.content_length;
    if length > MAX_RESPONSE_BODY_BYTES {
        return Err(CliError::protocol("response body exceeds its limit"));
    }
    let mut body = received.split_off(header_end);
    if body.len() > length {
        return Err(CliError::protocol(
            "response contains bytes beyond its declared body",
        ));
    }
    let received_body_bytes = body.len();
    body.resize(length, 0);
    if let Err(error) = stream.read_exact(&mut body[received_body_bytes..]) {
        return Err(if error.kind() == std::io::ErrorKind::UnexpectedEof {
            CliError::protocol("response ended before its declared body")
        } else {
            CliError::transport(format!("failed to read response body: {error}"))
        });
    }
    let mut extra = [0_u8; 1];
    match stream.read(&mut extra) {
        Ok(0) => {}
        Ok(_) => {
            return Err(CliError::protocol(
                "response contains bytes beyond its declared body",
            ));
        }
        Err(error) => {
            return Err(CliError::transport(format!(
                "failed to finish response: {error}"
            )));
        }
    }
    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| CliError::protocol("response body is not valid JSON"))?;
    Ok(ApiResponse {
        status: head.status,
        value,
    })
}

struct ResponseHead {
    status: u16,
    content_length: usize,
}

fn parse_response_head(bytes: &[u8]) -> Result<ResponseHead, CliError> {
    let header_text = std::str::from_utf8(bytes)
        .map_err(|_| CliError::protocol("response headers are not valid UTF-8"))?;
    let mut lines = header_text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("HTTP/1.1"))
                .then(|| fields.next())?
                .and_then(|value| value.parse::<u16>().ok())
        })
        .ok_or_else(|| CliError::protocol("response status line is invalid"))?;
    if status == 101 {
        return Err(CliError::protocol("unexpected protocol upgrade"));
    }
    let mut content_length = None;
    let mut content_type_is_json = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| CliError::protocol("response header is invalid"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(CliError::protocol("chunked responses are not supported"));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(CliError::protocol("response has duplicate content length"));
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| CliError::protocol("response content length is invalid"))?,
            );
        }
        if name.eq_ignore_ascii_case("content-type") {
            if content_type_is_json.is_some() {
                return Err(CliError::protocol("response has duplicate content type"));
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
        return Err(CliError::protocol(
            "response content type is not application/json",
        ));
    }
    Ok(ResponseHead {
        status,
        content_length: content_length
            .ok_or_else(|| CliError::protocol("response content length is missing"))?,
    })
}

fn response_exit_code(status: u16, value: &Value) -> Result<ExitCode, CliError> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::protocol("response envelope has no valid kind"))?;
    match kind {
        "result" | "accepted" if (200..300).contains(&status) => Ok(ExitCode::Success),
        "error" if status >= 400 => match value.pointer("/error/code").and_then(Value::as_str) {
            Some("unauthorized" | "forbidden") => Ok(ExitCode::Authorization),
            Some(
                "invalid_request"
                | "not_found"
                | "conflict"
                | "precondition_failed"
                | "unsupported",
            ) => Ok(ExitCode::RequestRejected),
            Some("backend_unavailable" | "timeout" | "internal" | "unknown") => {
                Ok(ExitCode::Service)
            }
            _ => Err(CliError::protocol("response error code is invalid")),
        },
        _ => Err(CliError::protocol(
            "HTTP status and response envelope disagree",
        )),
    }
}

fn json_request(
    method: &'static str,
    path: &str,
    body: &impl Serialize,
) -> Result<ApiRequest, CliError> {
    Ok(ApiRequest {
        method,
        path: path.to_owned(),
        body: serde_json::to_vec(body)
            .map_err(|_| CliError::protocol("request serialization failed"))?,
    })
}

fn get(path: &str) -> ApiRequest {
    ApiRequest {
        method: "GET",
        path: path.to_owned(),
        body: Vec::new(),
    }
}

fn only_id(args: &[String]) -> Result<u32, CliError> {
    number(only_token(args, "instance ID")?, "instance ID")
}

fn only_token<'a>(args: &'a [String], label: &str) -> Result<&'a str, CliError> {
    if args.len() != 1
        || args[0].is_empty()
        || args[0].len() > 128
        || !args[0]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(CliError::usage(format!(
            "exactly one valid {label} is required"
        )))
    } else {
        Ok(&args[0])
    }
}

fn generation(value: &str) -> Result<Generation, CliError> {
    number_u64(value, "generation").map(Generation)
}

fn number(value: &str, label: &str) -> Result<u32, CliError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| CliError::usage(format!("{label} must be a positive integer")))
}

fn number_u64(value: &str, label: &str) -> Result<u64, CliError> {
    value
        .parse::<u64>()
        .map_err(|_| CliError::usage(format!("{label} must be an unsigned integer")))
}

fn cpu_list(value: &str) -> Result<Vec<u32>, CliError> {
    if value.is_empty() {
        return Err(CliError::usage("CPU list must not be empty"));
    }
    let mut cpus = BTreeSet::new();
    for part in value.split(',') {
        let (start, end) = part
            .split_once('-')
            .map_or((part, part), |(start, end)| (start, end));
        let start = number_u64(start, "CPU ID")?;
        let end = number_u64(end, "CPU ID")?;
        if start > u32::MAX.into() || end > u32::MAX.into() || start > end || end - start > 4096 {
            return Err(CliError::usage("CPU range is invalid or too large"));
        }
        for cpu in start..=end {
            let cpu = u32::try_from(cpu).map_err(|_| CliError::usage("CPU ID exceeds u32"))?;
            if !cpus.insert(cpu) {
                return Err(CliError::usage("CPU list contains duplicates"));
            }
        }
    }
    Ok(cpus.into_iter().collect())
}

fn string_list(value: &str) -> Result<Vec<String>, CliError> {
    if value.is_empty() {
        return Err(CliError::usage("device list must not be empty"));
    }
    value
        .split(',')
        .map(|item| {
            let item = item.trim();
            if item.is_empty() {
                Err(CliError::usage("device list contains an empty item"))
            } else {
                Ok(item.to_owned())
            }
        })
        .collect()
}

fn byte_size(value: &str) -> Result<u64, CliError> {
    let (digits, multiplier) = [
        ("KiB", 1024_u64),
        ("MiB", 1024_u64.pow(2)),
        ("GiB", 1024_u64.pow(3)),
        ("TiB", 1024_u64.pow(4)),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| {
        value
            .strip_suffix(suffix)
            .map(|digits| (digits, multiplier))
    })
    .unwrap_or((value, 1));
    let bytes = number_u64(digits, "memory size")?
        .checked_mul(multiplier)
        .ok_or_else(|| CliError::usage("memory size overflows u64"))?;
    if bytes == 0 {
        Err(CliError::usage("memory size must be greater than zero"))
    } else {
        Ok(bytes)
    }
}

fn generated_request_id() -> String {
    format!(
        "kernmuxctl-{}-{}",
        std::process::id(),
        REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn validate_request_id(value: &str) -> Result<(), CliError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(CliError::usage(
            "request ID must be a 1-128 byte log-safe token",
        ))
    }
}

fn usage() -> String {
    format!(
        r"Kernmux automation client

Usage: kernmuxctl [--socket PATH] [--pretty] [--request-id ID] RESOURCE ACTION [OPTIONS]

Resources:
  host show
  host preflight
  pool show
  pool set --generation N --cpus LIST --memory SIZE
  pool release --generation N
  instance list
  instance show ID
  instance create --generation N --id ID --name NAME --cpus LIST --memory SIZE
  instance update ID --generation N [--cpus LIST] [--memory SIZE] [--devices BDF[,BDF]] [--dry-run]
  instance load ID --generation N --kernel PATH [--initrd PATH] [--cmdline TEXT]
  instance load-image ID --generation N --kernel ID [--initrd ID] [--cmdline TEXT]
  instance start ID --generation N
  instance stop ID --generation N [--force]
  instance unload ID --generation N
  instance delete ID --generation N
  image list
  image show KIND ID
  image import --generation N --kind KIND --source PATH [--expected-id ID]
  operation list
  operation show ID
  operation cancel ID
  events [--after SEQUENCE]

All mutations require --generation. Memory accepts bytes, KiB, MiB, GiB, or TiB.

Exit status:
  0 success                 2 invalid command
  3 transport failure       4 protocol failure
  5 request rejected        6 authorization failure
  7 service failure

API error envelopes are written to stdout unchanged. Local errors are written to stderr.
Default socket: {DEFAULT_SOCKET}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_host_preflight_as_a_read_only_versioned_request() {
        let request = parse_command(&strings(&["host", "preflight"])).unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/1.0/compatibility");
        assert!(request.body.is_empty());
    }

    #[test]
    fn parses_resource_commands_into_versioned_requests() {
        let invocation = parse_invocation(&strings(&[
            "--socket",
            "/tmp/api.sock",
            "instance",
            "create",
            "--generation",
            "4",
            "--id",
            "7",
            "--name",
            "lab",
            "--cpus",
            "2-3,8",
            "--memory",
            "1GiB",
        ]))
        .unwrap();
        assert_eq!(invocation.request.method, "POST");
        assert_eq!(invocation.request.path, "/1.0/instances");
        let body: Value = serde_json::from_slice(&invocation.request.body).unwrap();
        assert_eq!(body["expected_generation"], 4);
        assert_eq!(body["cpu_hardware_ids"], serde_json::json!([2, 3, 8]));
        assert_eq!(body["memory_bytes"], 1_073_741_824_u64);
    }

    #[test]
    fn mutations_require_generation_and_reject_ambiguous_resources() {
        assert!(parse_invocation(&strings(&["pool", "release"])).is_err());
        assert!(
            parse_invocation(&strings(&[
                "pool",
                "set",
                "--generation",
                "1",
                "--cpus",
                "2,2",
                "--memory",
                "1GiB"
            ]))
            .is_err()
        );
        assert!(
            parse_invocation(&strings(&["instance", "update", "1", "--generation", "2"])).is_err()
        );
        assert!(
            parse_invocation(&strings(&[
                "instance",
                "update",
                "1",
                "--generation",
                "2",
                "--devices",
                "0000:06:00.0,"
            ]))
            .is_err()
        );
    }

    #[test]
    fn parses_device_replacement_without_normalizing_identifiers() {
        let invocation = parse_invocation(&strings(&[
            "instance",
            "update",
            "1",
            "--generation",
            "2",
            "--devices",
            "0000:06:00.0,0000:07:00.0",
            "--dry-run",
        ]))
        .unwrap();
        let body: Value = serde_json::from_slice(&invocation.request.body).unwrap();

        assert_eq!(
            body["device_ids"],
            serde_json::json!(["0000:06:00.0", "0000:07:00.0"])
        );
        assert_eq!(body["dry_run"], true);
    }

    #[test]
    fn parses_image_catalog_commands_with_canonical_ids() {
        let id = format!("sha256:{}", "a".repeat(64));
        let show = parse_invocation(&strings(&["image", "show", "kernel", &id])).unwrap();
        assert_eq!(show.request.path, format!("/1.0/images/kernel/{id}"));

        let import = parse_invocation(&strings(&[
            "image",
            "import",
            "--generation",
            "2",
            "--kind",
            "initrd",
            "--source",
            "/boot/initrd",
            "--expected-id",
            &id,
        ]))
        .unwrap();
        let body: Value = serde_json::from_slice(&import.request.body).unwrap();
        assert_eq!(body["expected_generation"], 2);
        assert_eq!(body["kind"], "initrd");
        assert_eq!(body["expected_id"], id);

        assert!(parse_invocation(&strings(&["image", "show", "kernel", "sha256:ABC"])).is_err());
        assert!(
            parse_invocation(&strings(&[
                "image",
                "import",
                "--generation",
                "1",
                "--kind",
                "disk",
                "--source",
                "/tmp/disk"
            ]))
            .is_err()
        );
    }

    #[test]
    fn parses_managed_image_load_without_store_paths() {
        let kernel = format!("sha256:{}", "1".repeat(64));
        let initrd = format!("sha256:{}", "2".repeat(64));
        let invocation = parse_invocation(&strings(&[
            "instance",
            "load-image",
            "4",
            "--generation",
            "9",
            "--kernel",
            &kernel,
            "--initrd",
            &initrd,
        ]))
        .unwrap();
        assert_eq!(invocation.request.path, "/1.0/instances/4/load-image");
        let body: Value = serde_json::from_slice(&invocation.request.body).unwrap();
        assert_eq!(body["kernel_id"], kernel);
        assert_eq!(body["initrd_id"], initrd);
        assert!(body.get("kernel_path").is_none());
    }

    #[test]
    fn maps_api_envelopes_to_stable_exit_categories() {
        assert_eq!(
            response_exit_code(
                200,
                &serde_json::json!({"kind":"result","generation":1,"data":{}})
            )
            .unwrap(),
            ExitCode::Success
        );
        assert_eq!(
            response_exit_code(
                403,
                &serde_json::json!({"kind":"error","error":{"code":"forbidden"}})
            )
            .unwrap(),
            ExitCode::Authorization
        );
        assert_eq!(
            response_exit_code(
                409,
                &serde_json::json!({"kind":"error","error":{"code":"conflict"}})
            )
            .unwrap(),
            ExitCode::RequestRejected
        );
        assert_eq!(
            response_exit_code(
                503,
                &serde_json::json!({"kind":"error","error":{"code":"backend_unavailable"}})
            )
            .unwrap(),
            ExitCode::Service
        );
    }

    #[test]
    fn accepts_one_complete_json_response() {
        let body = br#"{"kind":"result","generation":3,"data":[]}"#;
        let mut response = response(body, "application/json; charset=utf-8");
        let parsed = read_response(&mut response).unwrap();
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.value["generation"], 3);
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_response_framing() {
        let body = br#"{"kind":"result"}"#;

        let mut truncated = response(body, "application/json").into_inner();
        truncated.pop();
        assert_eq!(
            read_response(&mut Cursor::new(truncated)).unwrap_err().code,
            ExitCode::Protocol
        );

        let mut trailing = response(body, "application/json").into_inner();
        trailing.push(b'x');
        assert_eq!(
            read_response(&mut Cursor::new(trailing)).unwrap_err().code,
            ExitCode::Protocol
        );

        let duplicate_type = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        let mut duplicate_type = Cursor::new([duplicate_type, body.to_vec()].concat());
        assert_eq!(
            read_response(&mut duplicate_type).unwrap_err().code,
            ExitCode::Protocol
        );
    }

    #[test]
    fn rejects_unsupported_http_response_shapes() {
        let mut wrong_type = response(br"{}", "text/plain");
        assert_eq!(
            read_response(&mut wrong_type).unwrap_err().code,
            ExitCode::Protocol
        );

        let mut chunked = Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"
                .to_vec(),
        );
        assert_eq!(
            read_response(&mut chunked).unwrap_err().code,
            ExitCode::Protocol
        );
    }

    fn response(body: &[u8], content_type: &str) -> Cursor<Vec<u8>> {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        Cursor::new([head.into_bytes(), body.to_vec()].concat())
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }
}
