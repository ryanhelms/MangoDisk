use std::{
    ffi::{CStr, OsStr},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

use crate::{
    PlatformError, PlatformResult, ProcessEndMode, ProcessEndStatus, ProcessMetric,
    ProcessMetricAbsence, ProcessMetricsSnapshot, ProcessState,
};

// `PROC_LIST_ALLPIDS` is a stable XNU selector from <sys/proc_info.h> that the
// libc crate does not export.
const PROC_LIST_ALLPIDS: i32 = 1;
// `proc_taskinfo` reports CPU totals in nanoseconds; Core converts ticks to
// seconds through `cpu_ticks_per_second`.
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

pub(super) fn snapshot_processes() -> PlatformResult<Vec<ProcessMetricsSnapshot>> {
    let pids = list_all_pids()?;
    let current_uid = unsafe { libc::getuid() };
    let mut processes = Vec::with_capacity(pids.len());
    for pid in pids {
        // Processes can exit between the PID listing and the per-process
        // queries. Skipping them preserves a coherent best-effort snapshot.
        if let Some(process) = read_process(pid, current_uid) {
            processes.push(process);
        }
    }
    Ok(processes)
}

pub(super) fn end_process(pid: u32, mode: ProcessEndMode) -> PlatformResult<ProcessEndStatus> {
    if pid == 0 || pid == std::process::id() {
        return Err(PlatformError::new(
            crate::PlatformErrorCode::AccessDenied,
            "refusing to end kernel_task or MangoDisk itself",
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
        _ => Err(PlatformError::io("end macOS process", &error)),
    }
}

fn list_all_pids() -> PlatformResult<Vec<i32>> {
    let byte_count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if byte_count <= 0 {
        return Err(PlatformError::operation_failed(
            "macOS process listing is unavailable",
        ));
    }
    let pid_count = byte_count as usize / std::mem::size_of::<libc::pid_t>();
    // Extra capacity absorbs processes spawned between the size probe and the
    // actual listing; a second growth attempt is unnecessary.
    let mut pids = vec![0 as libc::pid_t; pid_count + 64];
    let listed = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as i32,
        )
    };
    if listed <= 0 {
        return Err(PlatformError::operation_failed(
            "macOS process listing changed while being captured",
        ));
    }
    pids.truncate(listed as usize / std::mem::size_of::<libc::pid_t>());
    Ok(pids)
}

fn read_process(pid: i32, current_uid: u32) -> Option<ProcessMetricsSnapshot> {
    let info = task_all_info(pid)?;
    let bsd = info.pbsd;
    let task = info.ptinfo;
    let is_kernel_task = bsd.pbi_pid == 0;
    let name = c_string(&bsd.pbi_name).or_else(|| c_string(&bsd.pbi_comm))?;
    let owner_name = match user_name_for_uid(bsd.pbi_uid) {
        Some(name) => ProcessMetric::present(name),
        None => ProcessMetric::absent(ProcessMetricAbsence::NotAvailable),
    };
    Some(ProcessMetricsSnapshot {
        pid: bsd.pbi_pid,
        ppid: bsd.pbi_ppid,
        name,
        executable_path: executable_path(pid, is_kernel_task),
        owner_uid: ProcessMetric::present(bsd.pbi_uid),
        owner_name,
        owned_by_current_user: ProcessMetric::present(bsd.pbi_uid == current_uid),
        state: map_bsd_state(bsd.pbi_status),
        thread_count: u32::try_from(task.pti_threadnum).unwrap_or(u32::MAX),
        cpu_user_ticks: task.pti_total_user,
        cpu_kernel_ticks: task.pti_total_system,
        cpu_ticks_per_second: NANOSECONDS_PER_SECOND,
        rss_bytes: task.pti_resident_size,
        io_read_bytes: io_counter(pid, is_kernel_task, true),
        io_write_bytes: io_counter(pid, is_kernel_task, false),
        open_file_count: open_file_count(pid, is_kernel_task),
        started_at_ms: bsd
            .pbi_start_tvsec
            .saturating_mul(1000)
            .saturating_add(bsd.pbi_start_tvusec / 1000),
    })
}

fn task_all_info(pid: i32) -> Option<libc::proc_taskallinfo> {
    let mut info: libc::proc_taskallinfo = unsafe { std::mem::zeroed() };
    let expected = std::mem::size_of::<libc::proc_taskallinfo>() as i32;
    let size = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKALLINFO,
            0,
            (&mut info as *mut libc::proc_taskallinfo).cast(),
            expected,
        )
    };
    (size == expected).then_some(info)
}

fn executable_path(pid: i32, is_kernel_task: bool) -> ProcessMetric<PathBuf> {
    if is_kernel_task {
        // kernel_task has no userspace image path.
        return ProcessMetric::absent(ProcessMetricAbsence::NotApplicable);
    }
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            libc::PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    if length <= 0 {
        // SIP-protected or already-exited processes deny the path lookup.
        return ProcessMetric::absent(ProcessMetricAbsence::AccessDenied);
    }
    buffer.truncate(length as usize);
    ProcessMetric::present(PathBuf::from(OsStr::from_bytes(&buffer)))
}

fn io_counter(pid: i32, is_kernel_task: bool, read: bool) -> ProcessMetric<u64> {
    if is_kernel_task {
        return ProcessMetric::absent(ProcessMetricAbsence::NotApplicable);
    }
    let mut usage: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let status = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut usage as *mut libc::rusage_info_v2).cast(),
        )
    };
    if status != 0 {
        let absence = match status {
            libc::EPERM => ProcessMetricAbsence::AccessDenied,
            _ => ProcessMetricAbsence::NotAvailable,
        };
        return ProcessMetric::absent(absence);
    }
    ProcessMetric::present(if read {
        usage.ri_diskio_bytesread
    } else {
        usage.ri_diskio_byteswritten
    })
}

fn open_file_count(pid: i32, is_kernel_task: bool) -> ProcessMetric<u32> {
    if is_kernel_task {
        return ProcessMetric::absent(ProcessMetricAbsence::NotApplicable);
    }
    // `PROC_PIDLISTFDS` with an empty buffer returns the byte size needed to
    // list every descriptor, which is already an exact count without copying
    // potentially tens of thousands of entries into user space.
    let byte_count =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if byte_count <= 0 {
        return ProcessMetric::absent(ProcessMetricAbsence::AccessDenied);
    }
    let count = byte_count as usize / std::mem::size_of::<libc::proc_fdinfo>();
    ProcessMetric::present(u32::try_from(count).unwrap_or(u32::MAX))
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
    let name = unsafe { CStr::from_ptr(account.pw_name) };
    if name.to_bytes().is_empty() {
        return None;
    }
    Some(name.to_string_lossy().into_owned())
}

fn c_string(buffer: &[libc::c_char]) -> Option<String> {
    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), end) };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn map_bsd_state(status: u32) -> ProcessState {
    match status {
        libc::SIDL => ProcessState::Idle,
        libc::SRUN => ProcessState::Running,
        libc::SSLEEP => ProcessState::Sleeping,
        libc::SSTOP => ProcessState::Stopped,
        libc::SZOMB => ProcessState::Zombie,
        _ => ProcessState::Unknown,
    }
}
