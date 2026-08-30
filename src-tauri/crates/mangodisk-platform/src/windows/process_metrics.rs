use std::{
    ffi::OsString,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER,
        ERROR_INVALID_PARAMETER, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM,
    },
    Security::{
        EqualSid, GetLengthSid, GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_QUERY,
        TOKEN_USER,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, GetProcessIoCounters, GetProcessTimes,
            OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, TerminateProcess,
            IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        },
    },
    UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
};

use crate::{
    PlatformError, PlatformResult, ProcessEndMode, ProcessEndStatus, ProcessMetric,
    ProcessMetricAbsence, ProcessMetricsSnapshot, ProcessState,
};

// Windows file times count 100 ns intervals since 1601-01-01; Core converts
// ticks to seconds through `cpu_ticks_per_second`.
const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;
const FILETIME_UNIX_EPOCH_OFFSET: u64 = 116_444_736_000_000_000;
const MAX_EXECUTABLE_PATH_UNITS: usize = 32_768;

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Per-snapshot facts shared by every process entry. The current-user SID is
/// resolved once per process because token queries are the most expensive
/// part of the snapshot.
struct SnapshotContext {
    current_user_sid: Option<Vec<u8>>,
}

pub(super) fn snapshot_processes() -> PlatformResult<Vec<ProcessMetricsSnapshot>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let snapshot = OwnedHandle::new(snapshot).ok_or_else(|| {
        PlatformError::operation_failed("windows process metrics snapshot creation failed")
    })?;
    let context = SnapshotContext {
        current_user_sid: current_user_sid(),
    };
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return Err(PlatformError::operation_failed(
            "windows process metrics enumeration failed",
        ));
    }

    let mut processes = Vec::new();
    loop {
        if let Some(process) = read_process(&entry, &context) {
            processes.push(process);
        }
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            break;
        }
    }
    Ok(processes)
}

pub(super) fn end_process(pid: u32, mode: ProcessEndMode) -> PlatformResult<ProcessEndStatus> {
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return Err(PlatformError::new(
            crate::PlatformErrorCode::AccessDenied,
            "refusing to end the system idle process or MangoDisk itself",
        ));
    }
    match mode {
        // Windows has no graceful signal for windowless processes; posting
        // WM_CLOSE to the process windows is the only cooperative path. The
        // typed `Unsupported` outcome lets Core escalate to force explicitly.
        ProcessEndMode::Graceful => Ok(if post_close_to_process_windows(pid) {
            ProcessEndStatus::Ended
        } else {
            ProcessEndStatus::Unsupported
        }),
        ProcessEndMode::Force => {
            let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
            let Some(handle) = OwnedHandle::new(handle) else {
                return match unsafe { GetLastError() } {
                    ERROR_INVALID_PARAMETER => Ok(ProcessEndStatus::NotFound),
                    ERROR_ACCESS_DENIED => Ok(ProcessEndStatus::PermissionDenied),
                    _ => Err(PlatformError::operation_failed(
                        "windows process termination handle could not be opened",
                    )),
                };
            };
            if unsafe { TerminateProcess(handle.0, 1) } != 0 {
                Ok(ProcessEndStatus::Ended)
            } else {
                let error = std::io::Error::last_os_error();
                Err(PlatformError::io("terminate Windows process", &error))
            }
        }
    }
}

