use std::collections::HashSet;

use serde::Serialize;

use mangodisk_core::{
    associate_applications, build_process_tree, classify_process, ProcessApplicationAssociations,
    ProcessClassification, ProcessClassificationFacts, ProcessControlService, ProcessEndPlan,
    ProcessEndResult, ProcessInventoryService, ProcessScanFilter, ProcessSnapshot, ProcessTree,
};
use mangodisk_platform::ProcessEndMode;

use super::error::{run_blocking, CommandResult};

/// One process classification computed by Core, keyed by pid so the frontend
/// never duplicates the classification rule matrix.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessClassificationEntry {
    pub pid: u32,
    pub classification: ProcessClassification,
}

/// Presentation-ready composition over one inventory scan. The scan itself
/// stays the single expensive operation; the tree, application associations,
/// and classifications are deterministic Core projections of the same samples.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessScanView {
    pub snapshot: ProcessSnapshot,
    pub tree: ProcessTree,
    pub associations: ProcessApplicationAssociations,
    pub classifications: Vec<ProcessClassificationEntry>,
}

fn build_scan_view(snapshot: ProcessSnapshot) -> ProcessScanView {
    let tree = build_process_tree(&snapshot.processes);
    let associations = associate_applications(&snapshot.processes);
    let associated: HashSet<u32> = associations
        .matches
        .iter()
        .filter(|entry| entry.application_identifier.is_some())
        .map(|entry| entry.pid)
        .collect();
    let classifications = snapshot
        .processes
        .iter()
        .map(|sample| ProcessClassificationEntry {
            pid: sample.pid,
            classification: classify_process(&ProcessClassificationFacts::from_sample(
                sample,
                associated.contains(&sample.pid),
            )),
        })
        .collect();
    ProcessScanView {
        snapshot,
        tree,
        associations,
        classifications,
    }
}

/// Runs the two-sample inventory scan (~500 ms) off the async runtime and
/// returns the snapshot together with its Core-derived projections.
#[tauri::command]
pub async fn scan_processes(filter: ProcessScanFilter) -> CommandResult<ProcessScanView> {
    run_blocking("scan_processes", move || {
        ProcessInventoryService::scan(filter).map(build_scan_view)
    })
    .await
}

#[tauri::command]
pub async fn prepare_process_end(pids: Vec<u32>) -> CommandResult<ProcessEndPlan> {
    run_blocking("prepare_process_end", move || {
        ProcessControlService::prepare_end(pids)
    })
    .await
}

/// Executes a previously prepared plan. `confirmed` mirrors the Core gate: an
/// unconfirmed request is a typed invalid-input refusal and never reaches the
/// platform, so the confirmation dialog remains the only execution path.
#[tauri::command]
pub async fn execute_process_end(
    plan: ProcessEndPlan,
    mode: ProcessEndMode,
    confirmed: bool,
) -> CommandResult<ProcessEndResult> {
    run_blocking("execute_process_end", move || {
        ProcessControlService::execute_end(plan, mode, confirmed)
    })
    .await
}
