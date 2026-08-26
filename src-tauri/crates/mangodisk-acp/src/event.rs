//! UI-facing event vocabulary, modeled on AG-UI.
//!
//! ACP `session/update` notifications are translated into [`AgentUiEvent`]
//! values (see `translate.rs`). Events cross the process boundary toward the
//! Tauri frontend, so every variant is serde-tagged with a SCREAMING_SNAKE_CASE
//! `type` field, mirroring AG-UI wire names, and the stream is versioned
//! explicitly through [`AgentUiEventEnvelope::schema_version`].
//!
//! Ordering guarantees:
//!
//! - Events for one session arrive on one channel in the order the agent
//!   produced them on the ACP wire.
//! - `RunStarted` is queued before the prompt request is sent, so it always
//!   precedes every event of that run.
//! - A run ends with exactly one of `RunFinished` or `RunError`; any still
//!   open message is closed with its `*End` event first.
//! - `PermissionRequested` can arrive at any point during a run; resolving it
//!   does not produce further events.

use serde::{Deserialize, Serialize};

use crate::AcpErrorCode;

/// Current schema version stamped onto every emitted envelope.
pub const AGENT_UI_EVENT_SCHEMA_VERSION: u32 = 1;

/// Wire envelope carrying one UI event plus ordering metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentUiEventEnvelope {
    /// Version of the event schema; currently [`AGENT_UI_EVENT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Monotonic per-session sequence number. Gaps in `seq` mean events were
    /// dropped between bridge and consumer, never reordered by the bridge.
    pub seq: u64,
    #[serde(flatten)]
    pub event: AgentUiEvent,
}

impl AgentUiEventEnvelope {
    pub fn new(seq: u64, event: AgentUiEvent) -> Self {
        Self {
            schema_version: AGENT_UI_EVENT_SCHEMA_VERSION,
            seq,
            event,
        }
    }
}

/// Events a chat UI needs to render one provider session.
///
/// Synthesized by the bridge (`Run*`) or translated from ACP session updates
/// (the rest). ACP has no explicit message boundaries, so `*Start`/`*End`
/// pairs are derived from chunk `messageId`s and run lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentUiEvent {
    /// A prompt run has been accepted and sent to the agent.
    RunStarted { run_id: String },
    /// The agent completed the run.
    RunFinished {
        run_id: String,
        stop_reason: RunStopReason,
    },
    /// The run failed (agent error) or the provider died mid-run.
    RunError {
        run_id: Option<String>,
        code: AcpErrorCode,
        /// Short, privacy-safe summary. Agent-provided error text is passed
        /// through capped; provider-process failures use fixed text.
        message: String,
    },
    /// A new agent text message started streaming.
    TextMessageStart { message_id: String },
    /// The next text delta of the message.
    TextMessageContent { message_id: String, delta: String },
    /// The message stream ended.
    TextMessageEnd { message_id: String },
    /// The agent started streaming reasoning ("thought") content.
    ThinkingMessageStart { message_id: String },
    /// The next reasoning delta.
    ThinkingMessageContent { message_id: String, delta: String },
    /// The reasoning stream ended.
    ThinkingMessageEnd { message_id: String },
    /// The agent initiated a tool call.
    ToolCallStart {
        tool_call_id: String,
        /// Human-readable title supplied by the agent (e.g. the tool name).
        name: String,
        kind: ToolCallKind,
        status: ToolCallStatus,
    },
    /// Input arguments for a tool call, delivered when the agent reports
    /// `rawInput`. ACP sends arguments atomically, so `args` is complete JSON,
    /// not a streaming delta.
    ToolCallArgs {
        tool_call_id: String,
        args: serde_json::Value,
    },
    /// A tool call reached a terminal status.
    ToolCallEnd {
        tool_call_id: String,
        status: ToolCallStatus,
    },
    /// The agent replaced its execution plan. Entries are the complete plan,
    /// not a delta.
    PlanUpdated { entries: Vec<AgentPlanEntry> },
    /// The agent asks the user to authorize a tool call. The bridge parks the
    /// ACP response until
    /// [`AgentSessionHandle::resolve_permission`](crate::AgentSessionHandle::resolve_permission)
    /// is called or the permission timeout auto-declines.
    PermissionRequested {
        request_id: u64,
        tool_call_id: String,
        title: Option<String>,
        options: Vec<AgentPermissionOption>,
    },
}

/// Why a run stopped; mirrors ACP `StopReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    /// The agent sent a stop reason this schema version does not know.
    #[serde(other)]
    Other,
}

/// Tool category hint for icon/treatment selection; mirrors ACP `ToolKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[serde(other)]
    Other,
}

/// Tool execution status; mirrors ACP `ToolCallStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Other,
}

/// One entry of an agent plan snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPlanEntry {
    pub content: String,
    pub priority: PlanEntryPriority,
    pub status: PlanEntryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryPriority {
    High,
    Medium,
    Low,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
    #[serde(other)]
    Other,
}

/// One selectable answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPermissionOption {
    pub option_id: String,
    pub label: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[serde(other)]
    Other,
}

/// The caller's answer to a pending permission request.
///
/// ACP has no distinct "deny" outcome: declining maps to the protocol's
/// `cancelled` outcome, while rejection-with-prejudice is expressed by
/// selecting one of the agent's `reject_*` options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum PermissionResolution {
    /// The user picked one of the offered options.
    Selected { option_id: String },
    /// The user dismissed or declined the request.
    Declined,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serializes_with_schema_version_and_type_tag() {
        let envelope = AgentUiEventEnvelope::new(
            7,
            AgentUiEvent::RunStarted {
                run_id: "run-1".to_string(),
            },
        );
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schema_version": 1,
                "seq": 7,
                "type": "RUN_STARTED",
                "run_id": "run-1",
            })
        );
    }

    #[test]
    fn envelope_round_trips_permission_event() {
        let envelope = AgentUiEventEnvelope::new(
            3,
            AgentUiEvent::PermissionRequested {
                request_id: 42,
                tool_call_id: "call-9".to_string(),
                title: Some("Delete files".to_string()),
                options: vec![AgentPermissionOption {
                    option_id: "allow".to_string(),
                    label: "Allow once".to_string(),
                    kind: PermissionOptionKind::AllowOnce,
                }],
            },
        );
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: AgentUiEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
        assert!(json.contains("\"PERMISSION_REQUESTED\""));
    }

    #[test]
    fn stop_reason_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunStopReason::EndTurn).unwrap(),
            "\"end_turn\""
        );
    }
}
