//! HTTP transport tests: the streamable-HTTP server binds loopback only and
//! requires a bearer token on every request.

use std::{process::Stdio, time::Duration};

use serde_json::json;
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
    time::timeout,
};

const TOKEN: &str = "mcp-test-secret";

fn spawn_http_server(home: &TempDir) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mangodisk-mcp"))
        .args(["--http", "--port", "0"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("MANGODISK_LOG", "error")
        .env("MANGODISK_MCP_TOKEN", TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the mangodisk-mcp binary must spawn")
}

/// Reads stderr until the server reports its bound port.
async fn read_listening_port(child: &mut Child) -> u16 {
    let stderr = child.stderr.take().expect("stderr pipe");
    let mut reader = BufReader::new(stderr);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let mut line = String::new();
        let read = timeout(Duration::from_secs(30), reader.read_line(&mut line))
            .await
            .expect("the server must report its port within the timeout")
            .expect("stderr must stay readable");
        assert!(read > 0, "the server exited before listening");
        if let Some(marker) = line.strip_prefix("mangodisk-mcp listening on http://127.0.0.1:") {
            let port = marker
                .trim_end_matches(['/', '\n', '\r'])
                .parse::<u16>()
                .expect("the listening line must carry a port");
            // Keep draining stderr in the background so later logs cannot
            // deadlock the pipe.
            tokio::spawn(async move {
                let mut buffer = String::new();
                while let Ok(count) = reader.read_line(&mut buffer).await {
                    if count == 0 {
                        break;
                    }
                    buffer.clear();
                }
            });
            return port;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no listening line before the deadline"
        );
    }
}

/// Minimal HTTP/1.1 POST over a raw stream. `Connection: close` makes the
/// server terminate the SSE response after the payload so the body read ends.
async fn post_initialize(port: u16, bearer: Option<&str>) -> (u16, String) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "mcp-auth-test", "version": "0.0.0" }
        }
    })
    .to_string();
    let authorization = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the test must connect to the server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request must be sent");
    let mut response = Vec::new();
    timeout(Duration::from_secs(30), stream.read_to_end(&mut response))
        .await
        .expect("the server must answer within the timeout")
        .expect("the response must be readable");
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .expect("the response must carry an HTTP status");
    (status, response)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_mode_requires_the_bearer_token() {
    let home = TempDir::new().expect("temp home");
    let mut child = spawn_http_server(&home);
    let port = read_listening_port(&mut child).await;

    let (status, _) = post_initialize(port, None).await;
    assert_eq!(status, 401, "requests without a token must be rejected");

    let (status, _) = post_initialize(port, Some("wrong-token")).await;
    assert_eq!(status, 401, "requests with a wrong token must be rejected");

    let (status, response) = post_initialize(port, Some(TOKEN)).await;
    assert_eq!(
        status, 200,
        "the correct token must be accepted: {response}"
    );
    assert!(
        response.contains("mangodisk-mcp"),
        "the initialize response must name the server: {response}"
    );

    // The auth test is done; terminate the server without leaking a process.
    child.kill().await.expect("the server must be stoppable");
    child
        .wait()
        .await
        .expect("the child process must be reaped");
}
