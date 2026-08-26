use std::{
    collections::HashSet,
    ffi::{CString, OsString},
    fs,
    mem::MaybeUninit,
    os::unix::ffi::{OsStrExt, OsStringExt},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use crate::{
    PlatformError, PlatformErrorCode, PlatformResult, ScanConcurrency, ScanDeviceClass, VolumeInfo,
};

const MOUNT_TABLE_PATHS: [&str; 2] = ["/proc/mounts", "/etc/mtab"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountEntry {
    source: PathBuf,
    mount_point: PathBuf,
    filesystem_type: String,
}

pub(super) fn system_volume() -> PlatformResult<VolumeInfo> {
    let mounts = mount_entries()?;
    let root = mounts
        .iter()
        .rev()
        .find(|entry| entry.mount_point.as_path() == Path::new("/"))
        .ok_or_else(|| {
            PlatformError::new(
                PlatformErrorCode::InvalidData,
                "Linux mount table does not contain the system volume",
            )
        })?;
    volume_info(root)
}

pub(super) fn volumes() -> PlatformResult<Vec<VolumeInfo>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for mount in mount_entries()? {
        if is_pseudo_filesystem(&mount.filesystem_type)
            || !seen.insert(mount.mount_point.clone())
            || !fs::metadata(&mount.mount_point).is_ok_and(|metadata| metadata.is_dir())
        {
            continue;
        }
        // Mounts may disappear after the kernel snapshot is read. A vanished
        // secondary volume must not hide the remaining usable volumes.
        if let Ok(volume) = volume_info(&mount) {
            result.push(volume);
        }
    }
    result.sort_by(|left, right| left.mount_point.cmp(&right.mount_point));
    Ok(result)
}

pub(super) fn is_mount_point(path: &Path) -> PlatformResult<bool> {
    let Some(parent) = path.parent() else {
        return Ok(true);
    };
    let metadata = fs::metadata(path)
        .map_err(|error| PlatformError::io("inspect Linux cleanup root", &error))?;
    let parent_metadata = fs::metadata(parent)
        .map_err(|error| PlatformError::io("inspect Linux cleanup root parent", &error))?;
    if metadata.dev() != parent_metadata.dev() {
        return Ok(true);
    }
    // Bind mounts can share their parent's device identity, so the kernel
    // mount table remains necessary even after the metadata comparison.
    mount_entries().map(|mounts| {
        mounts
            .iter()
            .any(|entry| entry.mount_point.as_path() == path)
    })
}

fn mount_entries() -> PlatformResult<Vec<MountEntry>> {
    let mut errors = Vec::with_capacity(MOUNT_TABLE_PATHS.len());
    for path in MOUNT_TABLE_PATHS {
        match fs::read(path) {
            Ok(contents) => match parse_mount_table(&contents) {
                Ok(mounts) => return Ok(mounts),
                Err(error) => errors.push(error),
            },
            Err(error) => {
                errors.push(PlatformError::io("read Linux mount table", &error));
            }
        }
    }
    Err(errors.pop().unwrap_or_else(|| {
        PlatformError::new(
            PlatformErrorCode::InvalidData,
            "Linux mount table is unavailable",
        )
    }))
}

fn parse_mount_table(contents: &[u8]) -> PlatformResult<Vec<MountEntry>> {
    let mut mounts = Vec::new();
    for line in contents.split(|byte| *byte == b'\n') {
        let fields = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        if fields.is_empty() || fields[0].starts_with(b"#") {
            continue;
        }
        if fields.len() < 3 {
            return Err(PlatformError::new(
                PlatformErrorCode::InvalidData,
                "Linux mount table contains an incomplete record",
            ));
        }
        let filesystem_type = std::str::from_utf8(fields[2]).map_err(|_| {
            PlatformError::new(
                PlatformErrorCode::InvalidData,
                "Linux mount table contains an invalid filesystem type",
            )
        })?;
        mounts.push(MountEntry {
            source: PathBuf::from(decode_mount_field(fields[0])),
            mount_point: PathBuf::from(decode_mount_field(fields[1])),
            filesystem_type: filesystem_type.to_string(),
        });
    }
    if mounts.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::InvalidData,
            "Linux mount table contains no records",
        ));
    }
    Ok(mounts)
}