fn read_process(
    entry: &PROCESSENTRY32W,
    context: &SnapshotContext,
) -> Option<ProcessMetricsSnapshot> {
    let pid = entry.th32ProcessID;
    let name = wide_c_string(&entry.szExeFile);
    if name.is_empty() {
        return None;
    }
    // The System Idle Process (0) and the System process (4) have no image
    // path, IO counters, or user account; opening them is denied by design.
    let kernel_owned = pid == 0 || pid == 4;
    let handle = (!kernel_owned)
        .then(|| unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
        .and_then(OwnedHandle::new);
    let executable_path = if kernel_owned {
        ProcessMetric::absent(ProcessMetricAbsence::NotApplicable)
    } else {
        match handle
            .as_ref()
            .and_then(|handle| query_executable_path(handle.0))
        {
            Some(path) => ProcessMetric::present(path),
            None => ProcessMetric::absent(ProcessMetricAbsence::AccessDenied),
        }
    };
    let times = handle.as_ref().and_then(|handle| process_times(handle.0));
    let memory = handle.as_ref().and_then(|handle| process_memory(handle.0));
    let io = handle.as_ref().and_then(|handle| process_io(handle.0));
    let owner = handle
        .as_ref()
        .and_then(|handle| process_owner(handle.0, context));
    let (owner_name, owned_by_current_user) = match owner {
        Some(owner) => (
            match owner.name {
                Some(name) => ProcessMetric::present(name),
                None => ProcessMetric::absent(ProcessMetricAbsence::NotAvailable),
            },
            ProcessMetric::present(owner.is_current_user),
        ),
        None => {
            let absence = if kernel_owned {
                ProcessMetricAbsence::NotApplicable
            } else {
                ProcessMetricAbsence::AccessDenied
            };
            (
                ProcessMetric::absent(absence),
                // PID 0 and 4 are kernel-owned, never the current user.
                if kernel_owned {
                    ProcessMetric::present(false)
                } else {
                    ProcessMetric::absent(absence)
                },
            )
        }
    };
    Some(ProcessMetricsSnapshot {
        pid,
        ppid: entry.th32ParentProcessID,
        name,
        executable_path,
        // Windows security identifiers do not fit a u32; ownership decisions
        // use `owned_by_current_user` instead.
        owner_uid: ProcessMetric::absent(ProcessMetricAbsence::Unsupported),
        owner_name,
        owned_by_current_user,
        // ToolHelp exposes no reliable per-process execution state.
        state: ProcessState::Unknown,
        thread_count: entry.cntThreads,
        cpu_user_ticks: times.map_or(0, |times| times.user_ticks),
        cpu_kernel_ticks: times.map_or(0, |times| times.kernel_ticks),
        cpu_ticks_per_second: FILETIME_TICKS_PER_SECOND,
        rss_bytes: memory.map_or(0, |memory| memory.working_set_bytes),
        io_read_bytes: match io {
            Some(io) => ProcessMetric::present(io.read_bytes),
            None => ProcessMetric::absent(if kernel_owned {
                ProcessMetricAbsence::NotApplicable
            } else {
                ProcessMetricAbsence::AccessDenied
            }),
        },
        io_write_bytes: match io {
            Some(io) => ProcessMetric::present(io.write_bytes),
            None => ProcessMetric::absent(if kernel_owned {
                ProcessMetricAbsence::NotApplicable
            } else {
                ProcessMetricAbsence::AccessDenied
            }),
        },
        // Reliable per-process handle enumeration requires walking the kernel
        // handle table, which is out of scope for this snapshot.
        open_file_count: ProcessMetric::absent(ProcessMetricAbsence::Unsupported),
        started_at_ms: times.map_or(0, |times| times.started_at_ms),
    })
}

#[derive(Clone, Copy)]
struct ProcessTimes {
    started_at_ms: u64,
    user_ticks: u64,
    kernel_ticks: u64,
}

fn process_times(handle: HANDLE) -> Option<ProcessTimes> {
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    let succeeded =
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    (succeeded != 0).then_some(ProcessTimes {
        started_at_ms: filetime_to_unix_ms(&creation),
        user_ticks: filetime_ticks(&user),
        kernel_ticks: filetime_ticks(&kernel),
    })
}

fn filetime_ticks(value: &FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

fn filetime_to_unix_ms(value: &FILETIME) -> u64 {
    filetime_ticks(value).saturating_sub(FILETIME_UNIX_EPOCH_OFFSET) / 10_000
}

struct ProcessMemory {
    working_set_bytes: u64,
}

fn process_memory(handle: HANDLE) -> Option<ProcessMemory> {
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let succeeded = unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) };
    (succeeded != 0).then_some(ProcessMemory {
        working_set_bytes: counters.WorkingSetSize as u64,
    })
}

