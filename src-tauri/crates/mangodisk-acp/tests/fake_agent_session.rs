//! End-to-end tests against a scripted fake ACP agent.
//!
//! The fake agent is this test binary re-executed as a child process: when
//! `MANGODISK_ACP_FAKE_AGENT=1` is set, `fake_agent_entrypoint` speaks
//! newline-delimited JSON-RPC over stdio instead of asserting. This exercises
//! the real provider path — process spawn, stdio pipes, handshake, streaming,
//! permission parking, cancellation, and child reaping — without any real
//! provider CLI.
//!
//! Note: the libtest harness may print a banner line to the child's stdout
//! before the entrypoint runs. That line is not valid JSON-RPC; the bridge
//! tolerates it (the SDK answers malformed input with an error response and
//! continues), and the fake agent ignores any line without a `method`.

use std::io::{BufRead, Write};
use std::time::Duration;

use mangodisk_acp::{
    AcpErrorCode, AgentBridge, AgentProviderDescriptor, AgentSessionConfig, AgentUiEvent,
    CommandSpec, PermissionResolution, ProbeSpec, RunStopReason, StdioMcpServerSpec,
};
use serde_json::{json, Value};

const FAKE_ENV: &str = "MANGODISK_ACP_FAKE_AGENT";
const PID_FILE_ENV: &str = "MANGODISK_ACP_FAKE_PID_FILE";
const SILENT_ENV: &str = "MANGODISK_ACP_FAKE_SILENT";

const EVENT_WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Fake agent (runs only inside the re-executed child process)
// ---------------------------------------------------------------------------

#[test]
fn fake_agent_entrypoint() {
    if std::env::var_os(FAKE_ENV).is_none() {
        return;
    }
    run_fake_agent();
    // Skip libtest's result output so the protocol stream stays clean.
    std::process::exit(0);
}

fn run_fake_agent() {
    if let Ok(pid_file) = std::env::var(PID_FILE_ENV) {
        std::fs::write(pid_file, std::process::id().to_string()).unwrap();
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();

    // libtest prints `test fake_agent_entrypoint ... ` without a trailing
    // newline before running the test. Terminate that partial line first so
    // every protocol line below starts at a column boundary; the banner
    // lines themselves are tolerated by the bridge as malformed input.
    writeln!(stdout).unwrap();
    stdout.flush().unwrap();

    if std::env::var_os(SILENT_ENV).is_some() {
        // Silent mode: never answer anything; used to exercise the handshake
        // timeout and reaping of a half-started provider.
        for line in stdin.lock().lines() {
            if line.is_err() {
                break;
            }
        }
        return;
    }

    let mut cancel_seen = false;
    let mut pending_wait_prompt: Option<Value> = None;
    let mut mcp_names: Vec<String> = Vec::new();
    // (prompt id, our request id) for flows where we await a client response.
    let mut pending_permission: Option<(Value, Value)> = None;
    let mut pending_fs: Option<(Value, Value)> = None;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to one of our requests.
            let id = message.get("id").cloned();
            if let Some((prompt_id, permission_id)) = pending_permission.take() {
                if id.as_ref() == Some(&permission_id) {
                    let outcome = describe_permission_outcome(&message);
                    send_chunk(&mut stdout, &format!("outcome:{outcome}"));
                    respond_prompt(
                        &mut stdout,
                        prompt_id,
                        if cancel_seen { "cancelled" } else { "end_turn" },
                    );
                } else {
                    pending_permission = Some((prompt_id, permission_id));
                }
                continue;
            }
            if let Some((prompt_id, fs_id)) = pending_fs.take() {
                if id.as_ref() == Some(&fs_id) {
                    let code = message
                        .pointer("/error/code")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    send_chunk(&mut stdout, &format!("fs-error:{code}"));
                    respond_prompt(&mut stdout, prompt_id, "end_turn");
                } else {
                    pending_fs = Some((prompt_id, fs_id));
                }
                continue;
            }
            continue;
        };

        match method {
            "initialize" => {
                write_line(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "protocolVersion": 1,
                            "agentCapabilities": {},
                            "authMethods": [],
                            "agentInfo": {"name": "mangodisk-fake-agent", "version": "0.1.0"},
                        }
                    }),
                );
            }
            "session/new" => {
                // Stash for the next default prompt to echo back, proving the
                // MangoDisk MCP server registration reached the agent.
                mcp_names = message
                    .pointer("/params/mcpServers")
                    .and_then(Value::as_array)
                    .map(|servers| {
                        servers
                            .iter()
                            .filter_map(|server| server.get("name").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                write_line(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {"sessionId": "fake-session-1"}
                    }),
                );
            }
            "session/prompt" => {
                let prompt_id = message["id"].clone();
                let text = message
                    .pointer("/params/prompt/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                match text.as_str() {
                    "die" => std::process::exit(3),
                    "error" => {
                        write_line(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": prompt_id,
                                "error": {"code": -32603, "message": "agent exploded on purpose"}
                            }),
                        );
                    }
                    "wait" => {
                        // Announce receipt so the test can cancel
                        // deterministically, then hold the turn open.
                        send_chunk(&mut stdout, "waiting");
                        pending_wait_prompt = Some(prompt_id);
                    }
                    "ask" => {
                        let permission_id = json!("perm-1");
                        write_line(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": permission_id,
                                "method": "session/request_permission",
                                "params": {
                                    "sessionId": "fake-session-1",
                                    "toolCall": {"toolCallId": "tc-perm", "title": "Delete cache"},
                                    "options": [
                                        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                        {"optionId": "deny", "name": "Deny", "kind": "reject_once"}
                                    ]
                                }
                            }),
                        );
                        pending_permission = Some((prompt_id, permission_id));
                    }
                    "fs" => {
                        let fs_id = json!("fs-1");
                        write_line(
                            &mut stdout,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": fs_id,
                                "method": "fs/read_text_file",
                                "params": {"sessionId": "fake-session-1", "path": "/nonexistent"}
                            }),
                        );
                        pending_fs = Some((prompt_id, fs_id));
                    }
                    _ => {
                        send_default_sequence(&mut stdout, &mcp_names);
                        respond_prompt(&mut stdout, prompt_id, "end_turn");
                    }
                }
            }
            "session/cancel" => {
                cancel_seen = true;
                if let Some(prompt_id) = pending_wait_prompt.take() {
                    respond_prompt(&mut stdout, prompt_id, "cancelled");
                }
            }
            _ => {}
        }
    }
}

