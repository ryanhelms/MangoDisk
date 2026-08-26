mod directories;
mod processes;
mod volumes;

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use crate::{
    ApplicationDirectories, Platform, PlatformCancellation, PlatformError, PlatformErrorCode,
    PlatformResult, PlatformStartupChangeRequest, PlatformStartupChangeResult,
    PlatformStartupCoverageReason, PlatformStartupSourceResult, PlatformSystemSettingChangeRequest,
    PlatformSystemSettingChangeResult, PlatformSystemSettingState, ScanPurpose, SkipReason,
    StartupPlatform, SystemInventory, SystemSettingsPlatform, UserDirectories, VolumeInfo,
};

pub struct LinuxPlatform;

pub(crate) fn application_directories(identifier: &str) -> PlatformResult<ApplicationDirectories> {
    directories::application_directories(identifier)
}

impl Platform for LinuxPlatform {
    fn os_name(&self) -> &'static str {
        "linux"
    }

    fn system_volume_path(&self) -> PathBuf {
        PathBuf::from("/")
    }

    fn system_volume(&self) -> PlatformResult<VolumeInfo> {
        volumes::system_volume()
    }

    fn volumes(&self) -> PlatformResult<Vec<VolumeInfo>> {
        volumes::volumes()
    }

    fn user_directories(&self) -> PlatformResult<UserDirectories> {
        directories::user_directories()
    }

    fn system_inventory_revision(&self) -> PlatformResult<String> {
        Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "Linux application inventory revision capture is not implemented",
        ))
    }

    fn system_inventory(&self) -> PlatformResult<SystemInventory> {
        Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "Linux application inventory is not implemented",
        ))
    }

    fn running_process_names(&self) -> PlatformResult<Vec<String>> {
        processes::running_process_names()
    }

    fn is_link_like(&self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_symlink()
    }

    fn is_same_filesystem(&self, root: &fs::Metadata, candidate: &fs::Metadata) -> bool {
        root.dev() == candidate.dev()
    }

    fn should_skip(
        &self,
        path: &Path,
        scan_root: &Path,
        purpose: ScanPurpose,
    ) -> Option<SkipReason> {
        if purpose != ScanPurpose::Cleanup
            && directories::is_non_cleanup_skipped(path, scan_root, purpose)
        {
            return Some(SkipReason::SystemCritical);
        }
        None
    }

    fn validate_cleanup_root(&self, path: &Path) -> PlatformResult<()> {
        let canonical = self.canonicalize_no_links(path)?;
        if canonical.parent().is_none() || volumes::is_mount_point(&canonical)? {
            return Err(PlatformError::invalid_path(
                "cleanup of a volume root is forbidden",
            ));
        }
        if directories::is_protected_cleanup_path(&canonical) {
            return Err(PlatformError::invalid_path(
                "cleanup of a protected Linux directory is forbidden",
            ));
        }

        let home = fs::canonicalize(directories::home_directory()?)
            .map_err(|error| PlatformError::io("canonicalize Linux user home", &error))?;
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
        ] {
            if canonical.starts_with(home.join(name)) {
                return Err(PlatformError::invalid_path(
                    "cleanup of a personal data directory is forbidden",
                ));
            }
        }
        Ok(())
    }
}

impl StartupPlatform for LinuxPlatform {
    fn scan_startup_sources(
        &self,
        _cancellation: &PlatformCancellation,
    ) -> PlatformResult<Vec<PlatformStartupSourceResult>> {
        Ok(vec![PlatformStartupSourceResult::unavailable(
            "linux.startup",
            true,
            PlatformStartupCoverageReason::NotImplemented,
        )])
    }

    fn change_startup_item(
        &self,
        _request: &PlatformStartupChangeRequest,
        _authorization_prompt: Option<&str>,
    ) -> PlatformResult<PlatformStartupChangeResult> {
        Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "Linux startup item changes are not implemented",
        ))
    }
}

impl SystemSettingsPlatform for LinuxPlatform {
    fn scan_system_settings(
        &self,
        _setting_ids: &[&str],
        _cancellation: &PlatformCancellation,
    ) -> PlatformResult<Vec<PlatformSystemSettingState>> {
        Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "Linux system settings inspection is not implemented",
        ))
    }

    fn change_system_setting(
        &self,
        _request: &PlatformSystemSettingChangeRequest,
    ) -> PlatformResult<PlatformSystemSettingChangeResult> {
        Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "Linux system setting changes are not implemented",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_linux_capabilities_return_typed_errors() {
        let platform = LinuxPlatform;
        assert_eq!(
            platform
                .system_inventory()
                .expect_err("Linux inventory should remain unavailable")
                .code(),
            PlatformErrorCode::Unsupported
        );
        assert_eq!(
            platform
                .system_inventory_revision()
                .expect_err("Linux inventory revision should remain unavailable")
                .code(),
            PlatformErrorCode::Unsupported
        );
    }

    #[test]
    fn startup_scan_reports_the_unimplemented_source() {
        let platform = LinuxPlatform;
        let cancellation = PlatformCancellation::new(|| false);

        let sources = platform
            .scan_startup_sources(&cancellation)
            .expect("the Linux startup coverage result should be available");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "linux.startup");
        assert_eq!(
            sources[0].reason,
            Some(PlatformStartupCoverageReason::NotImplemented)
        );
    }
}
