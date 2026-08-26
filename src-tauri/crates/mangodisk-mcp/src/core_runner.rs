use std::time::{Duration, Instant};

use mangodisk_core::{
    ApplicationUninstallExecutionProgress, CleanupExecutionProgress, CoreResult,
    OperationCancellationToken, ProgressSink, TraversalProgress,
};
use rmcp::{
    model::{CallToolResult, ProgressNotificationParam, ProgressToken},
    service::RequestContext,
    RoleServer,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::errors;

/// Minimum interval between execution progress notifications. Core throttles
/// its own item-level emissions, but stage boundaries arrive unthrottled; the
/// gate keeps bursts from flooding the MCP channel.
const EXECUTION_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// One guarded Core call issued by a tool. Owns the cancellation wiring and
/// the optional MCP progress channel so individual tools stay one-liners.
pub(crate) struct CoreOperation {
    cancellation: Option<OperationCancellationToken>,
    request_ct: CancellationToken,
    progress: Option<(rmcp::Peer<RoleServer>, ProgressToken)>,
}

/// One normalized progress observation crossing the MCP boundary.
///
/// Core execution snapshots carry filesystem paths (`current_item_path`) and
/// scans report the current path, neither of which may leave the process
/// through an unredacted channel. Every producer therefore maps to this shape
/// first: a numeric position, an optional total, and an optional stable
/// identifier (rule ID, application ID, or stage name) — nothing else.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgressEvent {
    value: f64,
    total: Option<f64>,
    message: Option<String>,
}

/// Hands the blocking Core closure progress adapters. Events reach the client
/// only when the client attached a `progressToken` to the call; otherwise
/// every adapter is a no-op.
pub(crate) struct ProgressForward {
    sender: Option<UnboundedSender<ProgressEvent>>,
}

impl ProgressForward {
    /// Scan progress: Core's `ProgressSink` contract, already throttled by the
    /// Core tracker. Counts and totals only, matching the original policy.
    pub(crate) fn sink(&self) -> impl ProgressSink {
        let sender = self.sender.clone();
        move |progress: TraversalProgress| {
            let Some(sender) = &sender else { return };
            let (value, total) = if progress.total_steps > 0 {
                (
                    progress.completed_steps as f64,
                    Some(progress.total_steps as f64),
                )
            } else {
                (progress.items_scanned as f64, None)
            };
            // Unbounded sends never block the disk workers; a dead receiver
            // means the response already completed and progress is moot.
            let _ = sender.send(ProgressEvent {
                value,
                total,
                message: None,
            });
        }
    }

    /// Cleanup execution progress: completed/total rules plus the current
    /// stable rule ID. The snapshot's `current_item_path` is deliberately
    /// never read here.
    pub(crate) fn cleanup_reporter(&self) -> impl FnMut(CleanupExecutionProgress) {
        let mut gate = ExecutionEventGate::new(self.sender.clone());
        move |snapshot: CleanupExecutionProgress| {
            let key = format!(
                "{:?}:{:?}:{}:{}:{}",
                snapshot.stage,
                snapshot.current_rule_id,
                snapshot.completed_rule_count,
                snapshot.affected_item_count,
                snapshot.released_bytes
            );
            gate.emit(
                key,
                ProgressEvent {
                    value: snapshot.completed_rule_count as f64,
                    total: (snapshot.total_rule_count > 0)
                        .then_some(snapshot.total_rule_count as f64),
                    message: snapshot
                        .current_rule_id
                        .clone()
                        .or_else(|| stage_label(&snapshot.stage)),
                },
            );
        }
    }

    /// Uninstall batch progress: completed/total applications plus the current
    /// stable application ID.
    pub(crate) fn uninstall_reporter(&self) -> impl FnMut(ApplicationUninstallExecutionProgress) {
        let mut gate = ExecutionEventGate::new(self.sender.clone());
        move |snapshot: ApplicationUninstallExecutionProgress| {
            let key = format!(
                "{:?}:{:?}:{}:{}:{}",
                snapshot.stage,
                snapshot.current_application_id,
                snapshot.completed_application_count,
                snapshot.affected_application_count,
                snapshot.released_bytes
            );
            gate.emit(
                key,
                ProgressEvent {
                    value: snapshot.completed_application_count as f64,
                    total: (snapshot.total_application_count > 0)
                        .then_some(snapshot.total_application_count as f64),
                    message: snapshot
                        .current_application_id
                        .clone()
                        .or_else(|| stage_label(&snapshot.stage)),
                },
            );
        }
    }
}

/// Serializes a stage enum through its stable wire name (camelCase, as used in
/// the desktop event protocol) so progress messages never carry free-form text.
fn stage_label(stage: &impl serde::Serialize) -> Option<String> {
    serde_json::to_value(stage)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
}

/// Per-callback dedup and throttle for execution snapshots. Identical states
/// never repeat, state changes are limited to one notification per interval,
/// and the terminal snapshot (value reaching total) always passes so clients
/// observe completion even after a fast final burst.
struct ExecutionEventGate {
    sender: Option<UnboundedSender<ProgressEvent>>,
    last_key: Option<String>,
    last_emit: Option<Instant>,
}

