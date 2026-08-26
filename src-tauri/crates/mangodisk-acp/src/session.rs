//! Session lifecycle: provider process spawning, ACP handshake, prompt
//! streaming, cancellation, and permission routing.
//!
//! One session owns one provider child process. The child is spawned through
//! the ACP SDK's [`AcpAgent`] transport, which (on unix) puts the provider in
//! its own process group and kills the whole group when the connection ends —
//! wrapper launchers such as `npx` cannot orphan the real agent behind our
//! back. Closing a session (or dropping the handle) tears down the
//! connection, which is what reaps the child.
//!
//! Privacy note: provider stderr may contain user paths or file contents, so
//! it is never logged or forwarded. Errors carry typed codes plus fixed or
//! agent-authored message text only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, EnvVariable, Implementation,
    InitializeRequest, McpServer, McpServerStdio, NewSessionRequest, ReadTextFileRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, TextContent, WriteTextFileRequest,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Handled, JsonRpcResponse, Responder,
};
use tokio::sync::{mpsc, oneshot};

use crate::event::{
    AgentPermissionOption, AgentUiEvent, AgentUiEventEnvelope, PermissionOptionKind,
};
use crate::provider::AgentProviderDescriptor;
use crate::translate::EventTranslator;
use crate::{AcpError, AcpErrorCode, AcpResult};

/// Default deadline for the `initialize` + `session/new` handshake. Provider
/// CLIs normally boot in well under a second; the headroom covers wrapper
/// launchers resolving packages on first run.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Default deadline for answering a permission request before the bridge
/// auto-declines it. Matches the interactive window a user needs to read and
/// judge a destructive action.
pub const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

/// Grace period for the connection task to wind down during [`AgentSessionHandle::close`]
/// before it is aborted (which still kills the provider process).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cap on agent-authored error text copied into diagnostics.
const MAX_REMOTE_ERROR_LEN: usize = 200;

/// A stdio MCP server the agent should connect to when the session starts.
///
/// This is how MangoDisk hands its own disk tools (`mangodisk-mcp`) to the
/// chat agent without the provider needing any MangoDisk-specific setup.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StdioMcpServerSpec {
    /// Server name as the agent will display it.
    pub name: String,
    /// Absolute path to the MCP server executable.
    pub command: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
}

/// Parameters for opening one provider session.
#[derive(Debug, Clone, Default)]
pub struct AgentSessionConfig {
    /// Working directory for the session. ACP requires an absolute path.
    pub cwd: PathBuf,
    /// stdio MCP servers registered through ACP `session/new`.
    pub mcp_servers: Vec<StdioMcpServerSpec>,
}

impl AgentSessionConfig {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            mcp_servers: Vec::new(),
        }
    }

    pub fn with_mcp_server(mut self, server: StdioMcpServerSpec) -> Self {
        self.mcp_servers.push(server);
        self
    }
}

/// Entry point of the crate: holds the provider catalog and timeouts, and
/// opens sessions.
///
/// All methods must be called from within a Tokio runtime (the Tauri adapter
/// runs on one). The bridge itself is cheap to construct and keeps no per-
/// session state; each session is owned by its [`AgentSessionHandle`].
#[derive(Debug, Clone)]
pub struct AgentBridge {
    catalog: Vec<AgentProviderDescriptor>,
    handshake_timeout: Duration,
    permission_timeout: Duration,
}

impl Default for AgentBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBridge {
    /// Bridge over the built-in provider catalog.
    pub fn new() -> Self {
        Self::with_providers(crate::provider::default_providers())
    }

