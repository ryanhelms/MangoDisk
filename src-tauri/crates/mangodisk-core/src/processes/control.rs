use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use mangodisk_platform::{
    current_platform, Platform, ProcessEndMode, ProcessEndStatus, ProcessMetricsSnapshot,
};
use serde::{Deserialize, Serialize};

use crate::{
    filesystem::metadata::now_ms,
    history::{
        HistoryService, OperationCategory, OperationDetails, OperationOutcome, OperationRecord,
        ProcessControlHistoryItem, ProcessControlHistoryItemStatus, ProcessControlOperationDetails,
        OPERATION_RECORD_SCHEMA_VERSION,
    },
    shared::{
        operation::{CoordinatedOperationKind, OperationGuard},
        CoreError, CoreResult,
    },
};

use super::{
    analysis::{associate_applications, classify_process, ProcessClassification},
    ProcessClassificationFacts,
};

pub const PROCESS_END_PLAN_SCHEMA_VERSION: u32 = 1;

/// Plans expire quickly because process ownership and PID identity are
/// perishable evidence; a stale plan must be re-prepared against a fresh
/// snapshot rather than executed blindly.
const END_PLAN_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_END_PIDS: usize = 256;
const GRACEFUL_WAIT_TIMEOUT: Duration = Duration::from_secs(3);
const FORCE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

static PENDING_END_PLAN: OnceLock<Mutex<Option<ProcessEndPlan>>> = OnceLock::new();

/// Why one planned process may not be ended. These codes cross the adapter
/// boundary and must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessEndRefusal {
    /// The process disappeared between the scan and the plan.
    ProcessNotFound,
    /// Another user's (or root's) process and MangoDisk holds no privilege to
    /// end it. No elevation helper exists by design.
    OwnedByOtherUser,
    /// Ownership could not be proven, so the guard fails closed.
    OwnershipUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessEndDecision {
    Allowed,
    Refused(ProcessEndRefusal),
}

/// One plan entry with the identity evidence needed to revalidate the process
/// at execution time (PID reuse protection) and the guard decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEndPlanItem {
    pub pid: u32,
    pub name: String,
    pub started_at_ms: u64,
    pub classification: ProcessClassification,
    pub decision: ProcessEndDecision,
}

/// A confirmed-ready kill plan. Execution accepts only the exact plan value
/// Core issued through `prepare_end`, still within its TTL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEndPlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub issued_at_ms: u64,
    pub items: Vec<ProcessEndPlanItem>,
}

/// Outcome of one process after the execution passes finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessEndItemStatus {
    Ended,
    EndedAfterForce,
    /// Already gone when execution started; the goal state holds.
    AlreadyExited,
    /// Still alive after every permitted pass. This is the final authority.
    StillRunning,
    PermissionDenied,
    /// The platform cannot end the process in the requested mode.
    Unsupported,
    /// The PID now belongs to a different process than the plan captured.
    IdentityChanged,
    /// The plan refused this process; nothing was attempted.
    Refused,
    /// The platform request failed for an untyped reason.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEndItemResult {
    pub pid: u32,
    pub name: String,
    pub status: ProcessEndItemStatus,
    pub refusal: Option<ProcessEndRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEndResult {
    pub plan_id: String,
    pub mode: ProcessEndMode,
    pub requested_count: u64,
    pub ended_count: u64,
    pub failed_count: u64,
    /// Processes still alive after verification, including PID reuse
    /// survivors. Adapters must treat this list as the final authority.
    pub remaining_pids: Vec<u32>,
    pub items: Vec<ProcessEndItemResult>,
    pub elapsed_ms: u64,
}

pub struct ProcessControlService;

