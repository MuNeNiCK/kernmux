use std::{path::Path, time::Duration};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode, body::Incoming, header::SEC_WEBSOCKET_PROTOCOL,
};
use hyper_util::rt::TokioIo;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::OwnedSemaphorePermit,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::server::create_response_with_body,
        protocol::{Role, WebSocketConfig},
    },
};

const CONSOLE_PROTOCOL: &str = "kernmux-console-v1";
const BEARER_PROTOCOL_PREFIX: &str = "kernmux-bearer.";
const MAX_CONSOLE_WIRE_BYTES: usize = 64 * 1024 + 5;
const MAX_DAEMON_HEADER_BYTES: usize = 32 * 1024;
const DAEMON_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rejection {
    pub status: StatusCode,
    pub code: &'static str,
}

impl Rejection {
    const fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
}

pub(crate) async fn upgrade(
    mut request: Request<Incoming>,
    bearer_token: &str,
    origin_allowed: bool,
    daemon_socket: &Path,
    request_id: &str,
    permit: OwnedSemaphorePermit,
) -> Result<Response<Full<Bytes>>, Rejection> {
    let instance_id = console_instance(request.method(), request.uri().path())
        .ok_or_else(|| Rejection::new(StatusCode::NOT_FOUND, "route_not_found"))?;
    if !origin_allowed {
        return Err(Rejection::new(StatusCode::FORBIDDEN, "origin_forbidden"));
    }
    let offered = request
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Rejection::new(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    if !valid_subprotocols(offered, bearer_token) {
        return Err(Rejection::new(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let mut daemon = tokio::time::timeout(
        DAEMON_HANDSHAKE_TIMEOUT,
        connect_daemon(daemon_socket, instance_id, request_id),
    )
    .await
    .map_err(|_| Rejection::new(StatusCode::GATEWAY_TIMEOUT, "daemon_timeout"))?
    .map_err(|status| {
        if status == StatusCode::CONFLICT {
            Rejection::new(StatusCode::CONFLICT, "console_conflict")
        } else {
            Rejection::new(StatusCode::BAD_GATEWAY, "daemon_unavailable")
        }
    })?;

    let on_upgrade = hyper::upgrade::on(&mut request);
    let mut response = create_response_with_body(&request, || Full::new(Bytes::new()))
        .map_err(|_| Rejection::new(StatusCode::BAD_REQUEST, "invalid_websocket_upgrade"))?;
    response.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        hyper::header::HeaderValue::from_static(CONSOLE_PROTOCOL),
    );
    tokio::spawn(async move {
        let _permit = permit;
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_CONSOLE_WIRE_BYTES))
            .max_frame_size(Some(MAX_CONSOLE_WIRE_BYTES));
        let websocket =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(config))
                .await;
        bridge(websocket, &mut daemon).await;
    });
    Ok(response)
}

async fn connect_daemon(
    socket: &Path,
    instance_id: u32,
    request_id: &str,
) -> Result<UnixStream, StatusCode> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let request = format!(
        "POST /1.0/instances/{instance_id}/console HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: {CONSOLE_PROTOCOL}\r\nX-Request-Id: {request_id}\r\nContent-Length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut header = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while header.len() < MAX_DAEMON_HEADER_BYTES {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if read == 0 {
            return Err(StatusCode::BAD_GATEWAY);
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !header.ends_with(b"\r\n\r\n") {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let text = std::str::from_utf8(&header).map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .ok_or(StatusCode::BAD_GATEWAY)?;
    if status != StatusCode::SWITCHING_PROTOCOLS {
        return Err(status);
    }
    let mut connection_upgrade = false;
    let mut protocol_upgrade = false;
    for line in lines.filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(StatusCode::BAD_GATEWAY);
        };
        if name.eq_ignore_ascii_case("connection") {
            connection_upgrade = value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
        } else if name.eq_ignore_ascii_case("upgrade") {
            protocol_upgrade = value.trim().eq_ignore_ascii_case(CONSOLE_PROTOCOL);
        }
    }
    if !connection_upgrade || !protocol_upgrade {
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(stream)
}

async fn bridge<S, D>(mut websocket: WebSocketStream<S>, daemon: &mut D)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    D: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut daemon_buffer = vec![0_u8; MAX_CONSOLE_WIRE_BYTES];
    loop {
        tokio::select! {
            message = websocket.next() => match message {
                Some(Ok(Message::Binary(payload))) if payload.len() <= MAX_CONSOLE_WIRE_BYTES => {
                    if daemon.write_all(&payload).await.is_err() { break; }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if websocket.send(Message::Pong(payload)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    let _ = websocket.send(Message::Close(frame)).await;
                    break;
                }
                Some(Ok(Message::Text(_) | Message::Frame(_) | Message::Binary(_)) | Err(_)) | None => {
                    let _ = websocket.send(Message::Close(None)).await;
                    break;
                }
            },
            read = daemon.read(&mut daemon_buffer) => match read {
                Ok(0) | Err(_) => {
                    let _ = websocket.send(Message::Close(None)).await;
                    break;
                }
                Ok(read) => {
                    if websocket.send(Message::binary(Bytes::copy_from_slice(&daemon_buffer[..read]))).await.is_err() { break; }
                }
            }
        }
    }
    let _ = daemon.shutdown().await;
}

