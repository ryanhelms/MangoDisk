use std::collections::{HashMap, HashSet};

use mangodisk_core::{
    associate_applications, classify_process, top_processes_by_cpu, top_processes_by_rss,
    top_processes_by_write_rate, AnalysisService, ApplicationLeftoverService,
    ApplicationUninstallBatchSelection, ApplicationUninstallService, CleanupRequest,
    CleanupScanResult, CleanupScanService, CleanupService, DuplicateFileService,
    DuplicateFilesResult, HistoryService, LargeFileService, LargeFilesResult,
    OperationCancellationToken, PermanentDeleteCandidate, ProcessApplicationMatch,
    ProcessClassification, ProcessClassificationFacts, ProcessControlService, ProcessEndPlan,
    ProcessEndResult, ProcessInventoryService, ProcessSample, ProcessScanFilter, ProcessSnapshot,
    StartupDesiredState, StartupService, SystemSettingTargetState, SystemSettingsChangeSelection,
    SystemSettingsService,
};
use mangodisk_platform::ProcessEndMode;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData},
    service::RequestContext,
    tool, tool_router, RoleServer,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    core_runner::CoreOperation,
    errors,
    execution_tokens::MutationDomain,
    server::{AdapterState, MangoDiskServer},
};

/// Default process count for a topBy ranking.
const DEFAULT_PROCESS_TOP_LIMIT: usize = 10;
/// Cap for topBy rankings; larger requests are clamped so a client cannot
/// turn the bounded scan into an unbounded serialization.
const MAX_PROCESS_TOP_LIMIT: usize = 100;