impl ProcessControlService {
    /// Builds a kill plan for the requested PIDs.
    ///
    /// Hard errors (the whole request fails): the MangoDisk process itself,
    /// PID 0 or 1, and anything classified `CriticalSystem` — these are never
    /// plan items, not even as refusals. Per-item refusals cover processes
    /// owned by other users without privilege and unknown ownership.
    pub fn prepare_end(pids: Vec<u32>) -> CoreResult<ProcessEndPlan> {
        let pids = validate_requested_pids(&pids)?;
        let started = Instant::now();
        let snapshot = current_platform()
            .snapshot_processes()
            .map_err(CoreError::from)?;
        let snapshot_by_pid: HashMap<u32, &ProcessMetricsSnapshot> = snapshot
            .iter()
            .map(|process| (process.pid, process))
            .collect();
        let samples: Vec<super::ProcessSample> = snapshot
            .iter()
            .map(super::ProcessSample::from_platform)
            .collect();
        let associations = associate_applications(&samples);
        let associated: HashSet<u32> = associations
            .matches
            .iter()
            .filter(|entry| entry.application_identifier.is_some())
            .map(|entry| entry.pid)
            .collect();
        let privileged = current_process_privileged();

        let mut items = Vec::with_capacity(pids.len());
        for pid in pids {
            let Some(process) = snapshot_by_pid.get(&pid) else {
                items.push(ProcessEndPlanItem {
                    pid,
                    name: String::new(),
                    started_at_ms: 0,
                    classification: ProcessClassification::UserBackground,
                    decision: ProcessEndDecision::Refused(ProcessEndRefusal::ProcessNotFound),
                });
                continue;
            };
            let facts = ProcessClassificationFacts {
                pid: process.pid,
                owner_uid: process.owner_uid.value().copied(),
                owned_by_current_user: process.owned_by_current_user.value().copied(),
                executable_path_absence: process.executable_path.absence(),
                application_associated: associated.contains(&pid),
            };
            let classification = classify_process(&facts);
            if classification == ProcessClassification::CriticalSystem {
                log::warn!("process_end_plan_rejected pid={pid} reason=critical_system");
                return Err(CoreError::invalid_input(
                    "ending a critical system process is forbidden",
                ));
            }
            let decision = if privileged {
                ProcessEndDecision::Allowed
            } else {
                match facts.owned_by_current_user {
                    Some(true) => ProcessEndDecision::Allowed,
                    Some(false) => ProcessEndDecision::Refused(ProcessEndRefusal::OwnedByOtherUser),
                    None => ProcessEndDecision::Refused(ProcessEndRefusal::OwnershipUnknown),
                }
            };
            items.push(ProcessEndPlanItem {
                pid,
                name: process.name.clone(),
                started_at_ms: process.started_at_ms,
                classification,
                decision,
            });
        }

        let plan = ProcessEndPlan {
            schema_version: PROCESS_END_PLAN_SCHEMA_VERSION,
            plan_id: format!("process-end-{}-{}", now_ms(), std::process::id()),
            issued_at_ms: now_ms(),
            items,
        };
        let allowed = plan
            .items
            .iter()
            .filter(|item| item.decision == ProcessEndDecision::Allowed)
            .count();
        log::info!(
            "process_end_plan_issued plan_id={} requested_count={} allowed_count={} refused_count={} elapsed_ms={}",
            plan.plan_id,
            plan.items.len(),
            allowed,
            plan.items.len() - allowed,
            started.elapsed().as_millis()
        );
        store_pending_plan(&plan)?;
        Ok(plan)
    }

    /// Executes a previously issued plan after explicit caller confirmation.
    ///
    /// Execution passes: SIGTERM-style graceful requests for every allowed
    /// process, a bounded wait, then — only in `Force` mode — force requests
    /// for the processes still alive. A final verification snapshot reports
    /// the remaining PIDs, which are the final authority over every earlier
    /// status.
    pub fn execute_end(
        plan: ProcessEndPlan,
        mode: ProcessEndMode,
        confirmed: bool,
    ) -> CoreResult<ProcessEndResult> {
        if !confirmed {
            return Err(CoreError::invalid_input(
                "process end execution requires explicit confirmation",
            ));
        }
        if plan.schema_version != PROCESS_END_PLAN_SCHEMA_VERSION {
            return Err(CoreError::invalid_input(
                "the process end plan schema is unsupported",
            ));
        }
        validate_pending_plan(&plan)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::ProcessEnd)?;
        clear_pending_plan()?;
        let started = Instant::now();
        let started_at_ms = now_ms();

