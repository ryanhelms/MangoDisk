//! Stateful translation from ACP session updates to UI events.
//!
//! Kept pure and synchronous (no I/O, no clock) so ordering and synthesis
//! rules are unit-testable without a live agent. The translator owns the
//! per-session stream state ACP leaves implicit: which text/thinking message
//! is open and which run is active.

use agent_client_protocol::schema::v1::{
    self, ContentBlock, SessionUpdate, ToolCallStatus as AcpToolCallStatus,
};

use crate::event::{
    AgentPlanEntry, AgentUiEvent, PlanEntryPriority, PlanEntryStatus, RunStopReason, ToolCallKind,
    ToolCallStatus,
};
use crate::AcpErrorCode;

/// Cap on agent-provided error text forwarded into `RunError` events. Bounds
/// UI payloads and strips any bulk content a misbehaving agent attaches.
const MAX_RUN_ERROR_MESSAGE_LEN: usize = 500;

#[derive(Debug, Default)]
pub(crate) struct EventTranslator {
    next_run_seq: u64,
    next_message_seq: u64,
    active_run: Option<String>,
    open_text_message: Option<String>,
    open_thinking_message: Option<String>,
}

impl EventTranslator {
    /// Begin a prompt run. If a previous run never reported completion (for
    /// example the agent lost the response), it is closed with an error so
    /// the UI never sees two overlapping runs. Returns the new run id.
    pub(crate) fn begin_run(&mut self) -> (String, Vec<AgentUiEvent>) {
        let mut events = Vec::new();
        if let Some(previous) = self.active_run.take() {
            events.extend(self.close_open_messages());
            events.push(AgentUiEvent::RunError {
                run_id: Some(previous),
                code: AcpErrorCode::SessionLost,
                message: "run was superseded by a new prompt before completing".to_string(),
            });
        }

        self.next_run_seq += 1;
        let run_id = format!("run-{}", self.next_run_seq);
        self.active_run = Some(run_id.clone());
        events.push(AgentUiEvent::RunStarted {
            run_id: run_id.clone(),
        });
        (run_id, events)
    }

    /// Close the active run successfully.
    pub(crate) fn finish_run(&mut self, stop_reason: v1::StopReason) -> Vec<AgentUiEvent> {
        let Some(run_id) = self.active_run.take() else {
            // A response without a tracked run cannot be ordered against
            // anything; dropping it keeps the stream consistent.
            return Vec::new();
        };
        let mut events = self.close_open_messages();
        events.push(AgentUiEvent::RunFinished {
            run_id,
            stop_reason: map_stop_reason(stop_reason),
        });
        events
    }

    /// Close the active run with an error.
    pub(crate) fn fail_run(&mut self, code: AcpErrorCode, message: &str) -> Vec<AgentUiEvent> {
        let Some(run_id) = self.active_run.take() else {
            return Vec::new();
        };
        let mut events = self.close_open_messages();
        events.push(AgentUiEvent::RunError {
            run_id: Some(run_id),
            code,
            message: message.chars().take(MAX_RUN_ERROR_MESSAGE_LEN).collect(),
        });
        events
    }

