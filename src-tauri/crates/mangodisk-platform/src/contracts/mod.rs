mod applications;
mod directory_aggregate;
mod disk_cleanup;
mod error;
mod platform;
mod process_metrics;
mod processes;
mod scan;
mod startup;
mod system_settings;
mod volumes;

pub use applications::{
    ApplicationComponentAggregate, ApplicationComponentAggregateError, ApplicationInstallScope,
    ApplicationInventorySource, ApplicationSourceIdentity, ApplicationUninstallExecutionOutcome,
    ApplicationUninstallPlatformError, ApplicationUninstallRegistration,
    ApplicationUninstallRegistrationState, DetectedTool, InstalledApplication,
    MacosPrivilegedApplicationRemovalOutcome, SystemInventory, WindowsRegisteredUninstallKind,
    WindowsRegistryView,
};
#[cfg(all(test, any(windows, target_os = "macos")))]
pub(crate) use directory_aggregate::reference_directory_tree_aggregate;
#[cfg(any(windows, target_os = "macos"))]
pub(crate) use directory_aggregate::DirectoryAggregateProgress;
pub use directory_aggregate::{
    DirectPhysicalDirectoryEnumeration, DirectoryTreeAggregate, DirectoryTreeAggregateError,
    DirectoryTreeSourceAggregate,
};
pub use disk_cleanup::{
    PlatformCancellation, WindowsDiskCleanupAvailability, WindowsDiskCleanupEstimate,
    WindowsDiskCleanupExecution, WindowsDiskCleanupExecutionStatus, WindowsDiskCleanupKind,
};
pub use error::{PlatformError, PlatformErrorCode, PlatformMutationState, PlatformResult};
pub use platform::Platform;
pub use process_metrics::{
    ProcessEndMode, ProcessEndStatus, ProcessMetric, ProcessMetricAbsence, ProcessMetricsSnapshot,
    ProcessState,
};
pub use processes::{
    ApplicationProcessCloseMode, ApplicationProcessCloseResult, ApplicationProcessTarget,
    RunningProcessIdentity,
};
#[cfg(any(windows, target_os = "macos"))]
pub(crate) use scan::FilesystemChangeMonitorBackend;
pub use scan::{
    DirectoryEntryIdentities, FastAnalysisQuery, FastAnalysisRecord, FastAnalysisScanError,
    FastAnalysisSummary, FilesystemChangeImpactError, FilesystemChangeImpactOutcome,
    FilesystemChangeImpactPlan, FilesystemChangeImpactSummary, FilesystemChangeImpactUnavailable,
    FilesystemChangeMonitor, FilesystemChangeStatus, FilesystemChangeToken,
    LargeFileCandidateScanError, LargeFileCandidateSummary, PhysicalFileIdentity,
    ProjectMarkerCandidateProgress, ProjectMarkerCandidateQuery, ProjectMarkerCandidateScanError,
    ProjectMarkerCandidateSummary, ScanPurpose, SkipReason,
};
pub use startup::{
    PlatformStartupArtifact, PlatformStartupChangeRequest, PlatformStartupChangeResult,
    PlatformStartupConfiguredState, PlatformStartupControlCapability,
    PlatformStartupCoverageReason, PlatformStartupCoverageStatus, PlatformStartupDesiredState,
    PlatformStartupDiagnosticCode, PlatformStartupIdentityConfidence, PlatformStartupOwner,
    PlatformStartupRuntimeState, PlatformStartupScope, PlatformStartupSourceKind,
    PlatformStartupSourceResult, PlatformStartupSummarySource, PlatformStartupTarget,
    PlatformStartupTargetKind, PlatformStartupTrigger, PlatformStartupTrustState, StartupPlatform,
};
#[cfg(target_os = "macos")]
pub(crate) use system_settings::preflight_system_setting_change;
pub use system_settings::{
    PlatformSystemSettingChangeRequest, PlatformSystemSettingChangeResult,
    PlatformSystemSettingDiagnosticCode, PlatformSystemSettingSnapshot, PlatformSystemSettingState,
    PlatformSystemSettingValue, SystemSettingsPlatform,
};
pub use volumes::{
    ApplicationDirectories, ScanConcurrency, ScanDeviceClass, UserDirectories, VolumeInfo,
};