        let mut items: Vec<ProcessEndItemResult> = plan
            .items
            .iter()
            .map(|item| match item.decision {
                ProcessEndDecision::Allowed => ProcessEndItemResult {
                    pid: item.pid,
                    name: item.name.clone(),
                    status: ProcessEndItemStatus::StillRunning,
                    refusal: None,
                },
                ProcessEndDecision::Refused(refusal) => ProcessEndItemResult {
                    pid: item.pid,
                    name: item.name.clone(),
                    status: ProcessEndItemStatus::Refused,
                    refusal: Some(refusal),
                },
            })
            .collect();
        let candidates: HashMap<u32, u64> = plan
            .items
            .iter()
            .filter(|item| item.decision == ProcessEndDecision::Allowed)
            .map(|item| (item.pid, item.started_at_ms))
            .collect();

        // Revalidate identity before the first signal: a PID reused since the
        // plan was issued must never receive a termination request.
        let first_snapshot = current_platform()
            .snapshot_processes()
            .map_err(CoreError::from)?;
        let live = live_processes(&first_snapshot);
        let mut active: HashMap<u32, u64> = HashMap::new();
        for (pid, started_at) in &candidates {
            match live.get(pid) {
                None => set_item_status(&mut items, *pid, ProcessEndItemStatus::AlreadyExited),
                Some(current) if !same_process(*started_at, *current) => {
                    set_item_status(&mut items, *pid, ProcessEndItemStatus::IdentityChanged)
                }
                Some(_) => {
                    active.insert(*pid, *started_at);
                }
            }
        }

        request_end(&active, ProcessEndMode::Graceful, &mut items);
        let survivors = wait_for_exit(&active, GRACEFUL_WAIT_TIMEOUT);
        if mode == ProcessEndMode::Force && !survivors.is_empty() {
            let force_candidates: HashMap<u32, u64> =
                survivors.iter().map(|pid| (*pid, active[pid])).collect();
            request_end(&force_candidates, ProcessEndMode::Force, &mut items);
            wait_for_exit(&force_candidates, FORCE_WAIT_TIMEOUT);
        }

        // Verification pass: liveness is judged only from a fresh snapshot.
        let verification = current_platform()
            .snapshot_processes()
            .map_err(CoreError::from)?;
        let verification_live = live_processes(&verification);
        let mut remaining_pids = Vec::new();
        for (pid, started_at) in &active {
            if verification_live
                .get(pid)
                .is_some_and(|current| same_process(*started_at, *current))
            {
                remaining_pids.push(*pid);
                if !matches!(
                    item_status(&items, *pid),
                    Some(ProcessEndItemStatus::PermissionDenied)
                        | Some(ProcessEndItemStatus::Unsupported)
                        | Some(ProcessEndItemStatus::Failed)
                ) {
                    set_item_status(&mut items, *pid, ProcessEndItemStatus::StillRunning);
                }
                continue;
            }
            // The process is gone. `EndedAfterForce` stays only where a force
            // request was actually accepted; a process that died despite a
            // failed request keeps the honest request outcome instead of
            // claiming a kill Core never delivered.
            if !matches!(
                item_status(&items, *pid),
                Some(ProcessEndItemStatus::EndedAfterForce)
                    | Some(ProcessEndItemStatus::PermissionDenied)
                    | Some(ProcessEndItemStatus::Unsupported)
                    | Some(ProcessEndItemStatus::Failed)
            ) {
                set_item_status(&mut items, *pid, ProcessEndItemStatus::Ended);
            }
        }
        remaining_pids.sort_unstable();