    /// Translate one ACP session update. Unknown or UI-irrelevant update kinds
    /// (mode changes, usage, available commands, user-echo chunks) translate
    /// to nothing by design.
    pub(crate) fn translate_update(&mut self, update: &SessionUpdate) -> Vec<AgentUiEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.translate_chunk(&chunk.content, chunk.message_id.as_ref(), StreamKind::Text)
            }
            SessionUpdate::AgentThoughtChunk(chunk) => self.translate_chunk(
                &chunk.content,
                chunk.message_id.as_ref(),
                StreamKind::Thinking,
            ),
            SessionUpdate::ToolCall(tool_call) => self.translate_tool_call(tool_call),
            SessionUpdate::ToolCallUpdate(update) => self.translate_tool_call_update(update),
            SessionUpdate::Plan(plan) => vec![AgentUiEvent::PlanUpdated {
                entries: plan.entries.iter().map(map_plan_entry).collect(),
            }],
            _ => Vec::new(),
        }
    }

    fn translate_chunk(
        &mut self,
        content: &ContentBlock,
        message_id: Option<&v1::MessageId>,
        kind: StreamKind,
    ) -> Vec<AgentUiEvent> {
        // Only text renders in the chat stream. Non-text blocks (images,
        // resources) are skipped here rather than approximated.
        let ContentBlock::Text(text) = content else {
            return Vec::new();
        };

        let open = match kind {
            StreamKind::Text => self.open_text_message.as_ref(),
            StreamKind::Thinking => self.open_thinking_message.as_ref(),
        };
        let id = match message_id {
            Some(id) => id.0.to_string(),
            None => match open {
                Some(open) => open.clone(),
                None => self.synthesize_message_id(),
            },
        };

        let open_slot = match kind {
            StreamKind::Text => &mut self.open_text_message,
            StreamKind::Thinking => &mut self.open_thinking_message,
        };

        let mut events = Vec::new();
        if open_slot.as_deref() != Some(id.as_str()) {
            if let Some(previous) = open_slot.take() {
                events.push(end_message_event(kind, previous));
            }
            events.push(start_message_event(kind, id.clone()));
            *open_slot = Some(id.clone());
        }
        events.push(content_message_event(kind, id, text.text.clone()));
        events
    }

    fn translate_tool_call(&mut self, tool_call: &v1::ToolCall) -> Vec<AgentUiEvent> {
        let tool_call_id = tool_call.tool_call_id.0.to_string();
        let status = map_tool_call_status(tool_call.status);
        let mut events = vec![AgentUiEvent::ToolCallStart {
            tool_call_id: tool_call_id.clone(),
            name: tool_call.title.clone(),
            kind: map_tool_call_kind(tool_call.kind),
            status,
        }];
        if let Some(raw_input) = &tool_call.raw_input {
            events.push(AgentUiEvent::ToolCallArgs {
                tool_call_id: tool_call_id.clone(),
                args: raw_input.clone(),
            });
        }
        if is_terminal_status(status) {
            events.push(AgentUiEvent::ToolCallEnd {
                tool_call_id,
                status,
            });
        }
        events
    }

    fn translate_tool_call_update(&mut self, update: &v1::ToolCallUpdate) -> Vec<AgentUiEvent> {
        let tool_call_id = update.tool_call_id.0.to_string();
        let mut events = Vec::new();
        if let Some(raw_input) = &update.fields.raw_input {
            events.push(AgentUiEvent::ToolCallArgs {
                tool_call_id: tool_call_id.clone(),
                args: raw_input.clone(),
            });
        }
        if let Some(status) = update.fields.status.map(map_tool_call_status) {
            if is_terminal_status(status) {
                events.push(AgentUiEvent::ToolCallEnd {
                    tool_call_id,
                    status,
                });
            }
        }
        events
    }

    fn synthesize_message_id(&mut self) -> String {
        self.next_message_seq += 1;
        format!("msg-{}", self.next_message_seq)
    }

    fn close_open_messages(&mut self) -> Vec<AgentUiEvent> {
        let mut events = Vec::new();
        if let Some(open) = self.open_thinking_message.take() {
            events.push(AgentUiEvent::ThinkingMessageEnd { message_id: open });
        }
        if let Some(open) = self.open_text_message.take() {
            events.push(AgentUiEvent::TextMessageEnd { message_id: open });
        }
        events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Text,
    Thinking,
}

fn start_message_event(kind: StreamKind, message_id: String) -> AgentUiEvent {
    match kind {
        StreamKind::Text => AgentUiEvent::TextMessageStart { message_id },
        StreamKind::Thinking => AgentUiEvent::ThinkingMessageStart { message_id },
    }
}

fn content_message_event(kind: StreamKind, message_id: String, delta: String) -> AgentUiEvent {
    match kind {
        StreamKind::Text => AgentUiEvent::TextMessageContent { message_id, delta },
        StreamKind::Thinking => AgentUiEvent::ThinkingMessageContent { message_id, delta },
    }
}

fn end_message_event(kind: StreamKind, message_id: String) -> AgentUiEvent {
    match kind {
        StreamKind::Text => AgentUiEvent::TextMessageEnd { message_id },
        StreamKind::Thinking => AgentUiEvent::ThinkingMessageEnd { message_id },
    }
}