/// Default minimum size for large-file discovery (100 MiB).
const DEFAULT_LARGE_FILE_MINIMUM_BYTES: u64 = 100 * 1024 * 1024;
/// Default minimum size for duplicate detection (1 MiB); smaller duplicates
/// rarely reclaim meaningful space.
const DEFAULT_DUPLICATE_MINIMUM_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CleanupScanInput {
    /// Also discover build artifacts in project directories outside the usual caches. Slower, finds more.
    #[serde(default)]
    deep_project_discovery: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AnalyzeStorageInput {
    /// Absolute directory to analyze. Defaults to the user home directory.
    #[serde(default)]
    path: Option<String>,
    /// Ignore cached sizes and rescan from disk.
    #[serde(default)]
    refresh: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FindLargeFilesInput {
    /// Absolute directory to scan. Defaults to the user home directory.
    #[serde(default)]
    path: Option<String>,
    /// Minimum file size in bytes. Defaults to 100 MiB.
    #[serde(default = "default_large_file_minimum_bytes")]
    minimum_bytes: u64,
    /// Ignore cached results and rescan from disk.
    #[serde(default)]
    refresh: bool,
}

const fn default_large_file_minimum_bytes() -> u64 {
    DEFAULT_LARGE_FILE_MINIMUM_BYTES
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FindDuplicateFilesInput {
    /// Absolute directories to scan. At least one is required; scanning is confined to these roots.
    roots: Vec<String>,
    /// Minimum file size in bytes. Defaults to 1 MiB.
    #[serde(default = "default_duplicate_minimum_bytes")]
    minimum_bytes: u64,
    /// When set (with limit), return that page of duplicate groups instead of the first page.
    #[serde(default)]
    offset: Option<u64>,
    /// Page size for offset-based paging.
    #[serde(default)]
    limit: Option<u64>,
}

const fn default_duplicate_minimum_bytes() -> u64 {
    DEFAULT_DUPLICATE_MINIMUM_BYTES
}

/// Common guard fields for every mutation tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecutionGuardInput {
    /// The executionToken returned by the matching scan tool. Single-use, expires 10 minutes after the scan.
    token: String,
    /// Must be true to execute. Confirms the preview was reviewed.
    confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CleanupExecuteInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Rule identifiers from the cleanup scan preview. Must be a subset of the scanned selectable rules.
    rule_ids: Vec<String>,
    /// Validate everything but change nothing on disk.
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PermanentDeleteCandidateInput {
    /// Absolute path of the file to delete, copied from the scan result.
    path: String,
    /// Size in bytes reported by the scan; deletion is refused if the file changed.
    expected_bytes: u64,
    /// Modification time in milliseconds reported by the scan, when present.
    #[serde(default)]
    expected_modified_at_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PermanentDeleteInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Files to delete permanently. Bound to the scan session that issued the token; deleting every copy of a duplicate group is refused.
    candidates: Vec<PermanentDeleteCandidateInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UninstallSelectionInput {
    /// Application identifier from the applications scan.
    application_id: String,
    /// Component identifiers to remove. Empty selects the default component set.
    #[serde(default)]
    component_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApplicationUninstallExecuteInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Applications to uninstall. Must be a subset of the scanned catalog.
    selections: Vec<UninstallSelectionInput>,
    /// Validate everything but change nothing on disk.
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApplicationLeftoversExecuteInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Leftover candidate identifiers from the leftovers scan. Must be a subset of the scanned candidates.
    candidate_ids: Vec<String>,
    /// Validate everything but change nothing on disk.
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StartupDesiredStateInput {
    Enabled,
    Disabled,
    Removed,
}

impl From<StartupDesiredStateInput> for StartupDesiredState {
    fn from(value: StartupDesiredStateInput) -> Self {
        match value {
            StartupDesiredStateInput::Enabled => Self::Enabled,
            StartupDesiredStateInput::Disabled => Self::Disabled,
            StartupDesiredStateInput::Removed => Self::Removed,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartupApplyInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Startup item identifiers from the startup scan. Must be a subset of the scanned items.
    item_ids: Vec<String>,
    /// State to apply to every selected item.
    desired_state: StartupDesiredStateInput,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SystemSettingTargetInput {
    Optimized,
    Default,
}

impl From<SystemSettingTargetInput> for SystemSettingTargetState {
    fn from(value: SystemSettingTargetInput) -> Self {
        match value {
            SystemSettingTargetInput::Optimized => Self::Optimized,
            SystemSettingTargetInput::Default => Self::Default,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SystemSettingChangeInput {
    /// Setting identifier from the system settings scan.
    setting_id: String,
    /// Target state for the setting.
    target: SystemSettingTargetInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SystemSettingsApplyInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Settings to change. Must be a subset of the scanned settings.
    items: Vec<SystemSettingChangeInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessesScanInput {
    /// Only include processes whose name contains this text (case-insensitive).
    #[serde(default)]
    name_contains: Option<String>,
    /// Only include processes owned by this account name or numeric uid.
    #[serde(default)]
    user: Option<String>,
    /// Only include processes with at least this much resident memory in bytes.
    #[serde(default)]
    min_rss_bytes: Option<u64>,
    /// Rank the result by one metric and keep only the top entries. Without it the full pid-ordered list is returned.
    #[serde(default)]
    top_by: Option<ProcessTopByInput>,
    /// Maximum processes kept when topBy is set. Defaults to 10; values above 100 are capped. Ignored without topBy.
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ProcessTopByInput {
    Cpu,
    Rss,
    WriteRate,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ProcessEndModeInput {
    /// Ask the process to exit and wait briefly for it to disappear.
    #[default]
    Graceful,
    /// Escalate to an uninterruptible kill for processes still alive after the graceful pass.
    Force,
}

impl From<ProcessEndModeInput> for ProcessEndMode {
    fn from(value: ProcessEndModeInput) -> Self {
        match value {
            ProcessEndModeInput::Graceful => Self::Graceful,
            ProcessEndModeInput::Force => Self::Force,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessEndInput {
    #[serde(flatten)]
    guard: ExecutionGuardInput,
    /// Process identifiers from the processes_scan result. Must be a subset of the listed processes.
    pids: Vec<u32>,
    /// End mode. Defaults to graceful.
    #[serde(default)]
    mode: ProcessEndModeInput,
    /// Build the plan and report per-process decisions without ending anything.
    dry_run: bool,
}

/// One listed process: the Core sample flattened, enriched with the product
/// classification and the installed-application association when the platform
/// inventory provides one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessListEntry {
    #[serde(flatten)]
    sample: ProcessSample,
    classification: ProcessClassification,
    #[serde(skip_serializing_if = "Option::is_none")]
    application_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    application_name: Option<String>,
}

/// The Core snapshot metadata with the listed (filtered, optionally ranked)
/// processes. `ProcessSnapshot` is frozen, so the enrichment lives in
/// `ProcessListEntry` instead of a patched Core type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessesScanOutput {
    schema_version: u32,
    snapshot_id: String,
    captured_at_ms: u64,
    sample_interval_ms: u64,
    cpu_ticks_per_second: u64,
    logical_cpu_count: u32,
    new_process_count: u64,
    exited_process_count: u64,
    processes: Vec<ProcessListEntry>,
}

/// Outcome of `process_end`: a dry run reports the prepared plan, an execution
/// reports the Core result whose `remaining_pids` is the final authority.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessEndOutput {
    /// True when only the plan was prepared and nothing was ended.
    dry_run: bool,
    /// The prepared plan with per-process decisions; present on dry runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<ProcessEndPlan>,
    /// The execution outcome; present when the plan was executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ProcessEndResult>,
}

impl ProcessEndOutput {
    fn plan_preview(plan: ProcessEndPlan) -> Self {
        Self {
            dry_run: true,
            plan: Some(plan),
            result: None,
        }
    }

    fn executed(result: ProcessEndResult) -> Self {
        Self {
            dry_run: false,
            plan: None,
            result: Some(result),
        }
    }
}

#[tool_router(vis = "pub(crate)")]
impl MangoDiskServer {
    #[tool(
        description = "Scan the system for safe cleanup candidates (caches, build artifacts, browser data). Read-only. When mutations are enabled, the response includes an executionToken for cleanup_execute.",
        annotations(read_only_hint = true)
    )]
    async fn cleanup_scan(
        &self,
        Parameters(input): Parameters<CleanupScanInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::cleanup_scan()), &context);
        let scan = match operation
            .run("cleanup_scan", move |progress| {
                CleanupScanService::scan_with_deep_project_discovery(
                    input.deep_project_discovery,
                    progress.sink(),
                )
            })
            .await
        {
            Ok(scan) => scan,
            Err(error) => return Ok(error),
        };
        let token = self
            .state
            .issue_token(MutationDomain::Cleanup, cleanup_snapshot(&scan));
        Ok(self.respond(&scan, token))
    }

    #[tool(
        description = "Analyze disk usage of a directory tree (default: user home). Returns entries with sizes. Read-only.",
        annotations(read_only_hint = true)
    )]
    async fn analyze_storage(
        &self,
        Parameters(input): Parameters<AnalyzeStorageInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = CoreOperation::new(Some(OperationCancellationToken::analysis()), &context);
        let result = match operation
            .run("analyze_storage", move |progress| {
                AnalysisService::analyze_with_progress(input.path, input.refresh, progress.sink())
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Find files at or above a size threshold in a directory tree (default: user home). Read-only. When mutations are enabled, the response includes an executionToken for permanent_delete.",
        annotations(read_only_hint = true)
    )]
    async fn find_large_files(
        &self,
        Parameters(input): Parameters<FindLargeFilesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::large_files()), &context);
        let result = match operation
            .run("find_large_files", move |progress| {
                LargeFileService::find_with_progress(
                    input.path,
                    input.minimum_bytes,
                    input.refresh,
                    progress.sink(),
                )
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::PermanentDelete,
            large_files_snapshot(&result),
        );
        Ok(self.respond(&result, token))
    }

    #[tool(
        description = "Find exact-content duplicate files under the given roots. Read-only. When mutations are enabled, the response includes an executionToken for permanent_delete.",
        annotations(read_only_hint = true)
    )]
    async fn find_duplicate_files(
        &self,
        Parameters(input): Parameters<FindDuplicateFilesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = CoreOperation::new(
            Some(OperationCancellationToken::duplicate_files()),
            &context,
        );
        let roots = input.roots;
        let minimum_bytes = input.minimum_bytes;
        let result = match operation
            .run("find_duplicate_files", move |progress| {
                DuplicateFileService::find_paged_with_progress(
                    roots,
                    minimum_bytes,
                    progress.sink(),
                    |_| {},
                )
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::PermanentDelete,
            duplicates_snapshot(&result),
        );
        if input.offset.is_some() || input.limit.is_some() {
            let page = match DuplicateFileService::page(
                result.scan_id,
                input.offset.unwrap_or(0),
                input.limit.unwrap_or(50),
            ) {
                Ok(page) => page,
                Err(detail) => {
                    return Ok(errors::validation_failure("find_duplicate_files", detail))
                }
            };
            return Ok(self.respond(&page, token));
        }
        Ok(self.respond(&result, token))
    }

    #[tool(
        description = "Scan installed applications with their uninstallable components and sizes. Read-only. When mutations are enabled, the response includes an executionToken for application_uninstall_execute.",
        annotations(read_only_hint = true)
    )]
    async fn applications_scan(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = CoreOperation::new(
            Some(OperationCancellationToken::application_scan()),
            &context,
        );
        let scan = match operation
            .run("applications_scan", move |progress| {
                ApplicationUninstallService::scan_with_progress(progress.sink())
            })
            .await
        {
            Ok(scan) => scan,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::ApplicationUninstall,
            json!({
                "applicationIds": scan
                    .candidates
                    .iter()
                    .map(|candidate| candidate.application_id.clone())
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(self.respond(&scan, token))
    }

    #[tool(
        description = "Scan for leftover files of already-uninstalled applications. Read-only. When mutations are enabled, the response includes an executionToken for application_leftovers_execute.",
        annotations(read_only_hint = true)
    )]
    async fn application_leftovers_scan(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // The leftover scan runs under the shared applications operation kind
        // in Core, so it is cancelled through the matching public token.
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::applications()), &context);
        let scan = match operation
            .run("application_leftovers_scan", move |_| {
                ApplicationLeftoverService::scan()
            })
            .await
        {
            Ok(scan) => scan,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::ApplicationLeftovers,
            json!({
                "candidateIds": scan
                    .candidates
                    .iter()
                    .map(|candidate| candidate.candidate_id.clone())
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(self.respond(&scan, token))
    }

    #[tool(
        description = "Scan startup and login items with their current state. Read-only. When mutations are enabled, the response includes an executionToken for startup_apply.",
        annotations(read_only_hint = true)
    )]
    async fn startup_scan(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::startup_scan()), &context);
        let catalog = match operation
            .run("startup_scan", move |_| StartupService::scan())
            .await
        {
            Ok(catalog) => catalog,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::Startup,
            json!({
                "scanId": catalog.scan_id.clone(),
                "itemIds": catalog
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.item_id.clone())
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(self.respond(&catalog, token))
    }

    #[tool(
        description = "Scan tunable system settings with their current and recommended states. Read-only. When mutations are enabled, the response includes an executionToken for system_settings_apply.",
        annotations(read_only_hint = true)
    )]
    async fn system_settings_scan(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = CoreOperation::new(
            Some(OperationCancellationToken::system_settings_scan()),
            &context,
        );
        let catalog = match operation
            .run("system_settings_scan", move |_| {
                SystemSettingsService::scan()
            })
            .await
        {
            Ok(catalog) => catalog,
            Err(error) => return Ok(error),
        };
        let token = self.state.issue_token(
            MutationDomain::SystemSettings,
            json!({
                "scanId": catalog.scan_id.clone(),
                "settingIds": catalog
                    .items
                    .iter()
                    .map(|item| item.setting_id.clone())
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(self.respond(&catalog, token))
    }

    #[tool(
        description = "List recent MangoDisk operation records (scans, cleanups, deletions) with outcomes and reclaimed bytes. Read-only.",
        annotations(read_only_hint = true)
    )]
    async fn operation_history(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = CoreOperation::new(None, &context);
        let records = match operation
            .run("operation_history", move |_| HistoryService::list())
            .await
        {
            Ok(records) => records,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&json!({ "records": records }), None))
    }

    #[tool(
        description = "Scan running processes with CPU, memory, and IO metrics plus classification and application association. Read-only. The scan takes two counter samples about 500 ms apart, so it is bounded and streams no progress. When mutations are enabled, the response includes an executionToken for process_end covering the listed pids.",
        annotations(read_only_hint = true)
    )]
    async fn processes_scan(
        &self,
        Parameters(input): Parameters<ProcessesScanInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // Core exposes no process-scan cancellation token; the two-sample scan
        // is bounded (~500 ms), so there is nothing to cancel or stream.
        let operation = CoreOperation::new(None, &context);
        let filter = ProcessScanFilter {
            name_contains: input.name_contains,
            user: input.user,
            min_rss_bytes: input.min_rss_bytes,
        };
        let top_by = input.top_by;
        let limit = input
            .limit
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(DEFAULT_PROCESS_TOP_LIMIT)
            .min(MAX_PROCESS_TOP_LIMIT);
        let output = match operation
            .run("processes_scan", move |_| {
                let snapshot = ProcessInventoryService::scan(filter)?;
                Ok(processes_scan_output(snapshot, top_by, limit))
            })
            .await
        {
            Ok(output) => output,
            Err(error) => return Ok(error),
        };
        let token = self
            .state
            .issue_token(MutationDomain::ProcessEnd, processes_snapshot(&output));
        Ok(self.respond(&output, token))
    }

    #[tool(
        description = "Execute cleanup for selected rules from a cleanup_scan preview. Requires --enable-mutations, the scan's executionToken, and confirm: true. Supports dry_run. Streams per-rule progress when the request includes a progressToken.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn cleanup_execute(
        &self,
        Parameters(input): Parameters<CleanupExecuteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.rule_ids.is_empty() {
            return Ok(errors::validation_failure(
                "cleanup_execute",
                "ruleIds must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::Cleanup)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        if let Err(error) = ensure_within_snapshot(&input.rule_ids, &snapshot, "ruleIds", "rule") {
            return Ok(*error);
        }
        let request = CleanupRequest {
            rule_ids: input.rule_ids,
            dry_run: input.dry_run,
            project_roots: Vec::new(),
            source_selections: Vec::new(),
        };
        let operation = CoreOperation::new(Some(OperationCancellationToken::cleanup()), &context);
        let result = match operation
            .run("cleanup_execute", move |progress| {
                CleanupService::execute_with_progress(request, progress.cleanup_reporter())
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Permanently delete files reported by find_duplicate_files or find_large_files. Requires --enable-mutations, the scan's executionToken, and confirm: true. Files are not moved to trash. Candidate paths must match the scan session, so this tool is only usable when the server runs with --include-full-paths.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn permanent_delete(
        &self,
        Parameters(input): Parameters<PermanentDeleteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.candidates.is_empty() {
            return Ok(errors::validation_failure(
                "permanent_delete",
                "candidates must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::PermanentDelete)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        let scan_id = match snapshot["scanId"].as_u64() {
            Some(scan_id) => scan_id,
            None => {
                return Ok(errors::tool_error(
                    errors::PLAN_MISMATCH,
                    "the execution token snapshot has no scan session; scan again",
                ));
            }
        };
        let source = snapshot["source"].as_str().unwrap_or_default().to_string();
        let candidates = input
            .candidates
            .into_iter()
            .map(|candidate| PermanentDeleteCandidate {
                path: candidate.path,
                expected_bytes: candidate.expected_bytes,
                expected_modified_at_ms: candidate.expected_modified_at_ms,
            })
            .collect::<Vec<_>>();
        // Core exposes no permanent-delete cancellation token; deletion is
        // atomic per candidate, so a cancelled request cannot leave the
        // operation half-committed beyond the file in flight. The Core API is
        // a single batch call with no progress callback, so there is nothing
        // to stream — the tool result itself is the completion signal.
        let operation = CoreOperation::new(None, &context);
        let result = match operation
            .run("permanent_delete", move |_| match source.as_str() {
                "duplicateFiles" => {
                    DuplicateFileService::delete_files_permanently(scan_id, candidates)
                }
                "largeFiles" => LargeFileService::delete_files_permanently(
                    scan_id,
                    candidates
                        .into_iter()
                        .map(|candidate| candidate.path)
                        .collect(),
                ),
                _ => Err(mangodisk_core::CoreError::invalid_input(
                    "the execution token snapshot has an unknown scan source",
                )),
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Uninstall applications selected in an applications_scan preview. Requires --enable-mutations, the scan's executionToken, and confirm: true. Supports dry_run. Streams per-application progress when the request includes a progressToken.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn application_uninstall_execute(
        &self,
        Parameters(input): Parameters<ApplicationUninstallExecuteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.selections.is_empty() {
            return Ok(errors::validation_failure(
                "application_uninstall_execute",
                "selections must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::ApplicationUninstall)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        let requested = input
            .selections
            .iter()
            .map(|selection| selection.application_id.clone())
            .collect::<Vec<_>>();
        if let Err(error) =
            ensure_within_snapshot(&requested, &snapshot, "applicationIds", "application")
        {
            return Ok(*error);
        }
        let selections = input
            .selections
            .into_iter()
            .map(|selection| ApplicationUninstallBatchSelection {
                application_id: selection.application_id,
                component_ids: selection.component_ids,
            })
            .collect::<Vec<_>>();
        let dry_run = input.dry_run;
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::applications()), &context);
        let result = match operation
            .run("application_uninstall_execute", move |progress| {
                let plan = ApplicationUninstallService::create_batch_plan(&selections)?;
                ApplicationUninstallService::execute_batch_with_progress(
                    plan,
                    dry_run,
                    None,
                    progress.uninstall_reporter(),
                )
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Remove leftover files selected in an application_leftovers_scan preview. Requires --enable-mutations, the scan's executionToken, and confirm: true. Supports dry_run.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn application_leftovers_execute(
        &self,
        Parameters(input): Parameters<ApplicationLeftoversExecuteInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.candidate_ids.is_empty() {
            return Ok(errors::validation_failure(
                "application_leftovers_execute",
                "candidateIds must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::ApplicationLeftovers)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        if let Err(error) =
            ensure_within_snapshot(&input.candidate_ids, &snapshot, "candidateIds", "candidate")
        {
            return Ok(*error);
        }
        let candidate_ids = input.candidate_ids;
        let dry_run = input.dry_run;
        let operation = CoreOperation::new(
            Some(OperationCancellationToken::application_leftover_cleanup()),
            &context,
        );
        let result = match operation
            .run("application_leftovers_execute", move |_| {
                // The scan type is serialize-only, so the plan is rebuilt from
                // a fresh scan instead of a stored snapshot. Core then rescans
                // once more inside execute and verifies every fingerprint
                // before deleting, so drift fails closed.
                let scan = ApplicationLeftoverService::scan()?;
                let plan = ApplicationLeftoverService::create_plan(&scan, &candidate_ids)
                    .map_err(mangodisk_core::CoreError::invalid_input)?;
                let operation_id = format!(
                    "mcp-leftovers-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis())
                        .unwrap_or_default()
                );
                ApplicationLeftoverService::execute(plan, dry_run, operation_id)
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Enable, disable, or remove startup items selected in a startup_scan preview. Requires --enable-mutations, the scan's executionToken, and confirm: true.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn startup_apply(
        &self,
        Parameters(input): Parameters<StartupApplyInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.item_ids.is_empty() {
            return Ok(errors::validation_failure(
                "startup_apply",
                "itemIds must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::Startup)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        if let Err(error) = ensure_within_snapshot(&input.item_ids, &snapshot, "itemIds", "item") {
            return Ok(*error);
        }
        let scan_id = match snapshot["scanId"].as_str() {
            Some(scan_id) => scan_id.to_string(),
            None => {
                return Ok(errors::tool_error(
                    errors::PLAN_MISMATCH,
                    "the execution token snapshot has no startup scan session; scan again",
                ));
            }
        };
        let selection = mangodisk_core::StartupChangeSelection {
            scan_id,
            item_ids: input.item_ids,
            desired_state: input.desired_state.into(),
        };
        let operation =
            CoreOperation::new(Some(OperationCancellationToken::startup_change()), &context);
        let result = match operation
            .run("startup_apply", move |_| {
                // prepare_change revalidates every item against a fresh
                // platform scan and execute_change consumes the server-side
                // pending plan, so the fused call keeps Core's own two-phase
                // safety chain intact.
                let plan = StartupService::prepare_change(selection)?;
                StartupService::execute_change(plan.plan_id, None)
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "Apply optimized or default states to settings selected in a system_settings_scan preview. Requires --enable-mutations, the scan's executionToken, and confirm: true.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn system_settings_apply(
        &self,
        Parameters(input): Parameters<SystemSettingsApplyInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if input.items.is_empty() {
            return Ok(errors::validation_failure(
                "system_settings_apply",
                "items must not be empty".to_string(),
            ));
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::SystemSettings)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        let requested = input
            .items
            .iter()
            .map(|item| item.setting_id.clone())
            .collect::<Vec<_>>();
        if let Err(error) = ensure_within_snapshot(&requested, &snapshot, "settingIds", "setting") {
            return Ok(*error);
        }
        let scan_id = match snapshot["scanId"].as_str() {
            Some(scan_id) => scan_id.to_string(),
            None => {
                return Ok(errors::tool_error(
                    errors::PLAN_MISMATCH,
                    "the execution token snapshot has no system settings scan session; scan again",
                ));
            }
        };
        let selection = SystemSettingsChangeSelection {
            scan_id,
            items: input
                .items
                .into_iter()
                .map(|item| mangodisk_core::SystemSettingChangeSelectionItem {
                    setting_id: item.setting_id,
                    target: item.target.into(),
                })
                .collect(),
        };
        // Core exposes no cancellation token for system-settings changes.
        let operation = CoreOperation::new(None, &context);
        let result = match operation
            .run("system_settings_apply", move |_| {
                let plan = SystemSettingsService::prepare_change(selection)?;
                SystemSettingsService::execute_change(plan.plan_id)
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }

    #[tool(
        description = "End processes listed by processes_scan. Requires --enable-mutations, the scan's executionToken, and confirm: true. Defaults to a graceful end; mode: \"force\" escalates against survivors after a bounded wait. dry_run reports the plan decisions without ending anything. Core hard-refuses itself, pid 0/1, and critical system processes, revalidates process identity before signalling, and reports remainingPids as the final authority. Execution prepares a fresh Core plan inside the call; Core plans expire after 5 minutes while the executionToken lives 10, so a stale or superseded plan fails closed with a typed reason. The operation is bounded to a few seconds and streams no progress.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    async fn process_end(
        &self,
        Parameters(input): Parameters<ProcessEndInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(error) = self.state.ensure_mutations_enabled() {
            return Ok(*error);
        }
        if let Err(error) = AdapterState::ensure_confirmed(input.guard.confirm) {
            return Ok(*error);
        }
        if let Err(error) = ensure_pids_present(&input.pids) {
            return Ok(*error);
        }
        let snapshot = match self
            .state
            .take_token(&input.guard.token, MutationDomain::ProcessEnd)
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(*error),
        };
        if let Err(error) = ensure_pids_within_snapshot(&input.pids, &snapshot) {
            return Ok(*error);
        }
        let pids = input.pids;
        let mode = input.mode;
        let dry_run = input.dry_run;
        // Core exposes no process-end cancellation token; the operation is
        // bounded (graceful wait plus an optional force wait), so the tool
        // result itself is the completion signal.
        let operation = CoreOperation::new(None, &context);
        let result = match operation
            .run("process_end", move |_| {
                // prepare_end revalidates every pid against a fresh platform
                // snapshot and execute_end re-checks identity before
                // signalling, so the fused call keeps Core's own two-phase
                // safety chain intact.
                let plan = ProcessControlService::prepare_end(pids)?;
                if dry_run {
                    return Ok(ProcessEndOutput::plan_preview(plan));
                }
                let result = ProcessControlService::execute_end(plan, mode.into(), true)?;
                Ok(ProcessEndOutput::executed(result))
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(error),
        };
        Ok(self.respond(&result, None))
    }
}

fn cleanup_snapshot(scan: &CleanupScanResult) -> Value {
    json!({
        "ruleIds": scan
            .rules
            .iter()
            .filter(|rule| rule.selectable && rule.bytes > 0)
            .map(|rule| rule.rule_id.clone())
            .collect::<Vec<_>>(),
    })
}

fn large_files_snapshot(result: &LargeFilesResult) -> Value {
    json!({ "source": "largeFiles", "scanId": result.scan_id })
}

fn duplicates_snapshot(result: &DuplicateFilesResult) -> Value {
    json!({ "source": "duplicateFiles", "scanId": result.scan_id })
}

/// Builds the scan response: optionally ranks and truncates the listed
/// processes, then enriches each with classification and application
/// association. Runs inside the blocking Core worker because association reads
/// the platform application inventory.
fn processes_scan_output(
    snapshot: ProcessSnapshot,
    top_by: Option<ProcessTopByInput>,
    limit: usize,
) -> ProcessesScanOutput {
    let ProcessSnapshot {
        schema_version,
        snapshot_id,
        captured_at_ms,
        sample_interval_ms,
        cpu_ticks_per_second,
        logical_cpu_count,
        new_process_count,
        exited_process_count,
        processes,
    } = snapshot;
    let processes: Vec<ProcessSample> = match top_by {
        Some(ProcessTopByInput::Cpu) => top_processes_by_cpu(&processes, limit)
            .into_iter()
            .cloned()
            .collect(),
        Some(ProcessTopByInput::Rss) => top_processes_by_rss(&processes, limit)
            .into_iter()
            .cloned()
            .collect(),
        Some(ProcessTopByInput::WriteRate) => top_processes_by_write_rate(&processes, limit)
            .into_iter()
            .cloned()
            .collect(),
        None => processes,
    };
    let associations = associate_applications(&processes);
    let matches: HashMap<u32, &ProcessApplicationMatch> = associations
        .matches
        .iter()
        .map(|entry| (entry.pid, entry))
        .collect();
    let entries = processes
        .into_iter()
        .map(|sample| {
            let application = matches.get(&sample.pid).copied();
            let facts = ProcessClassificationFacts::from_sample(
                &sample,
                application.is_some_and(|entry| entry.application_identifier.is_some()),
            );
            ProcessListEntry {
                sample,
                classification: classify_process(&facts),
                application_identifier: application
                    .and_then(|entry| entry.application_identifier.clone()),
                application_name: application.and_then(|entry| entry.application_name.clone()),
            }
        })
        .collect();
    ProcessesScanOutput {
        schema_version,
        snapshot_id,
        captured_at_ms,
        sample_interval_ms,
        cpu_ticks_per_second,
        logical_cpu_count,
        new_process_count,
        exited_process_count,
        processes: entries,
    }
}

/// The token snapshot binds execution to the exact listed processes. pid and
/// startedAtMs together identify a process; Core revalidates that identity
/// against a fresh platform snapshot at prepare and execute time.
fn processes_snapshot(output: &ProcessesScanOutput) -> Value {
    json!({
        "processes": output
            .processes
            .iter()
            .map(|entry| json!({ "pid": entry.sample.pid, "startedAtMs": entry.sample.started_at_ms }))
            .collect::<Vec<_>>(),
    })
}

/// Rejects an empty pid selection before the token is consumed so a malformed
/// call never burns a grant, matching the guard ordering of the other mutation
/// tools. Boxed for the same cold-path `result_large_err` reason as the other
/// guard helpers.
fn ensure_pids_present(pids: &[u32]) -> Result<(), Box<CallToolResult>> {
    if pids.is_empty() {
        return Err(Box::new(errors::validation_failure(
            "process_end",
            "pids must not be empty".to_string(),
        )));
    }
    Ok(())
}

/// Pid-set counterpart to `ensure_within_snapshot`: every requested pid must
/// have been listed by the scan that issued the token. PID-reuse revalidation
/// itself stays with Core at prepare and execute time.
fn ensure_pids_within_snapshot(
    requested: &[u32],
    snapshot: &Value,
) -> Result<(), Box<CallToolResult>> {
    let allowed = snapshot
        .get("processes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("pid").and_then(Value::as_u64))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if let Some(unknown) = requested
        .iter()
        .find(|pid| !allowed.contains(&u64::from(**pid)))
    {
        log::info!("mcp_plan_mismatch key=processes");
        return Err(Box::new(errors::tool_error(
            errors::PLAN_MISMATCH,
            format!(
                "pid {unknown} is not part of the process scan preview; run processes_scan again"
            ),
        )));
    }
    Ok(())
}

/// Rejects an execute selection that reaches beyond what the preview scan
/// reported. The token snapshot is the adapter-side plan binding; Core still
/// performs its own freshness validation at the mutation boundary. The error
/// is boxed for the same cold-path `result_large_err` reason as the guards in
/// `server.rs`.
fn ensure_within_snapshot(
    requested: &[String],
    snapshot: &Value,
    key: &str,
    kind: &str,
) -> Result<(), Box<CallToolResult>> {
    let allowed = snapshot
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if let Some(unknown) = requested
        .iter()
        .find(|identifier| !allowed.contains(identifier.as_str()))
    {
        log::info!("mcp_plan_mismatch key={key}");
        return Err(Box::new(errors::tool_error(
            errors::PLAN_MISMATCH,
            format!(
                "{kind} `{unknown}` is not part of the scan preview; run the matching scan again"
            ),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_outside_the_snapshot_are_rejected() {
        let snapshot = json!({ "ruleIds": ["development.npm-cache"] });

        let error = ensure_within_snapshot(
            &["system.unknown-rule".to_string()],
            &snapshot,
            "ruleIds",
            "rule",
        )
        .expect_err("a rule outside the preview must be rejected");

        let json = serde_json::to_value(&error).expect("tool errors must serialize");
        assert_eq!(json["structuredContent"]["error"]["code"], "planMismatch");
    }

    #[test]
    fn selections_inside_the_snapshot_pass() {
        let snapshot = json!({ "ruleIds": ["a.b", "c.d"] });

        assert!(ensure_within_snapshot(&["c.d".to_string()], &snapshot, "ruleIds", "rule").is_ok());
    }

    #[test]
    fn a_snapshot_without_the_expected_key_allows_nothing() {
        let snapshot = json!({});

        assert!(
            ensure_within_snapshot(&["a.b".to_string()], &snapshot, "ruleIds", "rule").is_err()
        );
    }

    #[test]
    fn pids_outside_the_snapshot_are_rejected() {
        let snapshot = json!({ "processes": [{ "pid": 42, "startedAtMs": 7 }] });

        let error = ensure_pids_within_snapshot(&[43], &snapshot)
            .expect_err("a pid outside the preview must be rejected");

        let json = serde_json::to_value(&error).expect("tool errors must serialize");
        assert_eq!(json["structuredContent"]["error"]["code"], "planMismatch");
    }

    #[test]
    fn pids_inside_the_snapshot_pass() {
        let snapshot = json!({ "processes": [{ "pid": 42, "startedAtMs": 7 }, { "pid": 7, "startedAtMs": 3 }] });

        assert!(ensure_pids_within_snapshot(&[7, 42], &snapshot).is_ok());
    }

    #[test]
    fn a_snapshot_without_listed_processes_allows_no_pid() {
        let snapshot = json!({});

        assert!(ensure_pids_within_snapshot(&[42], &snapshot).is_err());
    }

    #[test]
    fn empty_pid_lists_are_rejected_before_the_token() {
        let error = ensure_pids_present(&[]).expect_err("an empty pid list must be rejected");

        let json = serde_json::to_value(&error).expect("tool errors must serialize");
        assert_eq!(json["structuredContent"]["error"]["code"], "invalidInput");
        assert!(ensure_pids_present(&[42]).is_ok());
    }

    #[test]
    fn process_end_input_rejects_an_unknown_mode() {
        let parsed = serde_json::from_value::<ProcessEndInput>(json!({
            "token": "mdx_test",
            "confirm": true,
            "dryRun": false,
            "pids": [42],
            "mode": "obliterate",
        }));

        assert!(
            parsed.is_err(),
            "an unknown mode must fail schema validation"
        );
    }

    #[test]
    fn process_end_input_defaults_to_graceful_and_maps_to_the_platform_mode() {
        let parsed = serde_json::from_value::<ProcessEndInput>(json!({
            "token": "mdx_test",
            "confirm": true,
            "dryRun": false,
            "pids": [42],
        }))
        .expect("the minimal guarded input must deserialize");

        assert_eq!(parsed.pids, vec![42]);
        assert!(matches!(parsed.mode, ProcessEndModeInput::Graceful));
        assert_eq!(ProcessEndMode::from(parsed.mode), ProcessEndMode::Graceful);
        assert_eq!(
            ProcessEndMode::from(ProcessEndModeInput::Force),
            ProcessEndMode::Force
        );
    }

    #[test]
    fn processes_scan_input_rejects_an_unknown_top_by() {
        let parsed = serde_json::from_value::<ProcessesScanInput>(json!({
            "topBy": "threads",
        }));

        assert!(
            parsed.is_err(),
            "an unknown topBy metric must fail schema validation"
        );
    }
}