        let ended_count = items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    ProcessEndItemStatus::Ended
                        | ProcessEndItemStatus::EndedAfterForce
                        | ProcessEndItemStatus::AlreadyExited
                )
            })
            .count() as u64;
        let result = ProcessEndResult {
            plan_id: plan.plan_id.clone(),
            mode,
            requested_count: items.len() as u64,
            ended_count,
            failed_count: items.len() as u64 - ended_count,
            remaining_pids,
            items,
            elapsed_ms: started.elapsed().as_millis() as u64,
        };
        log::info!(
            "process_end_finished plan_id={} mode={:?} requested_count={} ended_count={} failed_count={} remaining_count={} elapsed_ms={}",
            result.plan_id,
            mode,
            result.requested_count,
            result.ended_count,
            result.failed_count,
            result.remaining_pids.len(),
            result.elapsed_ms
        );
        record_history(&plan, mode, &result, started_at_ms);
        operation.complete();
        Ok(result)
    }
}

fn validate_requested_pids(pids: &[u32]) -> CoreResult<Vec<u32>> {
    if pids.is_empty() || pids.len() > MAX_END_PIDS {
        return Err(CoreError::invalid_input(
            "the process end pid count is invalid",
        ));
    }
    let mut seen = HashSet::with_capacity(pids.len());
    let mut unique = Vec::with_capacity(pids.len());
    for pid in pids {
        if *pid <= 1 {
            log::warn!("process_end_plan_rejected pid={pid} reason=system_pid");
            return Err(CoreError::invalid_input(
                "ending pid 0 or pid 1 is forbidden",
            ));
        }
        if *pid == std::process::id() {
            log::warn!("process_end_plan_rejected pid={pid} reason=self_pid");
            return Err(CoreError::invalid_input(
                "ending the MangoDisk process itself is forbidden",
            ));
        }
        if seen.insert(*pid) {
            unique.push(*pid);
        }
    }
    Ok(unique)
}

/// Only an unprivileged MangoDisk needs the other-user guard. Unix root can
/// end any non-critical process; on Windows and macOS without an elevation
/// helper MangoDisk stays conservative by design.
fn current_process_privileged() -> bool {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::geteuid() };
        uid == 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn store_pending_plan(plan: &ProcessEndPlan) -> CoreResult<()> {
    let mutex = PENDING_END_PLAN.get_or_init(|| Mutex::new(None));
    let mut pending = mutex
        .lock()
        .map_err(|_| CoreError::operation_failed("the process end plan store is unavailable"))?;
    *pending = Some(plan.clone());
    Ok(())
}

fn validate_pending_plan(plan: &ProcessEndPlan) -> CoreResult<()> {
    let mutex = PENDING_END_PLAN.get_or_init(|| Mutex::new(None));
    let pending = mutex
        .lock()
        .map_err(|_| CoreError::operation_failed("the process end plan store is unavailable"))?;
    if pending.as_ref() != Some(plan) {
        return Err(CoreError::invalid_input(
            "the process end plan was not prepared or was superseded",
        ));
    }
    if now_ms().saturating_sub(plan.issued_at_ms) > END_PLAN_TTL_MS {
        return Err(CoreError::invalid_input("the process end plan has expired"));
    }
    Ok(())
}

fn clear_pending_plan() -> CoreResult<()> {
    let mutex = PENDING_END_PLAN.get_or_init(|| Mutex::new(None));
    let mut pending = mutex
        .lock()
        .map_err(|_| CoreError::operation_failed("the process end plan store is unavailable"))?;
    *pending = None;
    Ok(())
}

fn live_processes(snapshot: &[ProcessMetricsSnapshot]) -> HashMap<u32, u64> {
    snapshot
        .iter()
        .map(|process| (process.pid, process.started_at_ms))
        .collect()
}

/// An unknown start time (0) cannot disprove identity, so it falls back to
/// plain PID matching; two known different start times prove PID reuse.
fn same_process(planned_started_at_ms: u64, current_started_at_ms: u64) -> bool {
    planned_started_at_ms == 0
        || current_started_at_ms == 0
        || planned_started_at_ms == current_started_at_ms
}

fn set_item_status(items: &mut [ProcessEndItemResult], pid: u32, status: ProcessEndItemStatus) {
    if let Some(item) = items.iter_mut().find(|item| item.pid == pid) {
        item.status = status;
    }
}

fn item_status(items: &[ProcessEndItemResult], pid: u32) -> Option<ProcessEndItemStatus> {
    items
        .iter()
        .find(|item| item.pid == pid)
        .map(|item| item.status)
}

