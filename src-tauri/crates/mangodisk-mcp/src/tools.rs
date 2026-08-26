use std::collections::HashSet;

use mangodisk_core::{
    AnalysisService, ApplicationLeftoverService, ApplicationUninstallBatchSelection,
    ApplicationUninstallService, CleanupRequest, CleanupScanResult, CleanupScanService,
    CleanupService, DuplicateFileService, DuplicateFilesResult, HistoryService, LargeFileService,
    LargeFilesResult, OperationCancellationToken, PermanentDeleteCandidate, StartupDesiredState,
    StartupService, SystemSettingTargetState, SystemSettingsChangeSelection, SystemSettingsService,
};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData},
    service::RequestContext,
    tool, tool_router, RoleServer,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    core_runner::CoreOperation,
    errors,
    execution_tokens::MutationDomain,
    server::{AdapterState, MangoDiskServer},
};

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
}