    /// Bridge over a caller-supplied catalog (extend or override the defaults
    /// from [`crate::default_providers`] before passing them here).
    pub fn with_providers(catalog: Vec<AgentProviderDescriptor>) -> Self {
        Self {
            catalog,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            permission_timeout: DEFAULT_PERMISSION_TIMEOUT,
        }
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub fn with_permission_timeout(mut self, timeout: Duration) -> Self {
        self.permission_timeout = timeout;
        self
    }

    /// The catalog this bridge launches from.
    pub fn providers(&self) -> &[AgentProviderDescriptor] {
        &self.catalog
    }

    /// Open a session with a provider from the catalog.
    pub async fn start_session(
        &self,
        provider_id: &str,
        config: AgentSessionConfig,
    ) -> AcpResult<AgentSessionHandle> {
        let descriptor = self
            .catalog
            .iter()
            .find(|descriptor| descriptor.id == provider_id)
            .ok_or_else(|| AcpError::provider_unknown(provider_id))?;
        self.start_session_for(descriptor, config).await
    }

    /// Open a session with an explicit descriptor, bypassing the catalog.
    /// Useful for per-machine overrides and for tests driving a fake agent.
    pub async fn start_session_for(
        &self,
        descriptor: &AgentProviderDescriptor,
        config: AgentSessionConfig,
    ) -> AcpResult<AgentSessionHandle> {
        // Fail closed before touching the process API: an unresolvable
        // program is reported as unavailable, never attempted.
        let resolved = crate::launch::resolve_program(&descriptor.launch.program)
            .ok_or_else(|| AcpError::provider_unavailable(&descriptor.id))?;
        let spawn = crate::launch::spawn_spec(&descriptor.launch, &resolved);

        let agent = AcpAgent::new(
            AcpAgentConfig::new(spawn.program)
                .args(spawn.args)
                .envs(spawn.env),
        );

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (handshake_tx, handshake_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let shared = Arc::new(SessionShared::new(event_tx, self.permission_timeout));
        let launch = SessionLaunch::new(config);

        let mut connection_task = tokio::spawn(run_connection(
            descriptor.id.clone(),
            agent,
            launch,
            shared.clone(),
            handshake_tx,
            shutdown_rx,
        ));

        match tokio::time::timeout(self.handshake_timeout, handshake_rx).await {
            Ok(Ok(Ok(handshake))) => {
                log::debug!("ACP session started: provider={}", descriptor.id);
                Ok(AgentSessionHandle {
                    provider_id: descriptor.id.clone(),
                    session_id: handshake.session_id.0.to_string(),
                    connection: handshake.connection,
                    shared,
                    event_rx,
                    shutdown_tx: Some(shutdown_tx),
                    connection_task,
                })
            }
            Ok(Ok(Err(error))) => {
                // main_fn reported a typed handshake failure and is already
                // winding the connection down; wait briefly for the child to
                // be reaped so callers can immediately retry.
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, &mut connection_task).await;
                Err(error)
            }
            Ok(Err(_dropped)) => {
                // The handshake channel closed without an answer: the
                // transport failed before or during the handshake (spawn
                // failure, instant exit). The connection task holds the
                // mapped cause.
                match tokio::time::timeout(CLOSE_TIMEOUT, &mut connection_task).await {
                    Ok(Ok(Err(error))) => Err(error),
                    Ok(Ok(Ok(()))) => Err(AcpError::handshake_failed(
                        "provider connection closed before the handshake completed",
                    )),
                    Ok(Err(_join_error)) => Err(AcpError::handshake_failed(
                        "connection task panicked during the handshake",
                    )),
                    Err(_) => Err(AcpError::handshake_failed(
                        "connection task did not report the handshake failure",
                    )),
                }
            }
            Err(_elapsed) => {
                // Aborting drops the connection future, and with it the SDK's
                // child guard: the half-started provider process is killed.
                // Awaiting the aborted task makes the kill deterministic
                // instead of depending on when the runtime next schedules it.
                connection_task.abort();
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, &mut connection_task).await;
                Err(AcpError::timeout(format!(
                    "ACP handshake did not complete within {:?}",
                    self.handshake_timeout
                )))
            }
        }
    }
}

/// Handle to one live provider session.
///
/// Events stream in through [`next_event`](Self::next_event) (or a receiver
/// taken with [`take_event_receiver`](Self::take_event_receiver`)); prompts,
/// cancellation, and permission answers go the other way. Dropping the handle
/// without [`close`](Self::close) aborts the connection task, which kills the
/// provider process via the SDK child guard — no orphaned provider processes.
pub struct AgentSessionHandle {
    provider_id: String,
    session_id: String,
    connection: ConnectionTo<Agent>,
    shared: Arc<SessionShared>,
    event_rx: mpsc::UnboundedReceiver<AgentUiEventEnvelope>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    connection_task: tokio::task::JoinHandle<Result<(), AcpError>>,
}

impl std::fmt::Debug for AgentSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSessionHandle")
            .field("provider_id", &self.provider_id)
            .field("session_id", &self.session_id)
            .field("alive", &self.is_alive())
            .finish_non_exhaustive()
    }
}

