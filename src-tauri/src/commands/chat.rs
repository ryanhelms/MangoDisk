//! In-app AI chat adapter over the ACP bridge (`mangodisk-acp`).
//!
//! The adapter owns three things and nothing more: provider probing (lazy,
//! only when the chat page asks), a registry of live provider sessions keyed
//! by adapter-assigned ids, and one forwarder task per session that republishes
//! the bridge's AG-UI event stream on a per-session Tauri channel. All disk
//! access reaches the agent through the `mangodisk-mcp` sidecar, registered
//! with the session via ACP `session/new`.
//!
//! Sidecar distribution: the binary is resolved at runtime (see
//! [`resolve_mcp_sidecar`]) instead of being declared as a Tauri
//! `externalBin`. A gitignored sidecar copy under `binaries/` cannot work here
//! because the cross-platform CI build runs without producing it first, and a
//! missing `externalBin` file fails the whole application build — `tauri-build`
//! copies declared sidecars during the app's build script, so even plain
//! `cargo check`/`cargo test` panic on a fresh clone. Shipping the sidecar
//! inside release bundles is a deliberate follow-up that must add the build
//! step to CI in the same change. Until then a missing binary fails the
//! session start closed with the typed `agentSidecarUnavailable` reason.
//!
//! Privacy: provider binary paths, the resolved sidecar path, and agent stderr
//! never cross into logs, events, or command results. The frontend receives
//! provider ids, display names, capped version strings, and typed error codes.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mangodisk_acp::{
    probe_available_providers, AgentBridge, AgentSessionConfig, AgentSessionHandle,
    AgentUiEventEnvelope, PermissionResolution, StdioMcpServerSpec,
};
use serde::Serialize;
use tauri::Manager;
use tokio::sync::Mutex;

use super::error::{into_command_result, CommandError, CommandResult};
use crate::events;

/// Environment override for the `mangodisk-mcp` binary, used by development
/// and support setups that keep the sidecar outside the bundle.
const MCP_SIDECAR_ENV: &str = "MANGODISK_MCP_BIN";
/// Server name the agent displays for MangoDisk's tool server.
const MCP_SERVER_NAME: &str = "mangodisk";

/// Tauri managed state holding the ACP bridge and every live chat session.
///
/// Dropping the registry (application exit) drops each
/// [`AgentSessionHandle`], which aborts the connection task and kills the
/// provider process group through the ACP SDK's child guard.
pub struct ChatSessionRegistry {
    bridge: AgentBridge,
    sessions: Mutex<HashMap<String, AgentSessionHandle>>,
    next_session_seq: AtomicU64,
}

impl Default for ChatSessionRegistry {
    fn default() -> Self {
        Self {
            bridge: AgentBridge::new(),
            sessions: Mutex::new(HashMap::new()),
            next_session_seq: AtomicU64::new(1),
        }
    }
}

impl ChatSessionRegistry {
    fn allocate_session_id(&self) -> String {
        format!(
            "chat-{}",
            self.next_session_seq.fetch_add(1, Ordering::SeqCst)
        )
    }
}

/// Provider entry shown by the chat page picker. The resolved binary path is
/// deliberately excluded: the UI never needs it and it is private machine
/// data.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAgentInfo {
    id: String,
    display_name: String,
    version: Option<String>,
}

/// Result of a started chat session. `sessionId` is the adapter-assigned key
/// used by every subsequent chat command and by the per-session event channel.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionInfo {
    session_id: String,
    provider_id: String,
    provider_display_name: String,
    mutations_enabled: bool,
}

/// Terminal notice emitted on `chat-session-ended-<id>` after the last stream
/// event, when the provider connection closes or the session is closed
/// locally. An active run always observes its own `RUN_ERROR` envelope first,
/// so this payload only marks the stream itself as finished.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatSessionEndedPayload {
    session_id: String,
}

/// Probes which provider CLIs are installed and answering on this machine.
/// Called lazily from the chat page; absent or unresponsive providers simply
/// do not appear, so an empty list is a valid result, not an error.
#[tauri::command]
pub async fn chat_probe_agents() -> CommandResult<Vec<ChatAgentInfo>> {
    let probed = probe_available_providers().await;
    log::info!("chat_agents_probed available={}", probed.len());
    Ok(probed
        .into_iter()
        .map(|provider| ChatAgentInfo {
            id: provider.descriptor.id,
            display_name: provider.descriptor.display_name,
            version: provider.version,
        })
        .collect())
}

