//! ACP bridge for MangoDisk's in-app AI chat.
//!
//! This crate is protocol-only: it probes, spawns, and manages locally
//! installed, already-authenticated AI provider CLIs (Claude Code, Codex,
//! Kimi, ...) over the Agent Client Protocol, and translates their session
//! output into UI-facing events modeled on the AG-UI vocabulary. No API keys
//! pass through here — authentication rides on each provider CLI's own login.
//!
//! Boundaries: no dependency on `mangodisk-core`, `mangodisk-platform`, or
//! Tauri; the only filesystem contact is spawning provider processes and
//! searching `PATH`. Disk tools reach the agent through MangoDisk's own MCP
//! server, registered per session via ACP `session/new` (see
//! [`AgentSessionConfig::with_mcp_server`]).
//!
//! Typical flow:
//!
//! 1. [`probe_available_providers`] lists which providers this machine has.
//! 2. [`AgentBridge::start_session`] spawns the provider and returns an
//!    [`AgentSessionHandle`].
//! 3. [`AgentSessionHandle::prompt`] streams [`AgentUiEventEnvelope`]s back
//!    through [`AgentSessionHandle::next_event`]; permission prompts are
//!    answered with [`AgentSessionHandle::resolve_permission`].
//! 4. [`AgentSessionHandle::close`] (or dropping the handle) reaps the
//!    provider process.

mod error;
mod event;
mod launch;
mod provider;
mod session;
mod translate;

pub use error::{AcpError, AcpErrorCode, AcpResult};
pub use event::{
    AgentPermissionOption, AgentPlanEntry, AgentUiEvent, AgentUiEventEnvelope,
    PermissionOptionKind, PermissionResolution, PlanEntryPriority, PlanEntryStatus, RunStopReason,
    ToolCallKind, ToolCallStatus, AGENT_UI_EVENT_SCHEMA_VERSION,
};
pub use launch::CommandSpec;
pub use provider::{
    default_providers, probe_available_providers, probe_providers, AgentProviderDescriptor,
    ProbeSpec, ProbedProvider, DEFAULT_PROBE_TIMEOUT,
};
pub use session::{
    AgentBridge, AgentSessionConfig, AgentSessionHandle, StdioMcpServerSpec,
    DEFAULT_HANDSHAKE_TIMEOUT, DEFAULT_PERMISSION_TIMEOUT,
};
