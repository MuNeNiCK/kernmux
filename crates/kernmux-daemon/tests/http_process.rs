#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::process::{Pid, Signal, getuid, kill_process};
use serde_json::Value;
use tempfile::TempDir;

const STARTUP_DEADLINE: Duration = Duration::from_secs(5);

struct DaemonProcess {
    child: Child,
    socket_path: PathBuf,
    import_path: PathBuf,
    _directory: TempDir,
}

impl DaemonProcess {
    fn spawn() -> Self {
        let directory = TempDir::new().expect("daemon test directory must be created");
        let socket_path = directory.path().join("kernmuxd.sock");
        let import_path = directory.path().join("vmlinux");
        fs::write(&import_path, b"test kernel image").expect("test image must be written");
        let child = Command::new(env!("CARGO_BIN_EXE_kernmuxd"))
            .env("KERNMUX_SOCKET_PATH", &socket_path)
            .env("KERNMUX_IMAGE_ROOTS", directory.path())
            .env("KERNMUX_IMAGE_STORE_ROOT", directory.path().join("images"))
            .env("KERNMUX_ADMINISTRATOR_UIDS", getuid().as_raw().to_string())
            .env("RUST_LOG", "error")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon process must start");
        let mut process = Self {
            child,
            socket_path,
            import_path,
            _directory: directory,
        };
        process.wait_until_ready();
        process
    }

    fn wait_until_ready(&mut self) {
        let started = Instant::now();
        while started.elapsed() < STARTUP_DEADLINE {
            if self.socket_path.exists() {
                return;
            }
            if let Some(status) = self
                .child
                .try_wait()
                .expect("daemon status must be readable")
            {
                panic!("daemon exited before creating its socket: {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("daemon did not create its socket before the startup deadline");
    }

    fn request(&self, request: &[u8]) -> HttpResult {
        let mut stream = UnixStream::connect(&self.socket_path)
            .expect("daemon Unix socket must accept connections");
        stream
            .write_all(request)
            .expect("HTTP request must be written");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("HTTP response must be read");
        HttpResult::parse(&response)
    }

    fn terminate(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            kill_process(Pid::from_child(&self.child), Signal::TERM)
                .expect("SIGTERM must reach daemon");
            let started = Instant::now();
            while started.elapsed() < STARTUP_DEADLINE {
                if self.child.try_wait().unwrap().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            self.child.kill().expect("stuck daemon must be killed");
            self.child.wait().expect("killed daemon must be reaped");
            panic!("daemon did not stop after SIGTERM");
        }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Debug)]
struct HttpResult {
    status: u16,
    body: Value,
}

impl HttpResult {
    fn parse(response: &[u8]) -> Self {
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("HTTP response must contain a header terminator");
        let headers = std::str::from_utf8(&response[..split]).expect("headers must be UTF-8");
        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .expect("HTTP status must be present");
        let body = serde_json::from_slice(&response[split + 4..]).expect("body must be JSON");
        Self { status, body }
    }
}

fn get(path: &str, request_id: &str) -> Vec<u8> {
    format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Request-Id: {request_id}\r\n\r\n"
    )
    .into_bytes()
}

fn post(path: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn serves_typed_api_across_the_process_and_socket_boundary() {
    let mut daemon = DaemonProcess::spawn();

    let snapshot = daemon.request(&get("/1.0", "process-e2e-snapshot"));
    assert_eq!(snapshot.status, 200);
    assert_eq!(snapshot.body["kind"], "result");
    assert!(snapshot.body["data"]["generation"].is_number());
    assert!(snapshot.body["data"]["kernel"]["multikernel_enabled"].is_boolean());

    let events = daemon.request(&get("/1.0/events?after=0", "process-e2e-events"));
    assert_eq!(events.status, 200);
    assert_eq!(events.body["kind"], "result");

    let images = daemon.request(&get("/1.0/images", "process-e2e-images"));
    assert_eq!(images.status, 200);
    assert_eq!(images.body["generation"], 1);
    assert_eq!(images.body["data"], serde_json::json!([]));

    let import_body = serde_json::json!({
        "expected_generation": 1,
        "kind": "kernel",
        "source_path": daemon.import_path,
    })
    .to_string();
    let import = daemon.request(&post("/1.0/images", &import_body));
    assert_eq!(import.status, 202);
    assert_eq!(import.body["kind"], "accepted");
    let operation_path = format!(
        "/1.0/operations/{}",
        import.body["operation"]["id"].as_str().unwrap()
    );
    let started = Instant::now();
    loop {
        let operation = daemon.request(&get(&operation_path, "process-e2e-image-operation"));
        match operation.body["data"]["state"].as_str() {
            Some("succeeded") => break,
            Some("failed" | "cancelled") => panic!("image import did not succeed: {operation:?}"),
            _ if started.elapsed() < STARTUP_DEADLINE => thread::sleep(Duration::from_millis(10)),
            _ => panic!("image import did not complete"),
        }
    }
    let images = daemon.request(&get("/1.0/images", "process-e2e-images-after-import"));
    assert_eq!(images.status, 200);
    assert_eq!(images.body["generation"], 2);
    assert_eq!(images.body["data"][0]["kind"], "kernel");
    assert_eq!(images.body["data"][0]["bytes"], 17);

    let missing_load_body = serde_json::json!({
        "expected_generation": snapshot.body["generation"],
        "kernel_id": format!("sha256:{}", "0".repeat(64)),
    })
    .to_string();
    let missing_load = daemon.request(&post("/1.0/instances/1/load-image", &missing_load_body));
    assert_eq!(missing_load.status, 202);
    let operation_path = format!(
        "/1.0/operations/{}",
        missing_load.body["operation"]["id"].as_str().unwrap()
    );
    let started = Instant::now();
    loop {
        let operation = daemon.request(&get(&operation_path, "process-e2e-missing-image"));
        match operation.body["data"]["state"].as_str() {
            Some("failed") => {
                assert_eq!(operation.body["data"]["error"]["code"], "not_found");
                break;
            }
            Some("succeeded" | "cancelled") => {
                panic!("missing managed image was not rejected: {operation:?}")
            }
            _ if started.elapsed() < STARTUP_DEADLINE => thread::sleep(Duration::from_millis(10)),
            _ => panic!("managed image load did not complete"),
        }
    }

    let malformed_mutation = daemon.request(&post("/1.0/instances", "{}"));
    assert_eq!(malformed_mutation.status, 400);
    assert_eq!(malformed_mutation.body["kind"], "error");
    assert_eq!(malformed_mutation.body["error"]["code"], "invalid_request");

    let inactive_console = daemon.request(
        b"POST /1.0/instances/1/console HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade, close\r\nUpgrade: kernmux-console-v1\r\n\r\n",
    );
    assert_eq!(inactive_console.status, 409);
    assert_eq!(inactive_console.body["error"]["code"], "conflict");

    let invalid_correlation = daemon.request(&get("/1.0", "invalid correlation"));
    assert_eq!(invalid_correlation.status, 400);
    assert_eq!(invalid_correlation.body["error"]["code"], "invalid_request");

    daemon.terminate();
    assert!(!Path::new(&daemon.socket_path).exists());
}