/// Starts one provider session and registers the MangoDisk MCP sidecar with
/// it. Mutation tools stay disabled unless `enable_mutations` is explicitly
/// true; the sidecar also enforces that gate server-side.
#[tauri::command]
pub async fn chat_start_session(
    app: tauri::AppHandle,
    registry: tauri::State<'_, ChatSessionRegistry>,
    provider_id: String,
    enable_mutations: bool,
) -> CommandResult<ChatSessionInfo> {
    let provider_id = provider_id.trim().to_string();
    if provider_id.is_empty() {
        return Err(CommandError::invalid_input(
            "chat_start_session",
            "providerIdEmpty",
        ));
    }
    let Some(sidecar) = resolve_mcp_sidecar_at_runtime() else {
        return Err(CommandError::adapter_failure(
            "chat_start_session",
            "agentSidecarUnavailable",
        ));
    };

    let mut server_args = Vec::new();
    if enable_mutations {
        server_args.push("--enable-mutations".to_string());
    }
    let session_config =
        AgentSessionConfig::new(chat_working_directory(&app)).with_mcp_server(StdioMcpServerSpec {
            name: MCP_SERVER_NAME.to_string(),
            command: sidecar,
            args: server_args,
            env: Vec::new(),
        });

    let display_name = registry
        .bridge
        .providers()
        .iter()
        .find(|descriptor| descriptor.id == provider_id)
        .map(|descriptor| descriptor.display_name.clone())
        .unwrap_or_else(|| provider_id.clone());
    let handle = into_command_result(
        "chat_start_session",
        registry
            .bridge
            .start_session(&provider_id, session_config)
            .await,
    )?;
    start_forwarded_session(
        &app,
        &registry,
        provider_id.clone(),
        display_name,
        enable_mutations,
        handle,
    )
    .await
}

/// Registers one started bridge session: allocates the adapter session id,
/// spawns its event forwarder, and stores the handle for later commands.
async fn start_forwarded_session(
    app: &tauri::AppHandle,
    registry: &ChatSessionRegistry,
    provider_id: String,
    provider_display_name: String,
    mutations_enabled: bool,
    mut handle: AgentSessionHandle,
) -> CommandResult<ChatSessionInfo> {
    let event_receiver = handle.take_event_receiver();
    let session_id = registry.allocate_session_id();
    spawn_chat_event_forwarder(app.clone(), session_id.clone(), event_receiver);
    registry
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), handle);
    log::info!(
        "chat_session_started session={session_id} provider={provider_id} mutations_enabled={mutations_enabled}"
    );
    Ok(ChatSessionInfo {
        session_id,
        provider_id,
        provider_display_name,
        mutations_enabled,
    })
}

/// Sends one user prompt to the session's agent. The run id is also announced
/// through the `RUN_STARTED` event; returning it here lets the caller
/// correlate without waiting for the event round-trip.
#[tauri::command]
pub async fn chat_send_prompt(
    registry: tauri::State<'_, ChatSessionRegistry>,
    session_id: String,
    text: String,
) -> CommandResult<String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(CommandError::invalid_input(
            "chat_send_prompt",
            "promptEmpty",
        ));
    }
    let sessions = registry.sessions.lock().await;
    let Some(session) = sessions.get(&session_id) else {
        return Err(CommandError::invalid_input(
            "chat_send_prompt",
            "chatSessionUnknown",
        ));
    };
    into_command_result("chat_send_prompt", session.prompt(text).await)
}

/// Cancels the session's current prompt turn (`session/cancel`). Pending
/// permission prompts are answered with the protocol's cancelled outcome by
/// the bridge.
#[tauri::command]
pub async fn chat_cancel(
    registry: tauri::State<'_, ChatSessionRegistry>,
    session_id: String,
) -> CommandResult<()> {
    let sessions = registry.sessions.lock().await;
    let Some(session) = sessions.get(&session_id) else {
        return Err(CommandError::invalid_input(
            "chat_cancel",
            "chatSessionUnknown",
        ));
    };
    into_command_result("chat_cancel", session.cancel().await)
}

/// Answers a permission prompt surfaced as a `PERMISSION_REQUESTED` event.
/// `option_id` selects one of the offered options; `None` declines the
/// request, which maps to the protocol's cancelled outcome.
#[tauri::command]
pub async fn chat_resolve_permission(
    registry: tauri::State<'_, ChatSessionRegistry>,
    session_id: String,
    request_id: u64,
    option_id: Option<String>,
) -> CommandResult<()> {
    let sessions = registry.sessions.lock().await;
    let Some(session) = sessions.get(&session_id) else {
        return Err(CommandError::invalid_input(
            "chat_resolve_permission",
            "chatSessionUnknown",
        ));
    };
    let resolution = match option_id {
        Some(option_id) => PermissionResolution::Selected { option_id },
        None => PermissionResolution::Declined,
    };
    into_command_result(
        "chat_resolve_permission",
        session.resolve_permission(request_id, resolution),
    )
}

/// Ends the session and reaps the provider process. Closing is idempotent so
/// the page can tear down defensively after a provider-initiated ending.
#[tauri::command]
pub async fn chat_close_session(
    registry: tauri::State<'_, ChatSessionRegistry>,
    session_id: String,
) -> CommandResult<()> {
    let Some(handle) = registry.sessions.lock().await.remove(&session_id) else {
        log::debug!("chat_session_close_skipped session={session_id} reason=already_closed");
        return Ok(());
    };
    log::info!("chat_session_closing session={session_id}");
    into_command_result("chat_close_session", handle.close().await)
}