impl ExecutionEventGate {
    fn new(sender: Option<UnboundedSender<ProgressEvent>>) -> Self {
        Self {
            sender,
            last_key: None,
            last_emit: None,
        }
    }

    fn emit(&mut self, key: String, event: ProgressEvent) {
        let Some(sender) = &self.sender else { return };
        let finished = event
            .total
            .is_some_and(|total| total > 0.0 && event.value >= total);
        if !finished {
            if self.last_key.as_deref() == Some(key.as_str()) {
                return;
            }
            if self
                .last_emit
                .is_some_and(|last| last.elapsed() < EXECUTION_PROGRESS_INTERVAL)
            {
                return;
            }
        }
        if sender.send(event).is_ok() {
            self.last_key = Some(key);
            self.last_emit = Some(Instant::now());
        }
    }
}

impl CoreOperation {
    pub(crate) fn new(
        cancellation: Option<OperationCancellationToken>,
        context: &RequestContext<RoleServer>,
    ) -> Self {
        let progress = context
            .meta
            .get_progress_token()
            .map(|token| (context.peer.clone(), token));
        Self {
            cancellation,
            request_ct: context.ct.clone(),
            progress,
        }
    }

    /// Runs the blocking Core use case on a blocking worker and returns its
    /// result. Domain failures are translated to the stable tool error
    /// protocol here so no tool can leak a native diagnostic by accident.
    pub(crate) async fn run<T, F>(self, operation: &'static str, f: F) -> Result<T, CallToolResult>
    where
        T: Send + 'static,
        F: FnOnce(ProgressForward) -> CoreResult<T> + Send + 'static,
    {
        // When the client cancels the MCP request or the transport drops, rmcp
        // cancels this token; forwarding it cancels the Core operation at its
        // next checkpoint instead of letting an orphaned scan finish.
        let watcher = self.cancellation.map(|cancellation| {
            let request_ct = self.request_ct.clone();
            tokio::spawn(async move {
                request_ct.cancelled().await;
                cancellation.cancel();
            })
        });

        let forwarder = self.progress.map(|(peer, token)| {
            let (sender, receiver) = unbounded_channel();
            let task = tokio::spawn(forward_progress(peer, token, receiver));
            (sender, task)
        });
        let (sender, forwarder_task) = match forwarder {
            Some((sender, task)) => (Some(sender), Some(task)),
            None => (None, None),
        };

        let result = tokio::task::spawn_blocking(move || f(ProgressForward { sender })).await;

        if let Some(watcher) = watcher {
            watcher.abort();
        }
        if let Some(task) = forwarder_task {
            // The sender dropped with the Core closure, so the forwarder exits
            // once the channel drains. The timeout only guards against a
            // wedged peer write blocking the tool response.
            if tokio::time::timeout(std::time::Duration::from_millis(500), task)
                .await
                .is_err()
            {
                log::info!("mcp_progress_forward_timeout operation={operation}");
            }
        }

        let outcome = result.map_err(|error| {
            log::error!("mcp_tool_worker_join_failed operation={operation} error={error}");
            errors::tool_error(
                errors::TASK_JOIN_FAILED,
                "the operation worker terminated unexpectedly",
            )
        })?;
        outcome.map_err(|error| errors::core_failure(operation, error))
    }
}

/// Relays normalized progress events as MCP progress notifications. Only
/// counters, totals, and stable identifiers cross the wire; paths never reach
/// this channel because `ProgressEvent` has no field that can hold one.
async fn forward_progress(
    peer: rmcp::Peer<RoleServer>,
    token: ProgressToken,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<ProgressEvent>,
) {
    while let Some(event) = receiver.recv().await {
        let mut notification = ProgressNotificationParam::new(token.clone(), event.value);
        if let Some(total) = event.total {
            notification = notification.with_total(total);
        }
        if let Some(message) = event.message {
            notification = notification.with_message(message);
        }
        if peer.notify_progress(notification).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangodisk_core::{ApplicationUninstallExecutionStage, CleanupExecutionStage};

    fn forward() -> (
        ProgressForward,
        tokio::sync::mpsc::UnboundedReceiver<ProgressEvent>,
    ) {
        let (sender, receiver) = unbounded_channel();
        (
            ProgressForward {
                sender: Some(sender),
            },
            receiver,
        )
    }

    fn cleanup_snapshot(
        stage: CleanupExecutionStage,
        rule_id: Option<&str>,
        completed: u64,
        total: u64,
        affected: u64,
        released: u64,
    ) -> CleanupExecutionProgress {
        CleanupExecutionProgress {
            stage,
            planned_rule_ids: vec!["development.npm-cache".to_string()],
            current_rule_id: rule_id.map(str::to_owned),
            // A private path on purpose: the reporter must never forward it.
            current_item_path: Some("/home/user/private/secret.txt".to_string()),
            current_rule_affected_item_count: affected,
            current_rule_released_bytes: released,
            completed_rule_results: Vec::new(),
            validated_rule_count: completed,
            completed_rule_count: completed,
            total_rule_count: total,
            checked_item_count: affected,
            checked_bytes: released,
            affected_item_count: affected,
            released_bytes: released,
            elapsed_ms: 5,
        }
    }

    #[test]
    fn cleanup_progress_maps_stable_fields_and_never_paths() {
        let (forward, mut receiver) = forward();
        let mut reporter = forward.cleanup_reporter();

        reporter(cleanup_snapshot(
            CleanupExecutionStage::Cleaning,
            Some("development.npm-cache"),
            0,
            2,
            3,
            1024,
        ));

        let event = receiver.try_recv().expect("the event must be forwarded");
        assert_eq!(event.value, 0.0);
        assert_eq!(event.total, Some(2.0));
        assert_eq!(event.message.as_deref(), Some("development.npm-cache"));
        let serialized = format!("{event:?}");
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("/home/user"));
    }

    #[test]
    fn cleanup_progress_falls_back_to_the_stable_stage_name() {
        let (forward, mut receiver) = forward();
        let mut reporter = forward.cleanup_reporter();

        reporter(cleanup_snapshot(
            CleanupExecutionStage::Validating,
            None,
            0,
            1,
            0,
            0,
        ));

        let event = receiver.try_recv().expect("the event must be forwarded");
        assert_eq!(event.message.as_deref(), Some("validating"));
    }

    #[test]
    fn execution_progress_dedups_identical_states() {
        let (forward, mut receiver) = forward();
        let mut reporter = forward.cleanup_reporter();
        let snapshot = || {
            cleanup_snapshot(
                CleanupExecutionStage::Cleaning,
                Some("development.npm-cache"),
                0,
                1,
                1,
                64,
            )
        };

        reporter(snapshot());
        reporter(snapshot());

        assert!(receiver.try_recv().is_ok());
        assert!(
            receiver.try_recv().is_err(),
            "an identical snapshot must not repeat"
        );
    }

    #[test]
    fn execution_progress_throttles_bursts_but_passes_completion() {
        let (forward, mut receiver) = forward();
        let mut reporter = forward.cleanup_reporter();

        reporter(cleanup_snapshot(
            CleanupExecutionStage::Validating,
            Some("a.b"),
            0,
            2,
            0,
            0,
        ));
        // A different state inside the interval is throttled away.
        reporter(cleanup_snapshot(
            CleanupExecutionStage::Cleaning,
            Some("a.b"),
            0,
            2,
            1,
            64,
        ));
        // The terminal snapshot always passes, even inside the interval.
        reporter(cleanup_snapshot(
            CleanupExecutionStage::Finalizing,
            None,
            2,
            2,
            1,
            64,
        ));

        let first = receiver.try_recv().expect("the first state must pass");
        assert_eq!(first.message.as_deref(), Some("a.b"));
        let second = receiver
            .try_recv()
            .expect("the terminal snapshot must pass");
        assert_eq!(second.value, 2.0);
        assert_eq!(second.total, Some(2.0));
        assert_eq!(second.message.as_deref(), Some("finalizing"));
        assert!(
            receiver.try_recv().is_err(),
            "the throttled middle state must be dropped"
        );
    }

    #[test]
    fn uninstall_progress_maps_applications_without_paths() {
        let (forward, mut receiver) = forward();
        let mut reporter = forward.uninstall_reporter();

        reporter(ApplicationUninstallExecutionProgress {
            stage: ApplicationUninstallExecutionStage::Uninstalling,
            current_application_id: Some("com.example.app".to_string()),
            completed_applications: Vec::new(),
            completed_application_count: 1,
            total_application_count: 3,
            affected_application_count: 1,
            failed_application_count: 0,
            released_bytes: 2048,
            elapsed_ms: 5,
        });

        let event = receiver.try_recv().expect("the event must be forwarded");
        assert_eq!(event.value, 1.0);
        assert_eq!(event.total, Some(3.0));
        assert_eq!(event.message.as_deref(), Some("com.example.app"));
    }

    #[test]
    fn reporters_are_noop_without_a_client_progress_token() {
        let forward = ProgressForward { sender: None };
        let mut cleanup = forward.cleanup_reporter();
        let mut uninstall = forward.uninstall_reporter();

        // Must not panic or block when no client is listening.
        cleanup(cleanup_snapshot(
            CleanupExecutionStage::Cleaning,
            Some("a.b"),
            0,
            1,
            0,
            0,
        ));
        uninstall(ApplicationUninstallExecutionProgress {
            stage: ApplicationUninstallExecutionStage::Validating,
            current_application_id: None,
            completed_applications: Vec::new(),
            completed_application_count: 0,
            total_application_count: 1,
            affected_application_count: 0,
            failed_application_count: 0,
            released_bytes: 0,
            elapsed_ms: 0,
        });
    }
}
