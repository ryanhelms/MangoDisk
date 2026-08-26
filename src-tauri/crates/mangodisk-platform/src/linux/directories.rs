use std::{
    env,
    ffi::OsString,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

use crate::{ApplicationDirectories, PlatformError, PlatformResult, ScanPurpose, UserDirectories};

const SYSTEM_CRITICAL_DIRECTORIES: [&str; 6] =
    ["/proc", "/sys", "/dev", "/run", "/var/run", "/var/lock"];
const TRANSIENT_DIRECTORIES: [&str; 2] = ["/tmp", "/var/tmp"];
/// Removable volumes mounted through udisks2 live under `/run/media/<user>`.
const REMOVABLE_MEDIA_ROOT: &str = "/run/media";
const PROTECTED_CLEANUP_DIRECTORIES: [&str; 15] = [
    "/usr",
    "/etc",
    "/bin",
    "/sbin",
    "/boot",
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/run",
    "/var/lock",
    "/var/lib",
    "/opt",
    "/snap",
    "/root",
];

pub(super) fn application_directories(identifier: &str) -> PlatformResult<ApplicationDirectories> {
    let home = home_directory()?;
    Ok(ApplicationDirectories {
        local_data_directory: xdg_directory("XDG_DATA_HOME", &home, ".local/share")
            .join(identifier),
        cache_directory: xdg_directory("XDG_CACHE_HOME", &home, ".cache").join(identifier),
    })
}

pub(super) fn user_directories() -> PlatformResult<UserDirectories> {
    let home = home_directory()?;
    let cache = xdg_directory("XDG_CACHE_HOME", &home, ".cache");
    let config = xdg_directory("XDG_CONFIG_HOME", &home, ".config");
    let data = xdg_directory("XDG_DATA_HOME", &home, ".local/share");
    let temporary = xdg_directory("TMPDIR", Path::new("/"), "tmp");
    Ok(UserDirectories::new(home, temporary, cache, [config, data]))
}

pub(super) fn home_directory() -> PlatformResult<PathBuf> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| PlatformError::invalid_path("Linux user home is unavailable"))?;
    if !home.is_absolute() {
        return Err(PlatformError::invalid_path(
            "Linux user home is not absolute",
        ));
    }
    Ok(home)
}

fn xdg_directory(variable: &str, home: &Path, fallback: &str) -> PathBuf {
    resolve_xdg_directory(home, env::var_os(variable), fallback)
}

fn resolve_xdg_directory(home: &Path, configured: Option<OsString>, fallback: &str) -> PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}

pub(super) fn is_non_cleanup_skipped(path: &Path, scan_root: &Path, purpose: ScanPurpose) -> bool {
    // udisks2 mounts removable media at `/run/media/<user>` on major desktop
    // distributions. Those subtrees hold ordinary user data, so the `/run`
    // system boundary must not blank them from scans or permanent deletion.
    if is_within_any(path, &SYSTEM_CRITICAL_DIRECTORIES) && !path.starts_with(REMOVABLE_MEDIA_ROOT)
    {
        return true;
    }
    // Transient directories are pruned only from duplicate scans rooted
    // elsewhere, matching the macOS policy: hashing throwaway copies wastes
    // time, while analysis and explicit user selections must stay intact.
    // Permanent deletion reuses this predicate with the volume root as the
    // scan scope, so pruning `/tmp` here would make its contents undeletable.
    purpose == ScanPurpose::DuplicateFiles
        && is_within_any(path, &TRANSIENT_DIRECTORIES)
        && !is_within_any(scan_root, &TRANSIENT_DIRECTORIES)
}

pub(super) fn is_protected_cleanup_path(path: &Path) -> bool {
    is_within_any(path, &PROTECTED_CLEANUP_DIRECTORIES) || is_root_library_path(path)
}

fn is_within_any(path: &Path, roots: &[&str]) -> bool {
    roots.iter().any(|root| path.starts_with(*root))
}

fn is_root_library_path(path: &Path) -> bool {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return false;
    }
    matches!(
        components.next(),
        Some(Component::Normal(name)) if name.as_bytes().starts_with(b"lib")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_paths_require_absolute_configuration() {
        let home = Path::new("/home/example");

        assert_eq!(
            resolve_xdg_directory(home, Some(OsString::from("/srv/example/cache")), ".cache"),
            PathBuf::from("/srv/example/cache")
        );
        assert_eq!(
            resolve_xdg_directory(home, Some(OsString::from("relative")), ".cache"),
            PathBuf::from("/home/example/.cache")
        );
        assert_eq!(
            resolve_xdg_directory(home, None, ".local/share"),
            PathBuf::from("/home/example/.local/share")
        );
    }

    #[test]
    fn system_directory_boundaries_do_not_match_similar_prefixes() {
        assert!(is_non_cleanup_skipped(
            Path::new("/proc/1/status"),
            Path::new("/"),
            ScanPurpose::Analysis
        ));
        assert!(!is_non_cleanup_skipped(
            Path::new("/process-data/status"),
            Path::new("/"),
            ScanPurpose::Analysis
        ));
        assert!(is_protected_cleanup_path(Path::new("/lib64/ld-linux.so")));
        assert!(!is_protected_cleanup_path(Path::new(
            "/home/example/library/report"
        )));
    }

    #[test]
    fn transient_directories_are_pruned_only_from_foreign_duplicate_scans() {
        assert!(is_non_cleanup_skipped(
            Path::new("/tmp/generated/file"),
            Path::new("/"),
            ScanPurpose::DuplicateFiles
        ));
        assert!(!is_non_cleanup_skipped(
            Path::new("/tmp/generated/file"),
            Path::new("/tmp/generated"),
            ScanPurpose::DuplicateFiles
        ));
        assert!(!is_non_cleanup_skipped(
            Path::new("/tmp/generated/file"),
            Path::new("/"),
            ScanPurpose::Analysis
        ));
    }
}