fn write_line(stdout: &mut impl Write, message: &Value) {
    writeln!(stdout, "{message}").unwrap();
    stdout.flush().unwrap();
}

fn send_chunk(stdout: &mut impl Write, text: &str) {
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "fake-session-1",
                "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}
            }
        }),
    );
}

fn send_default_sequence(stdout: &mut impl Write, mcp_names: &[String]) {
    let mcp_marker = format!("mcp:{}", mcp_names.join(","));
    let updates = [
        json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm "}}),
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hello "}, "messageId": "m1"}),
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "world"}, "messageId": "m1"}),
        json!({"sessionUpdate": "tool_call", "toolCallId": "tc-1", "title": "Scan disk", "kind": "search", "status": "in_progress", "rawInput": {"q": "big files"}}),
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "tc-1", "status": "completed"}),
        json!({"sessionUpdate": "plan", "entries": [{"content": "Scan", "priority": "high", "status": "completed"}]}),
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": mcp_marker}, "messageId": "m2"}),
    ];
    for update in updates {
        write_line(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"sessionId": "fake-session-1", "update": update}
            }),
        );
    }
}

fn respond_prompt(stdout: &mut impl Write, prompt_id: Value, stop_reason: &str) {
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": prompt_id,
            "result": {"stopReason": stop_reason}
        }),
    );
}

