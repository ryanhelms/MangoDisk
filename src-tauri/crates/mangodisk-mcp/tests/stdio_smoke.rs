//! End-to-end stdio smoke test: spawns the real binary with an isolated HOME
//! so no real user data is touched, drives the JSON-RPC handshake over pipes,
//! and verifies the read path plus the fail-closed mutation gate.

use std::{process::Stdio, time::Duration};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::timeout,
};

const READ_TIMEOUT: Duration = Duration::from_secs(30);

const EXPECTED_TOOLS: [&str; 15] = [
    "cleanup_scan",
    "analyze_storage",
    "find_large_files",
    "find_duplicate_files",
    "applications_scan",
    "application_leftovers_scan",
    "startup_scan",
    "system_settings_scan",
    "operation_history",
    "cleanup_execute",
    "permanent_delete",
    "application_uninstall_execute",
    "application_leftovers_execute",
    "startup_apply",
    "system_settings_apply",
];

fn spawn_server(home: &TempDir) -> Child {
    spawn_server_with(home, &[])
}

fn spawn_server_with(home: &TempDir, extra_args: &[&str]) -> Child {
    let data = home.path().join("data");
    let cache = home.path().join("cache");
    // TMPDIR keeps the Linux `${temp}` cleanup roots inside the fixture so a
    // scan never traverses the real shared /tmp tree.
    Command::new(env!("CARGO_BIN_EXE_mangodisk-mcp"))
        .args(extra_args)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CACHE_HOME", &cache)
        .env("TMPDIR", home.path().join("tmp"))
        .env("MANGODISK_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the mangodisk-mcp binary must spawn")
}

async fn send(stdin: &mut ChildStdin, message: Value) {
    let mut line = message.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("stdin write must succeed");
    stdin.flush().await.expect("stdin flush must succeed");
}

async fn read_response_for_id(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    id: u64,
) -> Value {
    read_response_collecting(reader, id, &mut Vec::new()).await
}

/// Reads until the response for `id`, collecting `notifications/progress`
/// messages observed on the way.
async fn read_response_collecting(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    id: u64,
    progress: &mut Vec<Value>,
) -> Value {
    let deadline = std::time::Instant::now() + READ_TIMEOUT;
    loop {
        let mut line = String::new();
        let read = timeout(READ_TIMEOUT, reader.read_line(&mut line))
            .await
            .expect("the server must answer within the timeout")
            .expect("stdout must stay readable");
        assert!(
            read > 0,
            "the server closed stdout before answering id {id}"
        );
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("method").and_then(Value::as_str) == Some("notifications/progress") {
            progress.push(message);
            continue;
        }
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            return message;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no response for id {id} before the deadline"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_server_serves_tools_and_fails_mutations_closed() {
    let home = TempDir::new().expect("temp home");
    let mut child = spawn_server(&home);
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    // Drain stderr so a chatty failure can never deadlock the pipe, and keep
    // the log for assertion messages.
    let mut stderr = child.stderr.take().expect("stderr pipe");
    let stderr_task = tokio::spawn(async move {
        let mut collected = String::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match tokio::io::AsyncReadExt::read(&mut stderr, &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => collected.push_str(&String::from_utf8_lossy(&buffer[..count])),
            }
        }
        collected
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "mcp-smoke-test", "version": "0.0.0" }
            }
        }),
    )
    .await;
    let initialize = read_response_for_id(&mut reader, 1).await;
    assert_eq!(
        initialize["result"]["serverInfo"]["name"], "mangodisk-mcp",
        "unexpected initialize response: {initialize}"
    );

    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }),
    )
    .await;
    let tools = read_response_for_id(&mut reader, 2).await;
    let names = tools["result"]["tools"]
        .as_array()
        .expect("tools/list must return an array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for expected in EXPECTED_TOOLS {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len(),
        "unexpected tools: {names:?}"
    );

    // A read tool runs against the isolated HOME and returns empty history.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "operation_history" }
        }),
    )
    .await;
    let history = read_response_for_id(&mut reader, 3).await;
    assert_eq!(
        history["result"]["structuredContent"]["records"],
        json!([]),
        "unexpected operation_history result: {history}"
    );
    assert_eq!(history["result"]["isError"], false);

    // Mutation tools stay listed but fail closed while mutations are disabled.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "cleanup_execute",
                "arguments": {
                    "token": "mdx_nonexistent",
                    "confirm": true,
                    "ruleIds": ["development.npm-cache"],
                    "dryRun": true
                }
            }
        }),
    )
    .await;
    let rejected = read_response_for_id(&mut reader, 4).await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "mutationsDisabled",
        "unexpected cleanup_execute rejection: {rejected}"
    );

    // Closing stdin (EOF) is the graceful shutdown path for stdio servers.
    drop(stdin);
    let status = timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the server must exit on stdin EOF")
        .expect("the child process must be waitable");
    assert!(status.success(), "unexpected exit status: {status}");

    let stderr_log = stderr_task.await.expect("stderr drain task");
    assert!(
        !stderr_log.contains("panicked"),
        "server panicked: {stderr_log}"
    );
}