#[derive(Clone, Copy)]
struct ProcessIo {
    read_bytes: u64,
    write_bytes: u64,
}

fn process_io(handle: HANDLE) -> Option<ProcessIo> {
    let mut counters: IO_COUNTERS = unsafe { zeroed() };
    let succeeded = unsafe { GetProcessIoCounters(handle, &mut counters) };
    (succeeded != 0).then_some(ProcessIo {
        read_bytes: counters.ReadTransferCount,
        write_bytes: counters.WriteTransferCount,
    })
}

struct ProcessOwner {
    name: Option<String>,
    is_current_user: bool,
}

fn process_owner(handle: HANDLE, context: &SnapshotContext) -> Option<ProcessOwner> {
    let sid = token_user_sid(handle)?;
    let is_current_user = context
        .current_user_sid
        .as_deref()
        .is_some_and(|current| unsafe {
            EqualSid(sid.as_ptr().cast(), current.as_ptr().cast()) != 0
        });
    Some(ProcessOwner {
        name: lookup_account_name(&sid),
        is_current_user,
    })
}

/// Reads the user SID of a process token as owned bytes so later lookups
/// never depend on the token handle lifetime.
fn token_user_sid(process: HANDLE) -> Option<Vec<u8>> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let token = OwnedHandle::new(token)?;
    let mut length = 0_u32;
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut length) };
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; length as usize];
    let succeeded = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    };
    if succeeded == 0 {
        return None;
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    copy_sid(user.User.Sid)
}

fn copy_sid(sid: windows_sys::Win32::Security::PSID) -> Option<Vec<u8>> {
    if sid.is_null() {
        return None;
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    if length == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) };
    Some(bytes.to_vec())
}

fn current_user_sid() -> Option<Vec<u8>> {
    static CURRENT_USER_SID: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    CURRENT_USER_SID
        .get_or_init(|| token_user_sid(unsafe { GetCurrentProcess() }))
        .clone()
}

fn lookup_account_name(sid: &[u8]) -> Option<String> {
    let mut name_length = 0_u32;
    let mut domain_length = 0_u32;
    let mut sid_use = 0;
    unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid.as_ptr().cast_mut().cast(),
            std::ptr::null_mut(),
            &mut name_length,
            std::ptr::null_mut(),
            &mut domain_length,
            &mut sid_use,
        )
    };
    if name_length == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    let mut name = vec![0_u16; name_length as usize];
    let mut domain = vec![0_u16; domain_length as usize];
    let succeeded = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid.as_ptr().cast_mut().cast(),
            name.as_mut_ptr(),
            &mut name_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut sid_use,
        )
    };
    if succeeded == 0 || name_length == 0 {
        return None;
    }
    name.truncate(name_length as usize);
    let name = OsString::from_wide(&name).to_string_lossy().into_owned();
    (!name.is_empty()).then_some(name)
}

fn query_executable_path(handle: HANDLE) -> Option<PathBuf> {
    let mut buffer = vec![0_u16; MAX_EXECUTABLE_PATH_UNITS];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

fn post_close_to_process_windows(pid: u32) -> bool {
    let mut context = WindowCloseContext { pid, posted: false };
    unsafe {
        EnumWindows(
            Some(post_close_to_process_window),
            (&mut context as *mut WindowCloseContext) as LPARAM,
        );
    }
    context.posted
}

struct WindowCloseContext {
    pid: u32,
    posted: bool,
}

unsafe extern "system" fn post_close_to_process_window(window: HWND, context: LPARAM) -> i32 {
    let context = unsafe { &mut *(context as *mut WindowCloseContext) };
    let mut window_pid = 0_u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut window_pid);
    }
    if window_pid == context.pid && unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } != 0 {
        context.posted = true;
    }
    1
}

fn wide_c_string(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    OsString::from_wide(&units[..length])
        .to_string_lossy()
        .into_owned()
}
