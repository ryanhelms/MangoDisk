use std::{
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use crate::{
    PlatformError, PlatformResult, ProcessEndMode, ProcessEndStatus, ProcessMetric,
    ProcessMetricAbsence, ProcessMetricsSnapshot, ProcessState,
};

// The libc crate does not export the Linux sysconf selectors. glibc and musl
// both assign `_SC_CLK_TCK = 2` as a stable ABI value; the unit test below
// asserts the resolved tick rate stays inside the documented Linux range so an
// incompatible C library fails loudly instead of corrupting CPU rates.
const SC_CLK_TCK: i32 = 2;

/// Per-snapshot facts that are identical for every process entry.
struct SnapshotContext {
    boot_time_seconds: u64,
    clock_ticks_per_second: u64,
    current_uid: u32,
}

pub(super) fn snapshot_processes() -> PlatformResult<Vec<ProcessMetricsSnapshot>> {
    let context = SnapshotContext {
        boot_time_seconds: boot_time_seconds()?,
        clock_ticks_per_second: clock_ticks_per_second()?,
        current_uid: unsafe { libc::getuid() },
    };
    let entries = fs::read_dir("/proc")
        .map_err(|error| PlatformError::io("read Linux process table", &error))?;
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        if !is_process_directory(&entry.file_name()) {
            continue;
        }
        let Ok(pid) = String::from_utf8_lossy(entry.file_name().as_bytes()).parse::<u32>() else {
            continue;
        };
        // Processes can exit between directory enumeration and the per-file
        // reads. Skipping them preserves a coherent best-effort snapshot.
        if let Some(process) = read_process(pid, &entry.path(), &context) {
            processes.push(process);
        }
    }
    Ok(processes)
}

pub(super) fn end_process(pid: u32, mode: ProcessEndMode) -> PlatformResult<ProcessEndStatus> {
    if pid == 0 || pid == std::process::id() {
        return Err(PlatformError::new(
            crate::PlatformErrorCode::AccessDenied,
            "refusing to end the kernel idle task or MangoDisk itself",
        ));
    }
    let Ok(native_pid) = i32::try_from(pid) else {
        return Err(PlatformError::new(
            crate::PlatformErrorCode::InvalidData,
            "process id exceeds pid_t",
        ));
    };
    let signal = match mode {
        ProcessEndMode::Graceful => libc::SIGTERM,
        ProcessEndMode::Force => libc::SIGKILL,
    };
    if unsafe { libc::kill(native_pid, signal) } == 0 {
        return Ok(ProcessEndStatus::Ended);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(ProcessEndStatus::NotFound),
        Some(libc::EPERM) => Ok(ProcessEndStatus::PermissionDenied),
        _ => Err(PlatformError::io("end Linux process", &error)),
    }
}