fn is_terminal_status(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

fn map_stop_reason(stop_reason: v1::StopReason) -> RunStopReason {
    match stop_reason {
        v1::StopReason::EndTurn => RunStopReason::EndTurn,
        v1::StopReason::MaxTokens => RunStopReason::MaxTokens,
        v1::StopReason::MaxTurnRequests => RunStopReason::MaxTurnRequests,
        v1::StopReason::Refusal => RunStopReason::Refusal,
        v1::StopReason::Cancelled => RunStopReason::Cancelled,
        _ => RunStopReason::Other,
    }
}

fn map_tool_call_kind(kind: v1::ToolKind) -> ToolCallKind {
    match kind {
        v1::ToolKind::Read => ToolCallKind::Read,
        v1::ToolKind::Edit => ToolCallKind::Edit,
        v1::ToolKind::Delete => ToolCallKind::Delete,
        v1::ToolKind::Move => ToolCallKind::Move,
        v1::ToolKind::Search => ToolCallKind::Search,
        v1::ToolKind::Execute => ToolCallKind::Execute,
        v1::ToolKind::Think => ToolCallKind::Think,
        v1::ToolKind::Fetch => ToolCallKind::Fetch,
        v1::ToolKind::SwitchMode => ToolCallKind::SwitchMode,
        _ => ToolCallKind::Other,
    }
}

fn map_tool_call_status(status: AcpToolCallStatus) -> ToolCallStatus {
    match status {
        AcpToolCallStatus::Pending => ToolCallStatus::Pending,
        AcpToolCallStatus::InProgress => ToolCallStatus::InProgress,
        AcpToolCallStatus::Completed => ToolCallStatus::Completed,
        AcpToolCallStatus::Failed => ToolCallStatus::Failed,
        _ => ToolCallStatus::Other,
    }
}

fn map_plan_entry(entry: &v1::PlanEntry) -> AgentPlanEntry {
    AgentPlanEntry {
        content: entry.content.clone(),
        priority: match entry.priority {
            v1::PlanEntryPriority::High => PlanEntryPriority::High,
            v1::PlanEntryPriority::Medium => PlanEntryPriority::Medium,
            v1::PlanEntryPriority::Low => PlanEntryPriority::Low,
            _ => PlanEntryPriority::Other,
        },
        status: match entry.status {
            v1::PlanEntryStatus::Pending => PlanEntryStatus::Pending,
            v1::PlanEntryStatus::InProgress => PlanEntryStatus::InProgress,
            v1::PlanEntryStatus::Completed => PlanEntryStatus::Completed,
            _ => PlanEntryStatus::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ContentChunk, Plan, PlanEntry, TextContent, ToolCall, ToolCallUpdate, ToolCallUpdateFields,
    };

    fn text_chunk(text: &str, message_id: Option<&str>) -> SessionUpdate {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text)));
        let chunk = match message_id {
            Some(id) => chunk.message_id(id),
            None => chunk,
        };
        SessionUpdate::AgentMessageChunk(chunk)
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

    #[test]
    fn text_chunks_open_continue_and_close_messages() {
        let mut translator = EventTranslator::default();
        translator.begin_run();

        let first = translator.translate_update(&text_chunk("Hello ", None));
        let second = translator.translate_update(&text_chunk("world", None));
        assert_eq!(event_types(&first), ["text_start", "text_content"]);
        assert_eq!(event_types(&second), ["text_content"]);

        // A changed message id closes the previous message first.
        let third = translator.translate_update(&text_chunk("next", Some("m2")));
        assert_eq!(
            event_types(&third),
            ["text_end", "text_start", "text_content"]
        );

        // Run completion closes the open message before RunFinished.
        let end = translator.finish_run(v1::StopReason::EndTurn);
        assert_eq!(event_types(&end), ["text_end", "run_finished"]);
    }

    #[test]
    fn thought_chunks_use_the_thinking_stream() {
        let mut translator = EventTranslator::default();
        translator.begin_run();
        let chunk = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("thinking..."),
        )));
        let events = translator.translate_update(&chunk);
        assert_eq!(event_types(&events), ["thinking_start", "thinking_content"]);
        let end = translator.finish_run(v1::StopReason::EndTurn);
        assert_eq!(event_types(&end), ["thinking_end", "run_finished"]);
    }

    #[test]
    fn tool_call_maps_start_args_and_terminal_end() {
        let mut translator = EventTranslator::default();

        let call = ToolCall::new("tc-1", "Read file")
            .kind(v1::ToolKind::Read)
            .status(v1::ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({"path": "report.txt"}));
        let events = translator.translate_update(&SessionUpdate::ToolCall(call));
        assert_eq!(event_types(&events), ["tool_call_start", "tool_call_args"]);

        let update = ToolCallUpdate::new(
            "tc-1",
            ToolCallUpdateFields::new().status(v1::ToolCallStatus::Completed),
        );
        let events = translator.translate_update(&SessionUpdate::ToolCallUpdate(update));
        assert_eq!(event_types(&events), ["tool_call_end"]);
    }

    #[test]
    fn non_terminal_tool_update_without_args_is_quiet() {
        let mut translator = EventTranslator::default();
        let update = ToolCallUpdate::new(
            "tc-1",
            ToolCallUpdateFields::new().status(v1::ToolCallStatus::InProgress),
        );
        assert!(translator
            .translate_update(&SessionUpdate::ToolCallUpdate(update))
            .is_empty());
    }

    #[test]
    fn plan_updates_map_entries() {
        let mut translator = EventTranslator::default();
        let plan = Plan::new(vec![PlanEntry::new(
            "Scan disk",
            v1::PlanEntryPriority::High,
            v1::PlanEntryStatus::InProgress,
        )]);
        let events = translator.translate_update(&SessionUpdate::Plan(plan));
        let [AgentUiEvent::PlanUpdated { entries }] = events.as_slice() else {
            panic!("expected a single plan update");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Scan disk");
        assert_eq!(entries[0].priority, PlanEntryPriority::High);
        assert_eq!(entries[0].status, PlanEntryStatus::InProgress);
    }

    #[test]
    fn superseded_run_is_closed_with_error() {
        let mut translator = EventTranslator::default();
        let _ = translator.begin_run();
        let (_, events) = translator.begin_run();
        assert_eq!(event_types(&events), ["run_error", "run_started"]);
    }

    #[test]
    fn run_error_message_is_capped() {
        let mut translator = EventTranslator::default();
        translator.begin_run();
        let long = "x".repeat(MAX_RUN_ERROR_MESSAGE_LEN * 2);
        let events = translator.fail_run(AcpErrorCode::ProviderExited, &long);
        let [AgentUiEvent::RunError { message, .. }] = events.as_slice() else {
            panic!("expected a run error");
        };
        assert_eq!(message.chars().count(), MAX_RUN_ERROR_MESSAGE_LEN);
    }
}