fn request_end(
    candidates: &HashMap<u32, u64>,
    mode: ProcessEndMode,
    items: &mut [ProcessEndItemResult],
) {
    let platform = current_platform();
    for pid in candidates.keys() {
        match platform.end_process(*pid, mode) {
            Ok(ProcessEndStatus::Ended) => {
                if mode == ProcessEndMode::Force {
                    set_item_status(items, *pid, ProcessEndItemStatus::EndedAfterForce);
                }
            }
            Ok(ProcessEndStatus::NotFound) => {
                set_item_status(items, *pid, ProcessEndItemStatus::AlreadyExited)
            }
            Ok(ProcessEndStatus::PermissionDenied) => {
                set_item_status(items, *pid, ProcessEndItemStatus::PermissionDenied)
            }
            Ok(ProcessEndStatus::Unsupported) => {
                set_item_status(items, *pid, ProcessEndItemStatus::Unsupported)
            }
            Err(error) => {
                log::warn!(
                    "process_end_request_failed pid={} mode={:?} error_code={:?} error_digest={}",
                    pid,
                    mode,
                    error.code(),
                    blake3::hash(error.as_bytes()).to_hex()
                );
                set_item_status(items, *pid, ProcessEndItemStatus::Failed);
            }
        }
    }
}

/// Polls fresh snapshots until every candidate is gone or the deadline
/// passes. Snapshot failures fail closed: the candidates are reported as
/// survivors rather than presumed dead.
fn wait_for_exit(candidates: &HashMap<u32, u64>, timeout: Duration) -> Vec<u32> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        match current_platform().snapshot_processes() {
            Ok(snapshot) => {
                let live = live_processes(&snapshot);
                let survivors: Vec<u32> = candidates
                    .iter()
                    .filter(|(pid, started_at)| {
                        live.get(*pid)
                            .is_some_and(|current| same_process(**started_at, *current))
                    })
                    .map(|(pid, _)| *pid)
                    .collect();
                if survivors.is_empty() || Instant::now() >= deadline {
                    return survivors;
                }
            }
            Err(error) => {
                log::warn!(
                    "process_end_wait_snapshot_failed error_code={:?} error_digest={}",
                    error.code(),
                    blake3::hash(error.as_bytes()).to_hex()
                );
                return candidates.keys().copied().collect();
            }
        }
        std::thread::sleep(EXIT_POLL_INTERVAL);
    }
}