fn read_process(
    pid: u32,
    process_path: &Path,
    context: &SnapshotContext,
) -> Option<ProcessMetricsSnapshot> {
    let stat = parse_proc_stat(&fs::read_to_string(process_path.join("stat")).ok()?)?;
    // `status` carries the effective uid and VmRSS. When it is unreadable the
    // process has either exited (skip) or the kernel hides it behind a mount
    // such as hidepid (typed absence rather than a fabricated owner).
    let status_text = match fs::read_to_string(process_path.join("status")) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => None,
    };
    let status = status_text
        .as_deref()
        .map(parse_proc_status)
        .unwrap_or_default();
    let owner_uid = match status {
        Some(status) => ProcessMetric::present(status.effective_uid),
        None => ProcessMetric::absent(ProcessMetricAbsence::AccessDenied),
    };
    let owned_by_current_user = match owner_uid.value() {
        Some(uid) => ProcessMetric::present(*uid == context.current_uid),
        None => ProcessMetric::absent(
            owner_uid
                .absence()
                .expect("an absent uid carries a typed absence"),
        ),
    };
    let owner_name = match owner_uid.value() {
        Some(uid) => match user_name_for_uid(*uid) {
            Some(name) => ProcessMetric::present(name),
            None => ProcessMetric::absent(ProcessMetricAbsence::NotAvailable),
        },
        None => ProcessMetric::absent(
            owner_uid
                .absence()
                .expect("an absent uid carries a typed absence"),
        ),
    };
    Some(ProcessMetricsSnapshot {
        pid,
        ppid: stat.ppid,
        name: stat.name,
        executable_path: executable_path(process_path),
        owner_uid,
        owner_name,
        owned_by_current_user,
        state: map_proc_state(stat.state),
        thread_count: stat.thread_count,
        cpu_user_ticks: stat.user_ticks,
        cpu_kernel_ticks: stat.kernel_ticks,
        cpu_ticks_per_second: context.clock_ticks_per_second,
        // Kernel threads report no VmRSS line; zero is their true resident set.
        rss_bytes: status.map_or(0, |status| status.rss_kb.saturating_mul(1024)),
        io_read_bytes: io_counter(process_path, "read_bytes"),
        io_write_bytes: io_counter(process_path, "write_bytes"),
        open_file_count: open_file_count(process_path),
        started_at_ms: context
            .boot_time_seconds
            .saturating_mul(1000)
            .saturating_add(
                stat.start_time_ticks.saturating_mul(1000) / context.clock_ticks_per_second.max(1),
            ),
    })
}

fn executable_path(process_path: &Path) -> ProcessMetric<PathBuf> {
    match fs::read_link(process_path.join("exe")) {
        Ok(path) => ProcessMetric::present(path),
        Err(error) => {
            let absence = match error.kind() {
                // Kernel threads have no executable image link at all.
                std::io::ErrorKind::NotFound => ProcessMetricAbsence::NotApplicable,
                std::io::ErrorKind::PermissionDenied => ProcessMetricAbsence::AccessDenied,
                _ => ProcessMetricAbsence::NotAvailable,
            };
            ProcessMetric::absent(absence)
        }
    }
}

fn io_counter(process_path: &Path, key: &str) -> ProcessMetric<u64> {
    match fs::read_to_string(process_path.join("io")) {
        Ok(text) => match parse_proc_io(&text, key) {
            Some(value) => ProcessMetric::present(value),
            None => ProcessMetric::absent(ProcessMetricAbsence::NotAvailable),
        },
        Err(error) => {
            let absence = match error.kind() {
                std::io::ErrorKind::PermissionDenied => ProcessMetricAbsence::AccessDenied,
                // The kernel was built without task IO accounting, or the
                // process is a kernel thread without an IO record.
                std::io::ErrorKind::NotFound => ProcessMetricAbsence::Unsupported,
                _ => ProcessMetricAbsence::NotAvailable,
            };
            ProcessMetric::absent(absence)
        }
    }
}

fn open_file_count(process_path: &Path) -> ProcessMetric<u32> {
    match fs::read_dir(process_path.join("fd")) {
        Ok(entries) => ProcessMetric::present(entries.flatten().count() as u32),
        Err(error) => {
            let absence = match error.kind() {
                std::io::ErrorKind::PermissionDenied => ProcessMetricAbsence::AccessDenied,
                _ => ProcessMetricAbsence::NotAvailable,
            };
            ProcessMetric::absent(absence)
        }
    }
}

fn user_name_for_uid(uid: u32) -> Option<String> {
    let mut account: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut account,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || account.pw_name.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(account.pw_name) };
    if name.to_bytes().is_empty() {
        return None;
    }
    Some(name.to_string_lossy().into_owned())
}

fn boot_time_seconds() -> PlatformResult<u64> {
    let text = fs::read_to_string("/proc/stat")
        .map_err(|error| PlatformError::io("read Linux boot time", &error))?;
    parse_proc_boot_time(&text)
        .ok_or_else(|| PlatformError::operation_failed("Linux boot time is missing"))
}

