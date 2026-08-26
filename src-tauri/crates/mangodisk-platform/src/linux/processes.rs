use std::{fs, os::unix::ffi::OsStrExt, path::Path};

use crate::{PlatformError, PlatformResult};

pub(super) fn running_process_names() -> PlatformResult<Vec<String>> {
    let entries = fs::read_dir("/proc")
        .map_err(|error| PlatformError::io("read Linux process table", &error))?;
    let mut names = Vec::new();
    for entry in entries.flatten() {
        if !is_process_directory(&entry.file_name()) {
            continue;
        }
        // Processes can exit between directory enumeration and reading `comm`.
        // Skipping those entries preserves a coherent best-effort snapshot.
        let Ok(bytes) = fs::read(entry.path().join("comm")) else {
            continue;
        };
        let name = String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if !name.is_empty() {
            names.push(complete_process_name(&entry.path(), name));
        }
    }
    Ok(names)
}

fn is_process_directory(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty() && bytes.iter().all(|byte| byte.is_ascii_digit())
}

/// The kernel truncates `comm` to TASK_COMM_LEN - 1 (15) bytes, which would
/// make a longer executable name (for example `chromium-browser`) invisible to
/// exact process-name matching. When the comm value fills that budget, prefer
/// the full executable name from the `exe` link, but only when it extends the
/// truncated prefix; an unreadable or unrelated link keeps the comm value.
fn complete_process_name(process_path: &Path, comm_name: String) -> String {
    if comm_name.len() < 15 {
        return comm_name;
    }
    fs::read_link(process_path.join("exe"))
        .ok()
        .and_then(|target| {
            target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|executable| executable.starts_with(comm_name.as_str()))
        .unwrap_or(comm_name)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{complete_process_name, is_process_directory};

    #[test]
    fn process_directories_require_numeric_names() {
        assert!(is_process_directory(OsStr::new("42")));
        assert!(!is_process_directory(OsStr::new("self")));
        assert!(!is_process_directory(OsStr::new("42-task")));
        assert!(!is_process_directory(OsStr::new("")));
    }

    #[test]
    fn truncated_comm_extends_only_to_a_matching_executable_name() {
        let directory = std::env::temp_dir().join(format!("mangodisk-comm-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the comm fixture directory should be created");
        std::fs::write(directory.join("chromium-browser"), [])
            .expect("the executable fixture should be created");
        std::os::unix::fs::symlink(directory.join("chromium-browser"), directory.join("exe"))
            .expect("the exe link fixture should be created");

        assert_eq!(
            complete_process_name(&directory, "chromium-browse".to_string()),
            "chromium-browser"
        );
        assert_eq!(
            complete_process_name(&directory, "short-name".to_string()),
            "short-name"
        );

        std::fs::remove_dir_all(directory).expect("the comm fixture should be removed");
    }
}