fn decode_mount_field(encoded: &[u8]) -> OsString {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'\\'
            && index + 3 < encoded.len()
            && matches!(encoded[index + 1], b'0'..=b'3')
            && encoded[index + 2..=index + 3]
                .iter()
                .all(|byte| matches!(*byte, b'0'..=b'7'))
        {
            let value = (encoded[index + 1] - b'0') * 64
                + (encoded[index + 2] - b'0') * 8
                + (encoded[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    OsString::from_vec(decoded)
}

fn volume_info(mount: &MountEntry) -> PlatformResult<VolumeInfo> {
    let (total_bytes, available_bytes) = disk_space(&mount.mount_point)?;
    Ok(VolumeInfo {
        name: volume_name(mount),
        mount_point: mount.mount_point.to_string_lossy().into_owned(),
        total_bytes,
        available_bytes,
        used_bytes: total_bytes.saturating_sub(available_bytes),
        scan_concurrency: scan_concurrency(&mount.source, &mount.filesystem_type),
    })
}

fn volume_name(mount: &MountEntry) -> String {
    let mount_name = if mount.mount_point.as_path() == Path::new("/") {
        None
    } else {
        mount.mount_point.file_name()
    };
    mount_name
        .or(mount.source.file_name())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Linux".to_string())
}

fn disk_space(path: &Path) -> PlatformResult<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| PlatformError::invalid_path("Linux volume path contains an invalid byte"))?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // `c_path` is NUL-terminated and `stats` is initialized only after libc reports success.
    if unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(PlatformError::io(
            "inspect Linux volume capacity",
            &std::io::Error::last_os_error(),
        ));
    }
    let stats = unsafe { stats.assume_init() };
    Ok((
        byte_count(stats.f_blocks, stats.f_frsize),
        byte_count(stats.f_bavail, stats.f_frsize),
    ))
}

fn byte_count(blocks: impl Into<u128>, block_size: impl Into<u128>) -> u64 {
    let blocks = blocks.into();
    let block_size = block_size.into();
    blocks.saturating_mul(block_size).min(u128::from(u64::MAX)) as u64
}

fn scan_concurrency(source: &Path, filesystem_type: &str) -> ScanConcurrency {
    if is_network_filesystem(filesystem_type) {
        return ScanConcurrency::conservative(ScanDeviceClass::Network);
    }
    let Some(block_path) = block_device_sysfs_path(source) else {
        return ScanConcurrency::conservative(ScanDeviceClass::Unknown);
    };
    if read_sysfs_flag(&block_path.join("removable")) == Some(true) {
        return ScanConcurrency::conservative(ScanDeviceClass::Removable);
    }
    match read_sysfs_flag(&block_path.join("queue/rotational")) {
        Some(false) => ScanConcurrency::solid_state(),
        Some(true) => ScanConcurrency::rotational(),
        None => ScanConcurrency::conservative(ScanDeviceClass::Unknown),
    }
}

fn block_device_sysfs_path(source: &Path) -> Option<PathBuf> {
    let source = fs::canonicalize(source).ok()?;
    if !source.starts_with("/dev") {
        return None;
    }
    let name = source.file_name()?;
    if name.as_bytes().starts_with(b"loop")
        || name.as_bytes().starts_with(b"ram")
        || name.as_bytes().starts_with(b"zram")
    {
        return None;
    }
    let path = fs::canonicalize(Path::new("/sys/class/block").join(name)).ok()?;
    if path.join("partition").is_file() {
        path.parent().map(Path::to_path_buf)
    } else {
        Some(path)
    }
}

fn read_sysfs_flag(path: &Path) -> Option<bool> {
    match fs::read_to_string(path).ok()?.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn is_network_filesystem(filesystem_type: &str) -> bool {
    matches!(
        filesystem_type,
        "9p" | "afs"
            | "ceph"
            | "cifs"
            | "davfs"
            | "fuse.sshfs"
            | "glusterfs"
            | "nfs"
            | "nfs4"
            | "smb3"
            | "smbfs"
    )
}

fn is_pseudo_filesystem(filesystem_type: &str) -> bool {
    filesystem_type.starts_with("cgroup")
        || matches!(
            filesystem_type,
            "autofs"
                | "binfmt_misc"
                | "bpf"
                | "configfs"
                | "debugfs"
                | "devpts"
                | "devtmpfs"
                | "efivarfs"
                | "fuse.portal"
                | "fusectl"
                | "hugetlbfs"
                | "mqueue"
                | "nsfs"
                | "overlay"
                | "proc"
                | "pstore"
                | "ramfs"
                | "rpc_pipefs"
                | "rootfs"
                | "securityfs"
                | "selinuxfs"
                | "squashfs"
                | "sysfs"
                | "tmpfs"
                | "tracefs"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_table_parser_decodes_kernel_path_escapes() {
        let mounts = parse_mount_table(
            b"/dev/sda2 / ext4 rw 0 0\n/dev/sdb1 /media/My\\040Disk ext4 rw 0 0\n",
        )
        .expect("the fixture mount table should parse");

        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[1].mount_point, PathBuf::from("/media/My Disk"));
    }

    #[test]
    fn pseudo_filesystems_are_excluded_from_volume_discovery() {
        for filesystem_type in ["proc", "sysfs", "tmpfs", "cgroup2", "overlay", "squashfs"] {
            assert!(is_pseudo_filesystem(filesystem_type), "{filesystem_type}");
        }
        assert!(!is_pseudo_filesystem("ext4"));
        assert!(!is_pseudo_filesystem("btrfs"));
    }

    #[test]
    fn network_filesystems_use_conservative_scheduling() {
        let concurrency = scan_concurrency(Path::new("server:/volume"), "nfs4");

        assert_eq!(concurrency.class, ScanDeviceClass::Network);
        assert_eq!(concurrency.worker_limit, 1);
    }
}