fn record_history(
    plan: &ProcessEndPlan,
    mode: ProcessEndMode,
    result: &ProcessEndResult,
    started_at_ms: u64,
) {
    let details = ProcessControlOperationDetails {
        plan_id: plan.plan_id.clone(),
        mode,
        requested_count: result.requested_count,
        ended_count: result.ended_count,
        failed_count: result.failed_count,
        items: result
            .items
            .iter()
            .map(|item| ProcessControlHistoryItem {
                pid: item.pid,
                name: item.name.clone(),
                status: match item.status {
                    ProcessEndItemStatus::Ended
                    | ProcessEndItemStatus::EndedAfterForce
                    | ProcessEndItemStatus::AlreadyExited => ProcessControlHistoryItemStatus::Ended,
                    ProcessEndItemStatus::StillRunning => {
                        ProcessControlHistoryItemStatus::StillRunning
                    }
                    ProcessEndItemStatus::Refused => ProcessControlHistoryItemStatus::Refused,
                    ProcessEndItemStatus::PermissionDenied
                    | ProcessEndItemStatus::Unsupported
                    | ProcessEndItemStatus::IdentityChanged
                    | ProcessEndItemStatus::Failed => ProcessControlHistoryItemStatus::Failed,
                },
            })
            .collect(),
    };
    let record = OperationRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        operation_id: plan.plan_id.clone(),
        category: OperationCategory::ProcessControl,
        started_at_ms,
        finished_at_ms: now_ms(),
        outcome: if result.failed_count == 0 {
            OperationOutcome::Completed
        } else {
            OperationOutcome::CompletedWithWarnings
        },
        dry_run: false,
        selected_item_count: result.requested_count,
        affected_item_count: result.ended_count,
        expected_bytes: 0,
        released_bytes: None,
        released_bytes_is_estimate: false,
        failed_item_count: result.failed_count,
        details: OperationDetails::ProcessControl(details),
    };
    if let Err(error) = HistoryService::append(record) {
        log::warn!(
            "process_end_history_save_failed plan_id={} error_digest={}",
            plan.plan_id,
            blake3::hash(error.diagnostic().as_bytes()).to_hex()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Serializes tests that prepare and execute plans, because Core
    /// intentionally accepts only the most recently issued plan.
    static PLAN_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn plan_rejects_self_pid() {
        assert!(ProcessControlService::prepare_end(vec![std::process::id()]).is_err());
    }

    #[test]
    fn plan_rejects_system_pids() {
        assert!(ProcessControlService::prepare_end(vec![0]).is_err());
        assert!(ProcessControlService::prepare_end(vec![1]).is_err());
    }

    #[test]
    fn plan_rejects_empty_and_oversized_requests() {
        assert!(ProcessControlService::prepare_end(Vec::new()).is_err());
        assert!(
            ProcessControlService::prepare_end(vec![u32::MAX; MAX_END_PIDS + 1]).is_err(),
            "requests above the pid bound must fail before any snapshot"
        );
    }

    #[test]
    fn execute_requires_confirmation() {
        let plan = ProcessEndPlan {
            schema_version: PROCESS_END_PLAN_SCHEMA_VERSION,
            plan_id: "process-end-test-unconfirmed".to_string(),
            issued_at_ms: now_ms(),
            items: Vec::new(),
        };
        assert!(ProcessControlService::execute_end(plan, ProcessEndMode::Graceful, false).is_err());
    }

    #[test]
    fn execute_rejects_an_unprepared_plan() {
        let plan = ProcessEndPlan {
            schema_version: PROCESS_END_PLAN_SCHEMA_VERSION,
            plan_id: "process-end-test-fabricated".to_string(),
            issued_at_ms: now_ms(),
            items: Vec::new(),
        };
        assert!(ProcessControlService::execute_end(plan, ProcessEndMode::Graceful, true).is_err());
    }

    #[test]
    fn execute_rejects_an_unknown_plan_schema() {
        let plan = ProcessEndPlan {
            schema_version: PROCESS_END_PLAN_SCHEMA_VERSION + 1,
            plan_id: "process-end-test-schema".to_string(),
            issued_at_ms: now_ms(),
            items: Vec::new(),
        };
        assert!(ProcessControlService::execute_end(plan, ProcessEndMode::Graceful, true).is_err());
    }

    /// Linux: kernel threads have no userspace image and must be refused as
    /// critical system processes with a hard error.
    #[cfg(target_os = "linux")]
    #[test]
    fn plan_hard_fails_on_critical_system_processes() {
        let snapshot = current_platform()
            .snapshot_processes()
            .expect("the live snapshot should succeed");
        let Some(kernel_thread) = snapshot.iter().find(|process| {
            process.pid > 1
                && process.executable_path.absence()
                    == Some(mangodisk_platform::ProcessMetricAbsence::NotApplicable)
        }) else {
            // A container without visible kernel threads cannot exercise this
            // rule; the synthetic classification matrix covers the logic.
            return;
        };
        let result = ProcessControlService::prepare_end(vec![kernel_thread.pid]);
        assert!(
            result.is_err(),
            "pid {} without a userspace image must be a hard plan error",
            kernel_thread.pid
        );
    }

    /// Unix: a process owned by another user must be a typed per-item refusal
    /// when MangoDisk holds no privilege, never an allowed plan item.
    #[cfg(unix)]
    #[test]
    fn plan_refuses_other_user_processes_without_privilege() {
        let _plan_guard = PLAN_TEST_LOCK
            .lock()
            .expect("the plan test lock should be free");
        if current_process_privileged() {
            return;
        }
        let snapshot = current_platform()
            .snapshot_processes()
            .expect("the live snapshot should succeed");
        let Some(foreign) = snapshot.iter().find(|process| {
            process.pid > 1 && process.owned_by_current_user.value() == Some(&false)
        }) else {
            return;
        };
        let plan = ProcessControlService::prepare_end(vec![foreign.pid]).unwrap_or_else(|error| {
            // A root daemon with a readable executable is a service, not a
            // critical process; anything else skips this environment.
            panic!("a non-critical foreign process should produce a plan: {error}")
        });
        assert_eq!(plan.items.len(), 1);
        assert_eq!(
            plan.items[0].decision,
            ProcessEndDecision::Refused(ProcessEndRefusal::OwnedByOtherUser)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn plan_marks_missing_pids_as_typed_refusals() {
        let _plan_guard = PLAN_TEST_LOCK
            .lock()
            .expect("the plan test lock should be free");
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("true should start");
        child.wait().expect("true should exit");
        let plan = ProcessControlService::prepare_end(vec![child.id()])
            .expect("a missing pid should still produce a plan");
        assert_eq!(
            plan.items[0].decision,
            ProcessEndDecision::Refused(ProcessEndRefusal::ProcessNotFound)
        );
    }

    /// Happy path: plan and gracefully end a real fixture process, then
    /// verify the remaining-process authority reports nothing left.
    #[cfg(target_os = "linux")]
    #[test]
    fn execute_ends_a_spawned_fixture_process() {
        let _operation_guard = crate::shared::operation::test_operation_lock();
        let _plan_guard = PLAN_TEST_LOCK
            .lock()
            .expect("the plan test lock should be free");
        let mut child = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .expect("sleep should start");
        let pid = child.id();
        // Reap concurrently so a zombie cannot look alive to verification.
        let reaper = std::thread::spawn(move || child.wait());

        let plan = ProcessControlService::prepare_end(vec![pid]).expect("the plan should succeed");
        assert_eq!(plan.items[0].decision, ProcessEndDecision::Allowed);
        let consumed_plan = plan.clone();
        let result = ProcessControlService::execute_end(plan, ProcessEndMode::Graceful, true)
            .expect("execution should succeed");

        // Safety net: never leave the fixture behind on assertion failure.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        let _ = reaper.join();

        assert_eq!(result.ended_count, 1);
        assert!(result.remaining_pids.is_empty());
        assert_eq!(result.items[0].status, ProcessEndItemStatus::Ended);
        assert!(
            ProcessControlService::execute_end(consumed_plan, ProcessEndMode::Graceful, true)
                .is_err(),
            "a consumed plan must not be executable twice"
        );
    }

    /// Force escalation: a SIGTERM-ignoring process survives the graceful
    /// pass and is ended only after the explicit force pass.
    #[cfg(target_os = "linux")]
    #[test]
    fn execute_force_ends_a_term_ignoring_process() {
        let _operation_guard = crate::shared::operation::test_operation_lock();
        let _plan_guard = PLAN_TEST_LOCK
            .lock()
            .expect("the plan test lock should be free");
        let mut child = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; sleep 300"])
            .spawn()
            .expect("sh should start");
        let pid = child.id();
        let reaper = std::thread::spawn(move || child.wait());

        let graceful_plan =
            ProcessControlService::prepare_end(vec![pid]).expect("the plan should succeed");
        let graceful =
            ProcessControlService::execute_end(graceful_plan, ProcessEndMode::Graceful, true)
                .expect("graceful execution should succeed");
        assert_eq!(graceful.items[0].status, ProcessEndItemStatus::StillRunning);
        assert_eq!(graceful.remaining_pids, vec![pid]);

        let force_plan =
            ProcessControlService::prepare_end(vec![pid]).expect("the force plan should succeed");
        let force = ProcessControlService::execute_end(force_plan, ProcessEndMode::Force, true)
            .expect("force execution should succeed");

        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        let _ = reaper.join();

        assert_eq!(force.ended_count, 1);
        assert!(force.remaining_pids.is_empty());
        assert_eq!(force.items[0].status, ProcessEndItemStatus::EndedAfterForce);
    }
}