fn console_instance(method: &Method, path: &str) -> Option<u32> {
    if *method != Method::GET {
        return None;
    }
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let ["api", "1.0", "instances", id, "console"] = segments.as_slice() else {
        return None;
    };
    let id = id.parse::<u32>().ok()?;
    (1..=511).contains(&id).then_some(id)
}

fn valid_subprotocols(offered: &str, bearer_token: &str) -> bool {
    if offered.len() > 1024 {
        return false;
    }
    let expected = format!(
        "{BEARER_PROTOCOL_PREFIX}{}",
        base64url(bearer_token.as_bytes())
    );
    let mut console = 0_u8;
    let mut bearer = 0_u8;
    for protocol in offered.split(',').map(str::trim) {
        if protocol == CONSOLE_PROTOCOL {
            console = console.saturating_add(1);
        } else if protocol.starts_with(BEARER_PROTOCOL_PREFIX)
            && constant_time_equal(protocol.as_bytes(), expected.as_bytes())
        {
            bearer = bearer.saturating_add(1);
        }
    }
    console == 1 && bearer == 1
}

fn base64url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or_default()) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or_default());
        output.push(char::from(ALPHABET[((value >> 18) & 0x3f) as usize]));
        output.push(char::from(ALPHABET[((value >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            output.push(char::from(ALPHABET[((value >> 6) & 0x3f) as usize]));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[(value & 0x3f) as usize]));
        }
    }
    output
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..1024 {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::{io::duplex, net::UnixListener};

    use super::*;

    #[test]
    fn browser_console_route_is_exact_and_bounded() {
        assert_eq!(
            console_instance(&Method::GET, "/api/1.0/instances/4/console"),
            Some(4)
        );
        assert_eq!(
            console_instance(&Method::POST, "/api/1.0/instances/4/console"),
            None
        );
        assert_eq!(
            console_instance(&Method::GET, "/api/1.0/instances/0/console"),
            None
        );
        assert_eq!(
            console_instance(&Method::GET, "/api/1.0/instances/512/console"),
            None
        );
        assert_eq!(
            console_instance(&Method::GET, "/api/1.0/instances/4/console/extra"),
            None
        );
    }

    #[test]
    fn bearer_subprotocol_matches_browser_base64url() {
        assert_eq!(base64url(b"abc+/="), "YWJjKy89");
        assert!(valid_subprotocols(
            "kernmux-console-v1, kernmux-bearer.YWJjKy89",
            "abc+/="
        ));
        assert!(!valid_subprotocols(
            "kernmux-console-v1, kernmux-bearer.YWJjKy88",
            "abc+/="
        ));
        assert!(!valid_subprotocols("kernmux-console-v1", "abc+/="));
    }

    #[tokio::test]
    async fn daemon_handshake_is_exact_and_requires_console_protocol() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("POST /1.0/instances/4/console HTTP/1.1\r\n"));
            assert!(request.contains("Upgrade: kernmux-console-v1\r\n"));
            assert!(request.contains("X-Request-Id: gateway-test\r\n"));
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: kernmux-console-v1\r\n\r\n")
                .await
                .unwrap();
        });
        let _stream = connect_daemon(&socket, 4, "gateway-test").await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bridge_accepts_only_binary_and_preserves_bytes() {
        let (browser_io, gateway_io) = duplex(4096);
        let (mut daemon_peer, mut daemon_gateway) = duplex(4096);
        let browser = WebSocketStream::from_raw_socket(browser_io, Role::Client, None).await;
        let gateway = WebSocketStream::from_raw_socket(gateway_io, Role::Server, None).await;
        let relay = tokio::spawn(async move { bridge(gateway, &mut daemon_gateway).await });
        let (mut browser_write, mut browser_read) = browser.split();

        browser_write
            .send(Message::binary(vec![0x11, 0, 0, 0, 0]))
            .await
            .unwrap();
        let mut received = [0_u8; 5];
        daemon_peer.read_exact(&mut received).await.unwrap();
        assert_eq!(received, [0x11, 0, 0, 0, 0]);

        daemon_peer
            .write_all(&[0x20, 0, 0, 0, 2, b'o', b'k'])
            .await
            .unwrap();
        assert_eq!(
            browser_read.next().await.unwrap().unwrap(),
            Message::binary(vec![0x20, 0, 0, 0, 2, b'o', b'k'])
        );
        browser_write.send(Message::Close(None)).await.unwrap();
        relay.await.unwrap();
    }
}
