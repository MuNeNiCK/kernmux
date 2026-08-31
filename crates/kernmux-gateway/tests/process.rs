use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::net::UnixListener,
    thread,
};

use kernmux_client::UnixTransport;
use kernmux_gateway::{Gateway, GatewayConfig};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_authenticated_reads_and_mutations_to_the_unix_daemon() {
    let root = TempDir::new().unwrap();
    let socket = root.path().join("kernmuxd.sock");
    let daemon = UnixListener::bind(&socket).unwrap();
    let daemon_thread = thread::spawn(move || {
        let first = accept_request(&daemon);
        assert!(first.starts_with("GET /1.0 HTTP/1.1\r\n"), "{first}");
        let second = accept_request(&daemon);
        assert!(
            second.starts_with("POST /1.0/instances HTTP/1.1\r\n"),
            "{second}"
        );
        assert!(second.ends_with("{}"), "{second}");
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = GatewayConfig::loopback("g".repeat(48), root.path().into());
    config.bind = address;
    config.allowed_origins = [format!("http://{address}")].into_iter().collect();
    let gateway = Gateway::new(config, UnixTransport::new(&socket)).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(gateway.serve(listener, async {
        let _ = stop_rx.await;
    }));

    assert_status(&exchange(address, "GET", "/api/1.0", None, ""), 200);
    assert_status(
        &exchange(
            address,
            "POST",
            "/api/1.0/instances",
            Some(&format!("http://{address}")),
            "{}",
        ),
        200,
    );

    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
    daemon_thread.join().unwrap();
    let _ = fs::remove_file(socket);
}

fn accept_request(listener: &UnixListener) -> String {
    let (mut stream, _) = listener.accept().unwrap();
    let request = read_request(&mut stream);
    let body = br#"{"kind":"result","generation":3,"data":{}}"#;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    request
}

fn read_request(stream: &mut impl Read) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).unwrap();
    let length = text
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap();
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).unwrap();
    format!("{text}{}", String::from_utf8(body).unwrap())
}

fn exchange(
    address: SocketAddr,
    method: &str,
    path: &str,
    origin: Option<&str>,
    body: &str,
) -> Vec<u8> {
    let origin = origin.map_or_else(String::new, |origin| format!("Origin: {origin}\r\n"));
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        "g".repeat(48),
        body.len(),
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    response
}

fn assert_status(response: &[u8], expected: u16) {
    let response = std::str::from_utf8(response).unwrap();
    assert!(
        response.starts_with(&format!("HTTP/1.1 {expected} ")),
        "{response}"
    );
}
