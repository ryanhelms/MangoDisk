use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Typed reason a process metric is absent from a snapshot.
///
/// Core presents these reasons to users and adapters instead of inferring
/// support from an empty value, so a denied read can never be mistaken for a
/// genuinely zero counter. These codes cross the Core and adapter boundaries
/// and must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessMetricAbsence {
    /// The operating system has no implementation for this metric at all, for
    /// example per-process open-file enumeration on Windows.
    Unsupported,
    /// The operating system denied access, usually because the process belongs
    /// to another user or is protected by the kernel.
    AccessDenied,
    /// The metric does not exist for this kind of process, for example an
    /// executable path for a kernel thread.
    NotApplicable,
    /// The process changed or exited while the snapshot was being assembled.
    /// Callers may treat this as transient and observe a value on the next
    /// sample.
    NotAvailable,
}

/// One optional process metric paired with its typed absence reason.
///
/// The invariant `value.is_none() == absence.is_some()` is enforced by the
/// constructors so consumers never face an unexplained `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMetric<T> {
    value: Option<T>,
    absence: Option<ProcessMetricAbsence>,
}

impl<T> ProcessMetric<T> {
    pub fn present(value: T) -> Self {
        Self {
            value: Some(value),
            absence: None,
        }
    }

    pub fn absent(absence: ProcessMetricAbsence) -> Self {
        Self {
            value: None,
            absence: Some(absence),
        }
    }

    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn absence(&self) -> Option<ProcessMetricAbsence> {
        self.absence
    }

    pub fn into_value(self) -> Option<T> {
        self.value
    }
}

/// Normalized process lifecycle state across operating systems.
///
/// Windows exposes no reliable per-process state through the supported
/// enumeration APIs, so its snapshots report `Unknown` rather than inventing
/// one. These codes cross the Core and adapter boundaries and must remain
/// stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessState {
    Running,
    Sleeping,
    Idle,
    Stopped,
    Zombie,
    Dead,
    Unknown,
}

/// Raw per-process counters captured at one point in time.
///
/// The platform layer reports facts only: CPU values stay in the native tick
/// unit (`cpu_ticks_per_second` documents the unit, for example 100 jiffies
/// per second on Linux, nanoseconds on macOS, and 100 ns units on Windows).
/// Core takes two snapshots and computes all rates, which keeps every
/// platform-specific unit conversion out of rate math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMetricsSnapshot {
    pub pid: u32,
    /// Parent process identifier. `0` means the process has no living parent
    /// inside the snapshot (for example PID 0 itself or a reparented orphan).
    pub ppid: u32,
    /// Short process name (comm value or executable file name). Never empty.
    pub name: String,
    pub executable_path: ProcessMetric<PathBuf>,
    /// POSIX effective user identifier. Windows reports `Unsupported` because
    /// its security identifiers do not fit a `u32`; use
    /// `owned_by_current_user` for ownership decisions instead.
    pub owner_uid: ProcessMetric<u32>,
    pub owner_name: ProcessMetric<String>,
    /// Whether the process owner matches the user running MangoDisk. This is
    /// the cross-platform ownership fact that Core kill guards rely on.
    pub owned_by_current_user: ProcessMetric<bool>,
    pub state: ProcessState,
    pub thread_count: u32,
    /// Cumulative user CPU in the native tick unit. `0` when the process
    /// cannot be opened (Windows without query access), never a rate.
    pub cpu_user_ticks: u64,
    pub cpu_kernel_ticks: u64,
    pub cpu_ticks_per_second: u64,
    /// Resident working set in bytes. `0` when the process cannot be opened
    /// or has no userspace resident set (kernel threads).
    pub rss_bytes: u64,
    pub io_read_bytes: ProcessMetric<u64>,
    pub io_write_bytes: ProcessMetric<u64>,
    pub open_file_count: ProcessMetric<u32>,
    /// Process start time as Unix epoch milliseconds. `0` means the platform
    /// could not determine it; Core treats two snapshots with different
    /// non-zero start times for one PID as PID reuse.
    pub started_at_ms: u64,
}

/// Termination strength requested for one process.
///
/// `Graceful` gives the process an opportunity to clean up (SIGTERM on Unix,
/// `WM_CLOSE` on Windows). `Force` is uninterruptible (SIGKILL or
/// `TerminateProcess`) and must stay behind an explicit user escalation in
/// product flows. These codes cross process boundaries and must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessEndMode {
    Graceful,
    Force,
}

/// Bounded outcome of one platform end request.
///
/// `Ended` only certifies that the operating system accepted the request; a
/// process may still be alive afterwards (save prompts, signal handlers, or
/// immediate restarts). Callers must verify liveness from a fresh snapshot
/// instead of trusting this status alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessEndStatus {
    Ended,
    NotFound,
    PermissionDenied,
    /// The platform cannot end arbitrary processes, or the requested mode has
    /// no meaning for this process (for example a graceful close of a
    /// windowless Windows process).
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::{ProcessMetric, ProcessMetricAbsence};

    #[test]
    fn metric_constructors_enforce_the_absence_invariant() {
        let present = ProcessMetric::present(42_u64);
        assert_eq!(present.value(), Some(&42));
        assert_eq!(present.absence(), None);

        let absent = ProcessMetric::<u64>::absent(ProcessMetricAbsence::AccessDenied);
        assert_eq!(absent.value(), None);
        assert_eq!(absent.absence(), Some(ProcessMetricAbsence::AccessDenied));
    }
}