/// Republishes one session's AG-UI event stream on its per-session Tauri
/// channel. When the bridge closes the stream (provider exit or local close),
/// a terminal `chat-session-ended-<id>` notice is emitted so the frontend can
/// settle its session state instead of waiting forever.
fn spawn_chat_event_forwarder(
    app: tauri::AppHandle,
    session_id: String,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<AgentUiEventEnvelope>,
) {
    tauri::async_runtime::spawn(async move {
        let event_name = events::chat_session_event_name(&session_id);
        while let Some(envelope) = receiver.recv().await {
            events::emit_dynamic(&app, &event_name, envelope);
        }
        let ended_name = events::chat_session_ended_event_name(&session_id);
        events::emit_dynamic(
            &app,
            &ended_name,
            ChatSessionEndedPayload {
                session_id: session_id.clone(),
            },
        );
        log::debug!("chat_session_event_stream_closed session={session_id}");
    });
}

/// The agent's working directory. MangoDisk advertises no fs/terminal client
/// capabilities, so this path is only the anchor the provider CLI resolves
/// relative paths against; the user's home directory is the least surprising
/// anchor for a desktop chat.
fn chat_working_directory(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .home_dir()
        .ok()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn sidecar_executable_name() -> &'static str {
    if cfg!(windows) {
        "mangodisk-mcp.exe"
    } else {
        "mangodisk-mcp"
    }
}

/// Resolves the `mangodisk-mcp` binary at runtime:
/// 1. `MANGODISK_MCP_BIN` override (authoritative; a missing file fails closed);
/// 2. next to the application executable — where a future `externalBin` copy
///    or a manual side-by-side install would place it;
/// 3. the workspace `target/debug` or `target/release` directory, for plain
///    `cargo run` launches where the executable sits in `target/*/deps`.
fn resolve_mcp_sidecar(
    env_override: Option<OsString>,
    executable_dir: Option<&Path>,
    workspace_target_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(override_path) = env_override.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(override_path);
        return path.is_file().then_some(path);
    }

    let name = sidecar_executable_name();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(directory) = executable_dir {
        candidates.push(directory.join(name));
    }
    if let Some(target_dir) = workspace_target_dir {
        candidates.push(target_dir.join("debug").join(name));
        candidates.push(target_dir.join("release").join(name));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn resolve_mcp_sidecar_at_runtime() -> Option<PathBuf> {
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let workspace_target_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|workspace_root| workspace_root.join("target"));
    let resolved = resolve_mcp_sidecar(
        std::env::var_os(MCP_SIDECAR_ENV),
        executable_dir.as_deref(),
        workspace_target_dir.as_deref(),
    );
    if resolved.is_none() {
        log::warn!("chat_mcp_sidecar_unavailable");
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"stub").expect("fixture file must be writable");
    }

    #[test]
    fn env_override_is_authoritative_when_the_file_exists() {
        let directory = tempfile::tempdir().expect("tempdir");
        let sidecar = directory.path().join(sidecar_executable_name());
        touch(&sidecar);

        let resolved = resolve_mcp_sidecar(
            Some(sidecar.clone().into_os_string()),
            None,
            Some(directory.path()),
        );

        assert_eq!(resolved, Some(sidecar));
    }

    #[test]
    fn env_override_fails_closed_when_the_file_is_missing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fallback = directory
            .path()
            .join("debug")
            .join(sidecar_executable_name());
        std::fs::create_dir_all(fallback.parent().expect("parent")).expect("fixture directory");
        touch(&fallback);

        let resolved = resolve_mcp_sidecar(
            Some(directory.path().join("missing-mcp").into_os_string()),
            None,
            Some(directory.path()),
        );

        assert_eq!(resolved, None);
    }

    #[test]
    fn executable_sibling_wins_over_workspace_target() {
        let directory = tempfile::tempdir().expect("tempdir");
        let exe_dir = directory.path().join("exe");
        let target_debug = directory.path().join("target").join("debug");
        std::fs::create_dir_all(&exe_dir).expect("fixture directory");
        std::fs::create_dir_all(&target_debug).expect("fixture directory");
        let sibling = exe_dir.join(sidecar_executable_name());
        touch(&sibling);
        touch(&target_debug.join(sidecar_executable_name()));

        let resolved =
            resolve_mcp_sidecar(None, Some(&exe_dir), Some(&directory.path().join("target")));

        assert_eq!(resolved, Some(sibling));
    }

    #[test]
    fn workspace_target_fallback_covers_plain_cargo_runs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let release = directory.path().join("release");
        std::fs::create_dir_all(&release).expect("fixture directory");
        let sidecar = release.join(sidecar_executable_name());
        touch(&sidecar);

        let resolved = resolve_mcp_sidecar(None, Some(directory.path()), Some(directory.path()));

        assert_eq!(resolved, Some(sidecar));
    }

    #[test]
    fn missing_sidecar_everywhere_resolves_to_none() {
        let directory = tempfile::tempdir().expect("tempdir");

        let resolved = resolve_mcp_sidecar(None, Some(directory.path()), Some(directory.path()));

        assert_eq!(resolved, None);
    }
}