fn clock_ticks_per_second() -> PlatformResult<u64> {
    let ticks = unsafe { libc::sysconf(SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| PlatformError::operation_failed("Linux clock tick rate is unavailable"))
}

fn is_process_directory(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty() && bytes.iter().all(|byte| byte.is_ascii_digit())
}

#[derive(Debug, PartialEq, Eq)]
struct ProcStat {
    name: String,
    state: char,
    ppid: u32,
    user_ticks: u64,
    kernel_ticks: u64,
    thread_count: u32,
    start_time_ticks: u64,
}

/// Parses `/proc/<pid>/stat`. The comm field sits in parentheses and may
/// itself contain spaces and parentheses, so the closing delimiter is the
/// LAST `)` in the line; everything after it is a plain space-separated field
/// list starting at field 3 (state).
fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = text[open + 1..close].to_string();
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // Field indexes relative to field 3: state=0, ppid=1, utime=11, stime=12,
    // num_threads=17, starttime=19.
    if fields.len() < 20 || name.is_empty() {
        return None;
    }
    Some(ProcStat {
        name,
        state: fields[0].chars().next()?,
        ppid: fields[1].parse().ok()?,
        user_ticks: fields[11].parse().ok()?,
        kernel_ticks: fields[12].parse().ok()?,
        thread_count: fields[17].parse().ok()?,
        start_time_ticks: fields[19].parse().ok()?,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProcStatus {
    effective_uid: u32,
    rss_kb: u64,
}

/// Parses the effective uid and VmRSS from `/proc/<pid>/status`. The `Uid:`
/// line lists real, effective, saved, and filesystem uids; the effective uid
/// (second value) decides which privileges the process actually holds.
fn parse_proc_status(text: &str) -> Option<ProcStatus> {
    let mut status: Option<ProcStatus> = None;
    for line in text.lines() {
        if let Some(values) = line.strip_prefix("Uid:") {
            let uid = values.split_whitespace().nth(1)?.parse().ok()?;
            status = Some(ProcStatus {
                effective_uid: uid,
                rss_kb: 0,
            });
        } else if let Some(values) = line.strip_prefix("VmRSS:") {
            let rss_kb = values.split_whitespace().next()?.parse().ok()?;
            status = status.map(|status| ProcStatus { rss_kb, ..status });
        }
    }
    status
}

/// Extracts one byte counter such as `read_bytes` from `/proc/<pid>/io`.
fn parse_proc_io(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim() == key {
                return value.trim().parse().ok();
            }
        }
    }
    None
}

fn parse_proc_boot_time(text: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("btime ") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn map_proc_state(state: char) -> ProcessState {
    match state {
        'R' => ProcessState::Running,
        // Both interruptible (S) and uninterruptible (D) waits are grouped;
        // the product only distinguishes activity, not wait channels.
        'S' | 'D' => ProcessState::Sleeping,
        'T' | 't' => ProcessState::Stopped,
        'Z' => ProcessState::Zombie,
        'X' | 'x' => ProcessState::Dead,
        'I' | 'P' => ProcessState::Idle,
        _ => ProcessState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn stat_parser_handles_comm_with_spaces_and_parentheses() {
        let stat = parse_proc_stat(
            "1234 (weird (comm) x) S 1 1234 1234 0 -1 4194304 100 0 0 0 25 5 0 0 20 0 8 0 500 12345678\n",
        )
        .expect("a comm with spaces and parentheses must parse");
        assert_eq!(stat.name, "weird (comm) x");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.user_ticks, 25);
        assert_eq!(stat.kernel_ticks, 5);
        assert_eq!(stat.thread_count, 8);
        assert_eq!(stat.start_time_ticks, 500);
    }

    #[test]
    fn stat_parser_rejects_truncated_field_lists() {
        assert!(parse_proc_stat("1234 (proc) S 1 1234").is_none());
        assert!(parse_proc_stat("garbage").is_none());
        assert!(
            parse_proc_stat("1234 () S 1 1234 1234 0 -1 1 1 0 0 0 1 1 0 0 20 0 1 0 1 1").is_none()
        );
    }

    #[test]
    fn status_parser_reads_effective_uid_and_rss() {
        let status = parse_proc_status("Name:\texample\nUid:\t1000\t0\t0\t0\nVmRSS:\t   2048 kB\n")
            .expect("a complete status fixture must parse");
        assert_eq!(status.effective_uid, 0);
        assert_eq!(status.rss_kb, 2048);
    }

    #[test]
    fn status_parser_defaults_rss_for_kernel_threads() {
        let status = parse_proc_status("Name:\tkthreadd\nUid:\t0\t0\t0\t0\n")
            .expect("a status fixture without VmRSS must parse");
        assert_eq!(status.effective_uid, 0);
        assert_eq!(status.rss_kb, 0);
        assert!(parse_proc_status("Name:\tonly\n").is_none());
    }

    #[test]
    fn io_parser_reads_the_requested_counter_only() {
        let io =
            "rchar: 10\nwchar: 20\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 3\n";
        assert_eq!(parse_proc_io(io, "read_bytes"), Some(4096));
        assert_eq!(parse_proc_io(io, "write_bytes"), Some(8192));
        assert_eq!(parse_proc_io(io, "rchar"), Some(10));
        assert_eq!(parse_proc_io(io, "missing"), None);
    }

    #[test]
    fn boot_time_parser_reads_the_btime_line() {
        assert_eq!(
            parse_proc_boot_time("cpu  1 2 3\nbtime 1700000000\nprocesses 42\n"),
            Some(1_700_000_000)
        );
        assert_eq!(parse_proc_boot_time("cpu  1 2 3\n"), None);
    }

    #[test]
    fn clock_tick_rate_matches_the_documented_linux_range() {
        let ticks = clock_ticks_per_second().expect("the tick rate must resolve");
        // USER_HZ is 100 on every mainstream Linux architecture; allow the
        // full historically documented range so exotic kernels still pass.
        assert!((1..=1000).contains(&ticks), "unexpected tick rate {ticks}");
    }

    #[test]
    fn live_snapshot_contains_this_process() {
        let snapshot = snapshot_processes().expect("the host snapshot must succeed");
        assert!(!snapshot.is_empty());
        let own = snapshot
            .iter()
            .find(|process| process.pid == std::process::id())
            .expect("the snapshot must contain the test process");
        assert!(!own.name.is_empty());
        assert!(own.rss_bytes > 0);
        assert!(own.cpu_ticks_per_second > 0);
        assert!(own.started_at_ms > 0);
        assert_eq!(own.owned_by_current_user.value(), Some(&true));
        assert!(own.executable_path.value().is_some());
    }

    #[test]
    fn disappearing_pid_is_skipped() {
        let context = SnapshotContext {
            boot_time_seconds: 1,
            clock_ticks_per_second: 100,
            current_uid: 0,
        };
        assert!(read_process(u32::MAX - 1, Path::new("/proc"), &context).is_none());
    }

    #[test]
    fn end_process_refuses_self_and_pid_zero() {
        assert!(end_process(std::process::id(), ProcessEndMode::Force).is_err());
        assert!(end_process(0, ProcessEndMode::Force).is_err());
    }

    #[test]
    fn end_process_reports_not_found_for_dead_pids() {
        let mut child = Command::new("true").spawn().expect("true should start");
        child.wait().expect("true should exit");
        assert_eq!(
            end_process(child.id(), ProcessEndMode::Graceful)
                .expect("a dead pid must map to a typed outcome"),
            ProcessEndStatus::NotFound
        );
    }

    #[test]
    fn end_process_terminates_a_spawned_process() {
        let mut child = Command::new("sleep")
            .arg("300")
            .spawn()
            .expect("sleep should start");
        let pid = child.id();
        assert_eq!(
            end_process(pid, ProcessEndMode::Graceful).expect("SIGTERM should be accepted"),
            ProcessEndStatus::Ended
        );
        let status = child.wait().expect("the child should be reaped");
        assert!(!status.success());
        assert_eq!(
            end_process(pid, ProcessEndMode::Force).expect("a reaped pid is gone"),
            ProcessEndStatus::NotFound
        );
    }
}
