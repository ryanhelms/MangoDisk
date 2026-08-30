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

const EXPECTED_TOOLS: [&str; 17] = [
    "cleanup_scan",
    "analyze_storage",
    "find_large_files",
    "find_duplicate_files",
    "applications_scan",
    "application_leftovers_scan",
    "startup_scan",
    "system_settings_scan",
    "operation_history",
    "processes_scan",
    "cleanup_execute",
    "permanent_delete",
    "application_uninstall_execute",
    "application_leftovers_execute",
    "startup_apply",
    "system_settings_apply",
    "process_end",
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

    // The process scan is a read tool: it returns the real process table (the
    // fixture-isolated HOME does not isolate /proc, and reading it is safe)
    // but issues no execution token while mutations are disabled.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "processes_scan", "arguments": {} }
        }),
    )
    .await;
    let scan = read_response_for_id(&mut reader, 5).await;
    let scan_content = &scan["result"]["structuredContent"];
    assert_eq!(
        scan["result"]["isError"], false,
        "processes_scan must succeed: {scan}"
    );
    assert!(scan_content["schemaVersion"].is_number());
    assert!(scan_content["exitedProcessCount"].is_number());
    let processes = scan_content["processes"]
        .as_array()
        .expect("processes_scan must return a process list");
    assert!(
        !processes.is_empty(),
        "the real process table must not be empty"
    );
    for process in processes {
        assert!(
            process["classification"].is_string(),
            "every listed process must carry a classification: {process}"
        );
        if let Some(executable) = process["executablePath"].as_str() {
            assert!(
                !executable.starts_with('/'),
                "executable paths must be redacted: {executable}"
            );
        }
    }
    assert!(
        scan_content.get("executionToken").is_none(),
        "a disabled server must not issue tokens: {scan_content}"
    );

    // process_end fails closed the same way as every other mutation tool.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": "mdx_nonexistent",
                    "confirm": true,
                    "dryRun": false,
                    "pids": [424242]
                }
            }
        }),
    )
    .await;
    let rejected = read_response_for_id(&mut reader, 6).await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "mutationsDisabled",
        "unexpected process_end rejection: {rejected}"
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

/// Full guarded process-end pass on a real fixture process. Unix-only because
/// the flow relies on signal semantics (`sleep` accepting SIGTERM) and a
/// `kill -0` liveness check. Only the fixture pid is ever targeted.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_process_end_guarded_flow() {
    let home = TempDir::new().expect("temp home");
    let mut fixture = std::process::Command::new("sleep")
        .arg("300")
        .spawn()
        .expect("sleep should start");
    let fixture_pid = fixture.id();
    // Reap concurrently so the ended fixture cannot linger as a zombie and
    // still look alive to Core's verification snapshot.
    let reaper = std::thread::spawn(move || fixture.wait());

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
                "clientInfo": { "name": "mcp-process-test", "version": "0.0.0" }
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

    // An unfiltered scan lists the fixture and issues the execution token.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "processes_scan", "arguments": {} }
        }),
    )
    .await;
    let scan = read_response_for_id(&mut reader, 2).await;
    let scan_content = &scan["result"]["structuredContent"];
    assert_eq!(
        scan["result"]["isError"], false,
        "processes_scan must succeed: {scan}"
    );
    let fixture_entry = scan_content["processes"]
        .as_array()
        .expect("processes_scan must return a process list")
        .iter()
        .find(|process| process["pid"].as_u64() == Some(u64::from(fixture_pid)))
        .expect("the fixture process must be listed");
    assert!(
        fixture_entry["classification"].is_string(),
        "the fixture must carry a classification: {fixture_entry}"
    );
    let token = scan_content["executionToken"]
        .as_str()
        .expect("mutations are enabled, so the scan must issue a token")
        .to_string();

    // A bogus token is rejected before anything is prepared.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": "mdx_nonexistent",
                    "confirm": true,
                    "dryRun": false,
                    "pids": [fixture_pid]
                }
            }
        }),
    )
    .await;
    let rejected = read_response_for_id(&mut reader, 3).await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "tokenUnknown",
        "a bogus token must be rejected: {rejected}"
    );

    // confirm: false is rejected without burning the token.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": token,
                    "confirm": false,
                    "dryRun": false,
                    "pids": [fixture_pid]
                }
            }
        }),
    )
    .await;
    let rejected = read_response_for_id(&mut reader, 4).await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "confirmationRequired",
        "confirm: false must be rejected: {rejected}"
    );

    // The guarded end of the fixture succeeds with the intact token.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": token,
                    "confirm": true,
                    "dryRun": false,
                    "pids": [fixture_pid]
                }
            }
        }),
    )
    .await;
    let executed = read_response_for_id(&mut reader, 5).await;
    let end_content = &executed["result"]["structuredContent"];
    assert_eq!(
        executed["result"]["isError"], false,
        "the guarded process end must succeed: {executed}"
    );
    assert_eq!(end_content["dryRun"], false);
    assert_eq!(end_content["result"]["endedCount"], 1);
    assert_eq!(
        end_content["result"]["remainingPids"],
        json!([]),
        "remainingPids is the final authority and must be empty: {end_content}"
    );
    assert_eq!(end_content["result"]["items"][0]["status"], "ended");

    // Follow-up liveness check outside the tool protocol: the fixture must be
    // gone. Poll briefly because reaping is asynchronous.
    let mut gone = false;
    for _ in 0..50 {
        let alive = std::process::Command::new("kill")
            .args(["-0", &fixture_pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !alive {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the fixture process {fixture_pid} must be gone");

    // The consumed token cannot be replayed.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": token,
                    "confirm": true,
                    "dryRun": false,
                    "pids": [fixture_pid]
                }
            }
        }),
    )
    .await;
    let replayed = read_response_for_id(&mut reader, 6).await;
    assert_eq!(replayed["result"]["isError"], true);
    assert_eq!(
        replayed["result"]["structuredContent"]["error"]["code"], "tokenUnknown",
        "a consumed token must be single-use: {replayed}"
    );

    // A fresh scan authorizes only what Core's own guards allow: pid 1 is a
    // hard refusal surfaced as a typed guard error.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "processes_scan", "arguments": {} }
        }),
    )
    .await;
    let rescan = read_response_for_id(&mut reader, 7).await;
    let fresh_token = rescan["result"]["structuredContent"]["executionToken"]
        .as_str()
        .expect("the rescan must issue a token")
        .to_string();
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "process_end",
                "arguments": {
                    "token": fresh_token,
                    "confirm": true,
                    "dryRun": false,
                    "pids": [1]
                }
            }
        }),
    )
    .await;
    let guarded = read_response_for_id(&mut reader, 8).await;
    assert_eq!(guarded["result"]["isError"], true);
    assert_eq!(
        guarded["result"]["structuredContent"]["error"]["code"], "invalidInput",
        "ending pid 1 must surface Core's typed guard error: {guarded}"
    );

    drop(stdin);
    let status = timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the server must exit on stdin EOF")
        .expect("the child process must be waitable");
    assert!(status.success(), "unexpected exit status: {status}");

    // Safety net: never leave the fixture behind on assertion failure paths
    // that skipped the guarded end.
    let _ = std::process::Command::new("kill")
        .args(["-9", &fixture_pid.to_string()])
        .status();
    let _ = reaper.join();
}