fn describe_permission_outcome(response: &Value) -> String {
    match response
        .pointer("/result/outcome/outcome")
        .and_then(Value::as_str)
    {
        Some("selected") => {
            let option = response
                .pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("selected:{option}")
        }
        Some("cancelled") => "cancelled".to_string(),
        _ => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn fake_descriptor(extra_env: &[(&str, &str)]) -> AgentProviderDescriptor {
    let exe = std::env::current_exe().unwrap();
    let mut launch = CommandSpec::new(exe.to_string_lossy().into_owned()).args([
        "--exact",
        "fake_agent_entrypoint",
        "--nocapture",
        "--test-threads=1",
    ]);
    launch = launch.env(FAKE_ENV, "1");
    for (name, value) in extra_env {
        launch = launch.env(*name, *value);
    }
    AgentProviderDescriptor {
        id: "fake".to_string(),
        display_name: "Fake Agent".to_string(),
        launch,
        probe: ProbeSpec::new("mangodisk-fake-agent-unused"),
    }
}

fn test_bridge(descriptor: AgentProviderDescriptor) -> AgentBridge {
    AgentBridge::with_providers(vec![descriptor])
        .with_handshake_timeout(Duration::from_secs(10))
        .with_permission_timeout(Duration::from_millis(500))
}

fn event_types(events: &[AgentUiEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AgentUiEvent::RunStarted { .. } => "run_started",
            AgentUiEvent::RunFinished { .. } => "run_finished",
            AgentUiEvent::RunError { .. } => "run_error",
            AgentUiEvent::TextMessageStart { .. } => "text_start",
            AgentUiEvent::TextMessageContent { .. } => "text_content",
            AgentUiEvent::TextMessageEnd { .. } => "text_end",
            AgentUiEvent::ThinkingMessageStart { .. } => "thinking_start",
            AgentUiEvent::ThinkingMessageContent { .. } => "thinking_content",
            AgentUiEvent::ThinkingMessageEnd { .. } => "thinking_end",
            AgentUiEvent::ToolCallStart { .. } => "tool_call_start",
            AgentUiEvent::ToolCallArgs { .. } => "tool_call_args",
            AgentUiEvent::ToolCallEnd { .. } => "tool_call_end",
            AgentUiEvent::PlanUpdated { .. } => "plan_updated",
            AgentUiEvent::PermissionRequested { .. } => "permission_requested",
        })
        .collect()
}

async fn next_event(
    handle: &mut mangodisk_acp::AgentSessionHandle,
) -> mangodisk_acp::AgentUiEventEnvelope {
    tokio::time::timeout(EVENT_WAIT, handle.next_event())
        .await
        .expect("timed out waiting for an event")
        .expect("event stream ended unexpectedly")
}

/// Collect events until the run terminates (`RunFinished` or `RunError`).
async fn collect_run_events(handle: &mut mangodisk_acp::AgentSessionHandle) -> Vec<AgentUiEvent> {
    let mut events = Vec::new();
    loop {
        let envelope = next_event(handle).await;
        assert_eq!(
            envelope.schema_version,
            mangodisk_acp::AGENT_UI_EVENT_SCHEMA_VERSION
        );
        let is_terminal = matches!(
            envelope.event,
            AgentUiEvent::RunFinished { .. } | AgentUiEvent::RunError { .. }
        );
        events.push(envelope.event);
        if is_terminal {
            return events;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_streams_run_events_in_order() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    let run_id = handle.prompt("hello").await.unwrap();
    let events = collect_run_events(&mut handle).await;

    assert_eq!(
        event_types(&events),
        [
            "run_started",
            "thinking_start",
            "thinking_content",
            "text_start",
            "text_content",
            "text_content",
            "tool_call_start",
            "tool_call_args",
            "tool_call_end",
            "plan_updated",
            "text_end",
            "text_start",
            "text_content",
            "thinking_end",
            "text_end",
            "run_finished",
        ]
    );

    match &events[0] {
        AgentUiEvent::RunStarted { run_id: started } => assert_eq!(*started, run_id),
        other => panic!("expected RunStarted, got {other:?}"),
    }
    // Text content carries the streamed deltas in wire order.
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AgentUiEvent::TextMessageContent { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, ["Hello ", "world", "mcp:"]);
    match events.last() {
        Some(AgentUiEvent::RunFinished { stop_reason, .. }) => {
            assert_eq!(*stop_reason, RunStopReason::EndTurn)
        }
        other => panic!("expected RunFinished, got {other:?}"),
    }

    handle.close().await.unwrap();
}

#[tokio::test]
async fn session_registers_stdio_mcp_server_with_agent() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let config = AgentSessionConfig::new("/tmp").with_mcp_server(StdioMcpServerSpec {
        name: "mangodisk".to_string(),
        command: std::path::PathBuf::from("/usr/bin/false"),
        args: vec![],
        env: vec![],
    });
    let mut handle = bridge.start_session("fake", config).await.unwrap();

    handle.prompt("hello").await.unwrap();
    let events = collect_run_events(&mut handle).await;
    let mcp_chunk = events.iter().find_map(|event| match event {
        AgentUiEvent::TextMessageContent { delta, .. } if delta.starts_with("mcp:") => {
            Some(delta.clone())
        }
        _ => None,
    });
    assert_eq!(mcp_chunk.as_deref(), Some("mcp:mangodisk"));

    handle.close().await.unwrap();
}

#[tokio::test]
async fn permission_request_can_be_approved() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("ask").await.unwrap();

    // First the run starts, then the permission request arrives.
    let run_started = next_event(&mut handle).await;
    assert!(matches!(run_started.event, AgentUiEvent::RunStarted { .. }));
    let permission = next_event(&mut handle).await;
    let AgentUiEvent::PermissionRequested {
        request_id,
        tool_call_id,
        title,
        options,
    } = permission.event
    else {
        panic!("expected PermissionRequested, got {:?}", permission.event);
    };
    assert_eq!(tool_call_id, "tc-perm");
    assert_eq!(title.as_deref(), Some("Delete cache"));
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].option_id, "allow");

    handle
        .resolve_permission(
            request_id,
            PermissionResolution::Selected {
                option_id: "allow".to_string(),
            },
        )
        .unwrap();

    let remaining = collect_run_events(&mut handle).await;
    let deltas: Vec<&str> = remaining
        .iter()
        .filter_map(|event| match event {
            AgentUiEvent::TextMessageContent { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, ["outcome:selected:allow"]);
    assert!(matches!(
        remaining.last(),
        Some(AgentUiEvent::RunFinished {
            stop_reason: RunStopReason::EndTurn,
            ..
        })
    ));

    handle.close().await.unwrap();
}

#[tokio::test]
async fn permission_request_auto_declines_on_timeout() {
    // Permission timeout is 500ms in the test bridge; nobody resolves.
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("ask").await.unwrap();
    let events = collect_run_events(&mut handle).await;

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentUiEvent::PermissionRequested { .. })));
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AgentUiEvent::TextMessageContent { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, ["outcome:cancelled"]);

    // The request is gone; resolving late fails with a typed error.
    let request_id = events
        .iter()
        .find_map(|event| match event {
            AgentUiEvent::PermissionRequested { request_id, .. } => Some(*request_id),
            _ => None,
        })
        .unwrap();
    let error = handle
        .resolve_permission(request_id, PermissionResolution::Declined)
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::PermissionNotPending);

    handle.close().await.unwrap();
}