impl AgentSessionHandle {
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// The ACP session id assigned by the agent.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Whether the connection task is still running. `false` means the
    /// provider exited or the connection was lost; the event stream then ends
    /// once already-queued events are drained.
    pub fn is_alive(&self) -> bool {
        !self.shared.closed.load(Ordering::SeqCst) && !self.connection_task.is_finished()
    }

    /// Next UI event, or `None` once the session has ended and every queued
    /// event has been delivered.
    pub async fn next_event(&mut self) -> Option<AgentUiEventEnvelope> {
        self.event_rx.recv().await
    }

    /// Hand the event stream to a dedicated consumer (e.g. a Tauri event
    /// forwarder). Calling it again returns a receiver that is immediately
    /// closed; prefer moving the first receiver.
    pub fn take_event_receiver(&mut self) -> mpsc::UnboundedReceiver<AgentUiEventEnvelope> {
        let placeholder = mpsc::unbounded_channel().1;
        std::mem::replace(&mut self.event_rx, placeholder)
    }

    /// Send a user prompt. Returns the bridge-assigned run id.
    ///
    /// The response streams back as events: `RunStarted` is queued before the
    /// request goes out; the run ends with `RunFinished` or `RunError`.
    pub async fn prompt(&self, text: impl Into<String>) -> AcpResult<String> {
        if !self.is_alive() {
            return Err(AcpError::session_lost(
                "cannot prompt: the provider connection has ended",
            ));
        }

        let (run_id, events) = self.shared.begin_run();
        self.shared.emit_all(events);

        let request = agent_client_protocol::schema::v1::PromptRequest::new(
            SessionId::from(self.session_id.clone()),
            vec![ContentBlock::Text(TextContent::new(text.into()))],
        );
        let sent = self.connection.send_request(request);

        // Await the turn response in a Tokio task (never inside the SDK
        // dispatch loop, where blocking would deadlock the connection) and
        // translate the outcome into the terminal run event.
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let events = match sent.block_task().await {
                Ok(response) => shared.finish_run(response.stop_reason),
                Err(error) => {
                    if is_transport_level_failure(&error) {
                        // The connection is ending; the connection task
                        // classifies the real cause and closes the run.
                        // Emitting here too would race it with a wrong code.
                        Vec::new()
                    } else {
                        shared.fail_run(
                            AcpErrorCode::PromptFailed,
                            &format!("agent failed the prompt turn: {}", remote_message(&error)),
                        )
                    }
                }
            };
            shared.emit_all(events);
        });

        Ok(run_id)
    }

    /// Cancel the current prompt turn (`session/cancel`).
    ///
    /// Per the ACP spec, pending permission requests are answered with the
    /// `cancelled` outcome; the agent then ends the turn with a `cancelled`
    /// stop reason, which surfaces as `RunFinished`.
    pub async fn cancel(&self) -> AcpResult<()> {
        self.connection
            .send_notification(CancelNotification::new(self.session_id.clone()))
            .map_err(|_| {
                AcpError::session_lost("cannot cancel: the provider connection has ended")
            })?;
        self.shared.cancel_pending_permissions();
        Ok(())
    }

    /// Answer a pending permission request delivered earlier as
    /// [`AgentUiEvent::PermissionRequested`]. Unknown or already-resolved ids
    /// fail with [`AcpErrorCode::PermissionNotPending`].
    pub fn resolve_permission(
        &self,
        request_id: u64,
        resolution: crate::PermissionResolution,
    ) -> AcpResult<()> {
        let tx = self
            .shared
            .pending_permissions
            .lock()
            .expect("permission registry mutex poisoned")
            .remove(&request_id)
            .ok_or_else(|| AcpError::permission_not_pending(request_id))?;

        let outcome = match resolution {
            crate::PermissionResolution::Selected { option_id } => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
            }
            crate::PermissionResolution::Declined => RequestPermissionOutcome::Cancelled,
        };

        tx.send(outcome).map_err(|_| {
            AcpError::session_lost("permission responder is gone: the connection has ended")
        })
    }

    /// Gracefully end the session: the connection task shuts down and the
    /// provider process (including its process group on unix) is killed by
    /// the SDK's child guard. If the task does not wind down in time it is
    /// aborted, which has the same reaping effect.
    pub async fn close(mut self) -> AcpResult<()> {
        if let Some(shutdown) = self.shutdown_tx.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(CLOSE_TIMEOUT, &mut self.connection_task)
            .await
            .is_err()
        {
            log::warn!(
                "provider connection did not shut down in time; aborting: provider={}",
                self.provider_id
            );
            self.connection_task.abort();
        }
        Ok(())
    }
}

