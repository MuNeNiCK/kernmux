use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

static SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[test]
fn process_sends_a_versioned_request_and_prints_the_json_envelope() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        assert!(request.starts_with("GET /1.0/instances HTTP/1.1\r\n"));
        assert!(request.contains("\r\nX-Request-Id: process-test\r\n"));
        write_response(
            &mut stream,
            200,
            br#"{"kind":"result","generation":9,"data":[]}"#,
        );
    });

    let output = Command::new(env!("CARGO_BIN_EXE_kernmuxctl"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "--request-id",
            "process-test",
            "instance",
            "list",
        ])
        .output()
        .unwrap();

    server.join().unwrap();
    let _ = fs::remove_file(&socket);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "{\"data\":[],\"generation\":9,\"kind\":\"result\"}\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn process_preserves_error_envelopes_and_uses_the_stable_exit_category() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        write_response(
            &mut stream,
            403,
            br#"{"kind":"error","error":{"code":"forbidden","message":"denied","retryable":false}}"#,
        );
    });

    let output = Command::new(env!("CARGO_BIN_EXE_kernmuxctl"))
        .args(["--socket", socket.to_str().unwrap(), "host", "show"])
        .output()
        .unwrap();

    server.join().unwrap();
    let _ = fs::remove_file(&socket);
    assert_eq!(output.status.code(), Some(6));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "forbidden");
    assert!(output.stderr.is_empty());
}

fn read_request(stream: &mut impl Read) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    let head = String::from_utf8(bytes).unwrap();
    let content_length = head
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length: ")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap();
    let mut body = vec![0_u8; content_length];
    stream.read_exact(&mut body).unwrap();
    format!("{head}{}", String::from_utf8(body).unwrap())
}

fn write_response(stream: &mut impl Write, status: u16, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "kernmuxctl-test-{}-{}.sock",
        std::process::id(),
        SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}