#[tokio::test]
async fn cancel_ends_run_and_declines_pending_permission() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("ask").await.unwrap();
    let run_started = next_event(&mut handle).await;
    assert!(matches!(run_started.event, AgentUiEvent::RunStarted { .. }));
    let permission = next_event(&mut handle).await;
    let AgentUiEvent::PermissionRequested { request_id, .. } = permission.event else {
        panic!("expected PermissionRequested, got {:?}", permission.event);
    };

    handle.cancel().await.unwrap();

    // The parked permission is resolved as cancelled, and the turn ends with
    // a cancelled stop reason.
    let error = handle
        .resolve_permission(request_id, PermissionResolution::Declined)
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::PermissionNotPending);

    let remaining = collect_run_events(&mut handle).await;
    assert!(matches!(
        remaining.last(),
        Some(AgentUiEvent::RunFinished {
            stop_reason: RunStopReason::Cancelled,
            ..
        })
    ));

    handle.close().await.unwrap();
}

#[tokio::test]
async fn cancel_stops_waiting_run() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("wait").await.unwrap();

    // Wait for the fake agent to acknowledge the prompt before cancelling.
    let mut events = Vec::new();
    loop {
        let envelope = next_event(&mut handle).await;
        let is_waiting = matches!(
            &envelope.event,
            AgentUiEvent::TextMessageContent { delta, .. } if delta == "waiting"
        );
        events.push(envelope.event);
        if is_waiting {
            break;
        }
    }

    handle.cancel().await.unwrap();
    events.extend(collect_run_events(&mut handle).await);
    assert!(matches!(
        events.last(),
        Some(AgentUiEvent::RunFinished {
            stop_reason: RunStopReason::Cancelled,
            ..
        })
    ));

    handle.close().await.unwrap();
}

#[tokio::test]
async fn unsupported_capability_request_is_declined_with_typed_error() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("fs").await.unwrap();
    let events = collect_run_events(&mut handle).await;
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AgentUiEvent::TextMessageContent { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    // JSON-RPC method_not_found is -32601.
    assert_eq!(deltas, ["fs-error:-32601"]);

    handle.close().await.unwrap();
}