impl Drop for AgentSessionHandle {
    fn drop(&mut self) {
        // Safety net for callers that forget `close`: dropping the connection
        // future drops the SDK child guard, which kills the provider process.
        self.connection_task.abort();
    }
}

/// Shared per-session state between the handle, the connection task, and the
/// parked permission waiters.
struct SessionShared {
    translator: Mutex<EventTranslator>,
    pending_permissions: Mutex<HashMap<u64, oneshot::Sender<RequestPermissionOutcome>>>,
    next_permission_seq: AtomicU64,
    event_seq: AtomicU64,
    /// `None` once the connection has ended; taking the sender out is what
    /// closes the event stream for the handle.
    event_tx: Mutex<Option<mpsc::UnboundedSender<AgentUiEventEnvelope>>>,
    permission_timeout: Duration,
    closed: AtomicBool,
}

impl SessionShared {
    fn new(
        event_tx: mpsc::UnboundedSender<AgentUiEventEnvelope>,
        permission_timeout: Duration,
    ) -> Self {
        Self {
            translator: Mutex::new(EventTranslator::default()),
            pending_permissions: Mutex::new(HashMap::new()),
            next_permission_seq: AtomicU64::new(0),
            event_seq: AtomicU64::new(0),
            event_tx: Mutex::new(Some(event_tx)),
            permission_timeout,
            closed: AtomicBool::new(false),
        }
    }

    fn emit(&self, event: AgentUiEvent) {
        let guard = self.event_tx.lock().expect("event sender mutex poisoned");
        if let Some(tx) = guard.as_ref() {
            let seq = self.event_seq.fetch_add(1, Ordering::SeqCst);
            // A send failure means the consumer is gone; events are then
            // moot, so it is deliberately ignored.
            let _ = tx.send(AgentUiEventEnvelope::new(seq, event));
        }
    }

    fn emit_all(&self, events: Vec<AgentUiEvent>) {
        for event in events {
            self.emit(event);
        }
    }

    fn begin_run(&self) -> (String, Vec<AgentUiEvent>) {
        self.translator
            .lock()
            .expect("translator mutex poisoned")
            .begin_run()
    }

    fn finish_run(
        &self,
        stop_reason: agent_client_protocol::schema::v1::StopReason,
    ) -> Vec<AgentUiEvent> {
        self.translator
            .lock()
            .expect("translator mutex poisoned")
            .finish_run(stop_reason)
    }

    fn fail_run(&self, code: AcpErrorCode, message: &str) -> Vec<AgentUiEvent> {
        self.translator
            .lock()
            .expect("translator mutex poisoned")
            .fail_run(code, message)
    }

    /// Answer every parked permission waiter with `cancelled`, as required by
    /// the ACP spec when a turn is cancelled or the session ends.
    fn cancel_pending_permissions(&self) {
        let pending = std::mem::take(
            &mut *self
                .pending_permissions
                .lock()
                .expect("permission registry mutex poisoned"),
        );
        for (_, tx) in pending {
            let _ = tx.send(RequestPermissionOutcome::Cancelled);
        }
    }

    /// Mark the session ended, close the event stream, and release parked
    /// permission waiters.
    fn finish_connection(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cancel_pending_permissions();
        self.event_tx
            .lock()
            .expect("event sender mutex poisoned")
            .take();
    }
}

/// Requests handed to `run_session_main` once the transport is up.
struct SessionLaunch {
    initialize: InitializeRequest,
    new_session: NewSessionRequest,
}