/// Full guarded-mutation pass on a fixture: the `dev.npm-cache` rule root is
/// created under the isolated HOME, previewed, then executed with a progress
/// token. Asserts that execution streams MCP progress notifications carrying
/// only counters and stable identifiers, and that the fixture is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_mutation_execution_streams_progress() {
    let home = TempDir::new().expect("temp home");
    let cache_root = home.path().join(".npm/_cacache/content-v2/sha512/ab");
    std::fs::create_dir_all(&cache_root).expect("fixture cache directory");
    for index in 0..3 {
        std::fs::write(
            cache_root.join(format!("entry-{index}")),
            format!("fixture payload {index}"),
        )
        .expect("fixture cache entry");
    }
    std::fs::create_dir_all(home.path().join("tmp")).expect("fixture tmp directory");

    let mut child = spawn_server_with(&home, &["--enable-mutations"]);
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "mcp-progress-test", "version": "0.0.0" }
            }
        }),
    )
    .await;
    read_response_for_id(&mut reader, 1).await;
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "cleanup_scan", "arguments": {} }
        }),
    )
    .await;
    let scan = read_response_for_id(&mut reader, 2).await;
    let scan_content = &scan["result"]["structuredContent"];
    let npm_rule = scan_content["rules"]
        .as_array()
        .expect("cleanup scan must return rules")
        .iter()
        .find(|rule| rule["ruleId"] == "dev.npm-cache")
        .expect("the dev.npm-cache fixture rule must be scanned");
    assert!(
        npm_rule["bytes"].as_u64().unwrap_or(0) > 0,
        "the fixture cache must have reclaimable bytes: {npm_rule}"
    );
    assert_eq!(npm_rule["selectable"], true);
    let token = scan_content["executionToken"]
        .as_str()
        .expect("mutations are enabled, so the scan must issue a token")
        .to_string();

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "cleanup_execute",
                "_meta": { "progressToken": "exec-progress-1" },
                "arguments": {
                    "token": token,
                    "confirm": true,
                    "ruleIds": ["dev.npm-cache"],
                    "dryRun": false
                }
            }
        }),
    )
    .await;
    let mut progress = Vec::new();
    let executed = read_response_collecting(&mut reader, 3, &mut progress).await;

    assert_eq!(
        executed["result"]["isError"], false,
        "the guarded cleanup must succeed: {executed}"
    );
    assert!(
        !progress.is_empty(),
        "execution must stream at least one progress notification"
    );
    let home_path = home.path().to_string_lossy().to_string();
    let mut saw_terminal = false;
    for notification in &progress {
        assert_eq!(
            notification["params"]["progressToken"], "exec-progress-1",
            "progress must carry the client's token: {notification}"
        );
        assert!(
            notification["params"]["progress"].is_number(),
            "progress must be numeric: {notification}"
        );
        if notification["params"]["total"].as_f64() == Some(1.0)
            && notification["params"]["progress"].as_f64() == Some(1.0)
        {
            saw_terminal = true;
        }
        let wire = notification.to_string();
        assert!(
            !wire.contains(&home_path) && !wire.contains("_cacache"),
            "progress notifications must not contain fixture paths: {wire}"
        );
    }
    assert!(
        saw_terminal,
        "the completed execution must emit a terminal progress event: {progress:?}"
    );
    assert!(
        !home.path().join(".npm/_cacache").exists(),
        "the fixture cache root must be deleted by the guarded execution"
    );

    drop(stdin);
    let status = timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the server must exit on stdin EOF")
        .expect("the child process must be waitable");
    assert!(status.success(), "unexpected exit status: {status}");
}