#[tokio::test]
async fn provider_exit_mid_run_fails_run_with_typed_code() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("die").await.unwrap();
    let events = collect_run_events(&mut handle).await;
    assert_eq!(event_types(&events), ["run_started", "run_error"]);
    match events.last() {
        Some(AgentUiEvent::RunError { code, .. }) => {
            assert_eq!(*code, AcpErrorCode::ProviderExited)
        }
        other => panic!("expected RunError, got {other:?}"),
    }

    // The event stream ends once the connection is gone.
    let tail = tokio::time::timeout(EVENT_WAIT, handle.next_event())
        .await
        .expect("event stream should end after provider exit");
    assert!(tail.is_none());
    assert!(!handle.is_alive());

    handle.close().await.unwrap();
}

#[tokio::test]
async fn agent_prompt_error_fails_run_but_keeps_session() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();

    handle.prompt("error").await.unwrap();
    let events = collect_run_events(&mut handle).await;
    assert_eq!(event_types(&events), ["run_started", "run_error"]);
    match events.last() {
        Some(AgentUiEvent::RunError { code, message, .. }) => {
            assert_eq!(*code, AcpErrorCode::PromptFailed);
            assert!(message.contains("agent exploded on purpose"));
        }
        other => panic!("expected RunError, got {other:?}"),
    }

    // The connection survives an agent-level prompt error: a follow-up
    // prompt still streams a normal run.
    assert!(handle.is_alive());
    handle.prompt("hello").await.unwrap();
    let events = collect_run_events(&mut handle).await;
    assert!(matches!(
        events.last(),
        Some(AgentUiEvent::RunFinished {
            stop_reason: RunStopReason::EndTurn,
            ..
        })
    ));

    handle.close().await.unwrap();
}

#[tokio::test]
async fn unknown_provider_fails_closed() {
    let bridge = test_bridge(fake_descriptor(&[]));
    let error = bridge
        .start_session("nope", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::ProviderUnknown);
}

#[tokio::test]
async fn missing_binary_is_unavailable() {
    let mut descriptor = fake_descriptor(&[]);
    descriptor.launch = CommandSpec::new("mangodisk-definitely-missing-binary");
    let bridge = test_bridge(descriptor);
    let error = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::ProviderUnavailable);
}

#[tokio::test]
#[cfg(unix)]
async fn non_executable_binary_reports_spawn_failed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-executable");
    std::fs::write(&path, b"not a program\n").unwrap();

    let mut descriptor = fake_descriptor(&[]);
    descriptor.launch = CommandSpec::new(path.to_string_lossy().into_owned());
    let bridge = test_bridge(descriptor);
    let error = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::SpawnFailed);
}

#[tokio::test]
async fn handshake_timeout_kills_half_started_provider() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let descriptor = fake_descriptor(&[
        (SILENT_ENV, "1"),
        (PID_FILE_ENV, pid_file.to_str().unwrap()),
    ]);
    let bridge = AgentBridge::with_providers(vec![descriptor])
        .with_handshake_timeout(Duration::from_millis(500));

    let error = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), AcpErrorCode::Timeout);

    assert_process_gone(&pid_file).await;
}

#[tokio::test]
async fn close_reaps_provider_process() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let descriptor = fake_descriptor(&[(PID_FILE_ENV, pid_file.to_str().unwrap())]);
    let bridge = test_bridge(descriptor);

    let mut handle = bridge
        .start_session("fake", AgentSessionConfig::new("/tmp"))
        .await
        .unwrap();
    handle.prompt("hello").await.unwrap();
    let _ = collect_run_events(&mut handle).await;
    handle.close().await.unwrap();

    assert_process_gone(&pid_file).await;
}

/// Assert the recorded child pid is dead. A zombie counts as dead: the SDK
/// kills the process and the async-process reaper collects it asynchronously,
/// so the zombie state is a transient, expected step.
///
/// The poll uses Tokio's sleep: the test runtime is single-threaded, and a
/// blocking sleep would starve the very task cleanup being asserted on.
#[cfg(unix)]
async fn assert_process_gone(pid_file: &std::path::Path) {
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let exists = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| {
                stat.rsplit(')')
                    .nth(1)
                    .is_some_and(|rest| rest.trim_start().starts_with('Z'))
            })
            .unwrap_or(false);
        if !exists || zombie {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "process {pid} is still alive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(not(unix))]
async fn assert_process_gone(_pid_file: &std::path::Path) {
    // Process-group reaping is asserted on unix only; on Windows the SDK
    // kills the direct child and the assert would need OS-specific tooling.
}