impl SessionLaunch {
    fn new(config: AgentSessionConfig) -> Self {
        // Advertise no client capabilities: MangoDisk does not let the agent
        // touch the filesystem or terminals directly (disk access goes
        // through the MangoDisk MCP server instead). Requests that arrive
        // anyway are explicitly declined by handler, never silently served.
        let initialize = InitializeRequest::new(ProtocolVersion::V1)
            .client_info(
                Implementation::new("mangodisk", env!("CARGO_PKG_VERSION")).title("MangoDisk"),
            )
            .client_capabilities(ClientCapabilities::new());

        let mcp_servers = config
            .mcp_servers
            .into_iter()
            .map(|server| {
                McpServer::Stdio(
                    McpServerStdio::new(server.name, server.command)
                        .args(server.args)
                        .env(
                            server
                                .env
                                .into_iter()
                                .map(|(name, value)| EnvVariable::new(name, value))
                                .collect(),
                        ),
                )
            })
            .collect();

        Self {
            initialize,
            new_session: NewSessionRequest::new(config.cwd).mcp_servers(mcp_servers),
        }
    }
}

struct HandshakeOk {
    connection: ConnectionTo<Agent>,
    session_id: SessionId,
}

/// Owns the provider process for the session's lifetime: builds the client,
/// runs the ACP connection until shutdown, and maps the terminal outcome.
async fn run_connection(
    provider_id: String,
    agent: AcpAgent,
    launch: SessionLaunch,
    shared: Arc<SessionShared>,
    handshake_tx: oneshot::Sender<AcpResult<HandshakeOk>>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), AcpError> {
    let notification_shared = shared.clone();
    let permission_shared = shared.clone();

    let result = agent_client_protocol::Client
        .builder()
        .name("mangodisk-acp")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let events = notification_shared
                    .translator
                    .lock()
                    .expect("translator mutex poisoned")
                    .translate_update(&notification.update);
                notification_shared.emit_all(events);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, cx| {
                handle_permission_request(&permission_shared, request, responder, cx)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |_request: ReadTextFileRequest, responder, _cx| {
                decline_capability("fs/read_text_file", responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |_request: WriteTextFileRequest, responder, _cx| {
                decline_capability("fs/write_text_file", responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx| {
            run_session_main(cx, launch, handshake_tx, shutdown_rx).await
        })
        .await;

    match &result {
        Ok(()) => log::debug!("ACP connection closed cleanly: provider={provider_id}"),
        Err(_) => log::debug!("ACP connection closed with an error: provider={provider_id}"),
    }

    // If a run is still open at this point the provider died mid-turn; close
    // it out so the UI never waits forever.
    let (code, message) = match &result {
        Ok(()) => (
            AcpErrorCode::SessionLost,
            "session closed while a run was active".to_string(),
        ),
        Err(error) => map_connection_error(error),
    };
    let events = shared.fail_run(code, &message);
    shared.emit_all(events);

    shared.finish_connection();
    result.map_err(|error| {
        let (code, message) = map_connection_error(&error);
        AcpError::new(code, message)
    })
}

/// Handshake (`initialize` + `session/new`), then park until shutdown or
/// provider exit. Runs as the connection's foreground future, so blocking on
/// responses here is safe by design.
async fn run_session_main(
    cx: ConnectionTo<Agent>,
    launch: SessionLaunch,
    handshake_tx: oneshot::Sender<AcpResult<HandshakeOk>>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), agent_client_protocol::Error> {
    let handshake = async {
        let initialize_response = cx
            .send_request(launch.initialize)
            .block_task()
            .await
            .map_err(|error| map_handshake_error("initialize", &error))?;
        let new_session_response = cx
            .send_request(launch.new_session)
            .block_task()
            .await
            .map_err(|error| map_handshake_error("session/new", &error))?;
        Ok::<_, AcpError>((initialize_response, new_session_response))
    }
    .await;

    match handshake {
        Ok((_initialize, new_session)) => {
            let ok = HandshakeOk {
                connection: cx.clone(),
                session_id: new_session.session_id,
            };
            if handshake_tx.send(Ok(ok)).is_err() {
                // The caller stopped waiting (handshake timeout fired); end
                // the session instead of leaking a process nobody holds.
                return Ok(());
            }
        }
        Err(error) => {
            let _ = handshake_tx.send(Err(error));
            return Err(agent_client_protocol::Error::internal_error()
                .data("mangodisk-acp: session handshake failed"));
        }
    }

    tokio::select! {
        _ = shutdown_rx => {}
        _ = cx.incoming_closed() => {}
    }
    Ok(())
}

/// Permission routing: park the ACP response behind a oneshot, surface the
/// request as a UI event, and answer from a connection-scoped task so the
/// dispatch loop is never blocked while the user decides.
fn handle_permission_request(
    shared: &Arc<SessionShared>,
    request: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    cx: ConnectionTo<Agent>,
) -> Result<
    Handled<(
        RequestPermissionRequest,
        Responder<RequestPermissionResponse>,
    )>,
    agent_client_protocol::Error,
> {
    let request_id = shared.next_permission_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = oneshot::channel();
    shared
        .pending_permissions
        .lock()
        .expect("permission registry mutex poisoned")
        .insert(request_id, tx);

    shared.emit(AgentUiEvent::PermissionRequested {
        request_id,
        tool_call_id: request.tool_call.tool_call_id.0.to_string(),
        title: request.tool_call.fields.title.clone(),
        options: request
            .options
            .iter()
            .map(|option| AgentPermissionOption {
                option_id: option.option_id.0.to_string(),
                label: option.name.clone(),
                kind: map_permission_option_kind(option.kind),
            })
            .collect(),
    });

    let parked_shared = shared.clone();
    let timeout = shared.permission_timeout;
    let spawn_result = cx.spawn(async move {
        let outcome = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                // The resolver was dropped (session closing); decline.
                RequestPermissionOutcome::Cancelled
            }
            Err(_) => {
                log::info!("permission request {request_id} auto-declined after {timeout:?}");
                RequestPermissionOutcome::Cancelled
            }
        };
        parked_shared
            .pending_permissions
            .lock()
            .expect("permission registry mutex poisoned")
            .remove(&request_id);
        responder.respond(RequestPermissionResponse::new(outcome))
    });

    if spawn_result.is_err() {
        // The connection is already winding down; the unspawned task was
        // dropped with its responder, so the agent observes the closing
        // connection rather than an answer — equivalent to a decline.
        shared
            .pending_permissions
            .lock()
            .expect("permission registry mutex poisoned")
            .remove(&request_id);
    }
    Ok(Handled::Yes)
}

/// Decline a client-capability request we never advertised support for.
///
/// MangoDisk agents reach the disk only through the MangoDisk MCP server, so
/// direct fs/terminal access from the agent is answered with a stable typed
/// error instead of being silently served (the SDK's fallback would reply
/// `method_not_found` without the capability marker).
fn decline_capability<T: JsonRpcResponse>(
    capability: &'static str,
    responder: Responder<T>,
) -> Result<(), agent_client_protocol::Error> {
    log::warn!("declined unsupported agent capability request: {capability}");
    responder.respond_with_error(agent_client_protocol::Error::method_not_found().data(
        serde_json::json!({
            "reason": "capability_not_supported",
            "capability": capability,
        }),
    ))
}

fn map_permission_option_kind(
    kind: agent_client_protocol::schema::v1::PermissionOptionKind,
) -> PermissionOptionKind {
    use agent_client_protocol::schema::v1::PermissionOptionKind as AcpKind;
    match kind {
        AcpKind::AllowOnce => PermissionOptionKind::AllowOnce,
        AcpKind::AllowAlways => PermissionOptionKind::AllowAlways,
        AcpKind::RejectOnce => PermissionOptionKind::RejectOnce,
        AcpKind::RejectAlways => PermissionOptionKind::RejectAlways,
        _ => PermissionOptionKind::Other,
    }
}

/// Short agent-authored message text, privacy-bounded. ACP error messages are
/// one-line summaries by spec; the cap protects against agents that attach
/// bulk content anyway.
fn remote_message(error: &agent_client_protocol::Error) -> String {
    error.message.chars().take(MAX_REMOTE_ERROR_LEN).collect()
}

/// Coarse classification of an SDK error, based on typed markers and fixed
/// message prefixes found anywhere in its (possibly nested) `data`.
///
/// Privacy note: SDK errors can embed a provider stderr tail and the SDK's
/// own `spawned_at` source path in `data`. Classification only pattern-matches
/// fixed strings and never copies `data` into our errors or logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionErrorKind {
    /// The provider process or its transport went away.
    ProviderGone,
    /// Spawning the provider process failed at the OS level.
    SpawnFailed,
    /// Anything else (protocol violations, handler errors, agent errors).
    Other,
}

fn classify_connection_error(error: &agent_client_protocol::Error) -> ConnectionErrorKind {
    if agent_client_protocol::is_incoming_transport_closed(error) {
        return ConnectionErrorKind::ProviderGone;
    }
    let Some(data) = &error.data else {
        return ConnectionErrorKind::Other;
    };
    if data_strings_match(data, &|text| {
        text.starts_with("Process exited") || text.contains("never received")
    }) {
        return ConnectionErrorKind::ProviderGone;
    }
    if data_strings_match(data, &|text| text.contains("(os error")) {
        return ConnectionErrorKind::SpawnFailed;
    }
    ConnectionErrorKind::Other
}

/// Bounded recursive scan of a JSON value's string leaves. The bound keeps a
/// pathological error payload from burning CPU; ACP error data is small in
/// practice.
fn data_strings_match(value: &serde_json::Value, predicate: &impl Fn(&str) -> bool) -> bool {
    const MAX_DEPTH: u8 = 4;
    fn walk(value: &serde_json::Value, depth: u8, predicate: &dyn Fn(&str) -> bool) -> bool {
        if depth == 0 {
            return false;
        }
        match value {
            serde_json::Value::String(text) => predicate(text),
            serde_json::Value::Array(items) => {
                items.iter().any(|item| walk(item, depth - 1, predicate))
            }
            serde_json::Value::Object(map) => map
                .values()
                .any(|nested| walk(nested, depth - 1, predicate)),
            _ => false,
        }
    }
    walk(value, MAX_DEPTH, predicate)
}

/// Whether a prompt-response failure means the transport died (as opposed to
/// the agent answering with a JSON-RPC error). The SDK fails pending requests
/// either with the typed incoming-transport-closed marker or with a
/// "response ... never received" internal error once the connection is gone.
fn is_transport_level_failure(error: &agent_client_protocol::Error) -> bool {
    agent_client_protocol::is_incoming_transport_closed(error)
        || error
            .data
            .as_ref()
            .is_some_and(|data| data_strings_match(data, &|text| text.contains("never received")))
}

/// Map an SDK error observed on the overall connection (post-handshake).
fn map_connection_error(error: &agent_client_protocol::Error) -> (AcpErrorCode, String) {
    match classify_connection_error(error) {
        ConnectionErrorKind::ProviderGone => (
            AcpErrorCode::ProviderExited,
            "provider process exited unexpectedly".to_string(),
        ),
        ConnectionErrorKind::SpawnFailed => (
            AcpErrorCode::SpawnFailed,
            "provider process could not be spawned".to_string(),
        ),
        ConnectionErrorKind::Other => (
            AcpErrorCode::SessionLost,
            "provider connection ended with an error".to_string(),
        ),
    }
}

/// Map an SDK error observed while the handshake requests were in flight.
fn map_handshake_error(phase: &str, error: &agent_client_protocol::Error) -> AcpError {
    match classify_connection_error(error) {
        ConnectionErrorKind::SpawnFailed => {
            AcpError::spawn_failed("provider process could not be spawned")
        }
        ConnectionErrorKind::ProviderGone => AcpError::provider_exited(format!(
            "provider process exited during the ACP {phase} exchange"
        )),
        ConnectionErrorKind::Other => AcpError::handshake_failed(format!(
            "ACP {phase} exchange failed: {}",
            remote_message(error)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_error_classification() {
        let spawn = agent_client_protocol::Error::internal_error()
            .data(serde_json::json!({"data": "Permission denied (os error 13)"}));
        assert_eq!(
            classify_connection_error(&spawn),
            ConnectionErrorKind::SpawnFailed
        );

        let exited = agent_client_protocol::Error::internal_error()
            .data(serde_json::json!({"data": "Process exited with exit status: 3"}));
        assert_eq!(
            classify_connection_error(&exited),
            ConnectionErrorKind::ProviderGone
        );

        let other = agent_client_protocol::Error::internal_error()
            .data("some agent failure without a known marker");
        assert_eq!(
            classify_connection_error(&other),
            ConnectionErrorKind::Other
        );
    }

    #[test]
    fn transport_level_prompt_failure_detection() {
        let canceled = agent_client_protocol::Error::internal_error()
            .data("response to `session/prompt` never received: oneshot canceled");
        assert!(is_transport_level_failure(&canceled));

        let agent_error = agent_client_protocol::Error::invalid_params().data("bad prompt");
        assert!(!is_transport_level_failure(&agent_error));
    }
}
