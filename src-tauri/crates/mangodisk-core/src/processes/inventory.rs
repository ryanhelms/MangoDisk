use std::{
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use mangodisk_platform::{
    current_platform, Platform, ProcessMetricAbsence, ProcessMetricsSnapshot, ProcessState,
};
use serde::{Deserialize, Serialize};

use crate::{
    filesystem::metadata::now_ms,
    shared::{CoreError, CoreResult},
};

pub const PROCESS_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Interval between the two platform counter samples used for rate math.
/// Rate computation deliberately lives in Core so platform snapshots stay
/// unit-faithful raw facts.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const MAX_FILTER_TEXT_BYTES: usize = 128;

static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// User selection applied after rates are computed.
///
/// All text matching is case-insensitive. `user` matches either the resolved
/// account name or the numeric uid text so adapters never need to know which
/// form the platform provides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessScanFilter {
    pub name_contains: Option<String>,
    pub user: Option<String>,
    pub min_rss_bytes: Option<u64>,
}

/// One process with raw counters and Core-computed rates.
///
/// `cpu_percent` and the IO rates are `None` for processes that were not
/// present in the baseline sample (new, or reusing a PID) so a freshly
/// started process never reports its lifetime average as current load.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSample {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub executable_path: Option<PathBuf>,
    pub executable_path_absence: Option<ProcessMetricAbsence>,
    pub owner_uid: Option<u32>,
    pub owner_name: Option<String>,
    pub owned_by_current_user: Option<bool>,
    pub state: ProcessState,
    pub thread_count: u32,
    pub cpu_user_ticks: u64,
    pub cpu_kernel_ticks: u64,
    pub cpu_ticks_per_second: u64,
    pub cpu_percent: Option<f64>,
    pub rss_bytes: u64,
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
    pub io_absence: Option<ProcessMetricAbsence>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub open_file_count: Option<u32>,
    pub open_files_absence: Option<ProcessMetricAbsence>,
    pub started_at_ms: u64,
}

impl ProcessSample {
    pub(crate) fn from_platform(snapshot: &ProcessMetricsSnapshot) -> Self {
        Self {
            pid: snapshot.pid,
            ppid: snapshot.ppid,
            name: snapshot.name.clone(),
            executable_path: snapshot.executable_path.value().cloned(),
            executable_path_absence: snapshot.executable_path.absence(),
            owner_uid: snapshot.owner_uid.value().copied(),
            owner_name: snapshot.owner_name.value().cloned(),
            owned_by_current_user: snapshot.owned_by_current_user.value().copied(),
            state: snapshot.state,
            thread_count: snapshot.thread_count,
            cpu_user_ticks: snapshot.cpu_user_ticks,
            cpu_kernel_ticks: snapshot.cpu_kernel_ticks,
            cpu_ticks_per_second: snapshot.cpu_ticks_per_second,
            cpu_percent: None,
            rss_bytes: snapshot.rss_bytes,
            io_read_bytes: snapshot.io_read_bytes.value().copied(),
            io_write_bytes: snapshot.io_write_bytes.value().copied(),
            io_absence: snapshot
                .io_read_bytes
                .absence()
                .or(snapshot.io_write_bytes.absence()),
            read_bps: None,
            write_bps: None,
            open_file_count: snapshot.open_file_count.value().copied(),
            open_files_absence: snapshot.open_file_count.absence(),
            started_at_ms: snapshot.started_at_ms,
        }
    }
}

/// Point-in-time process inventory with computed rates.
///
/// The schema version covers the persisted and cross-process shape; readers
/// must reject unknown versions instead of guessing at changed rate fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub captured_at_ms: u64,
    pub sample_interval_ms: u64,
    pub cpu_ticks_per_second: u64,
    pub logical_cpu_count: u32,
    /// Processes seen only in the second sample; their rates are `None`.
    pub new_process_count: u64,
    /// Processes that exited between the two samples. This is a normal race,
    /// never an error.
    pub exited_process_count: u64,
    pub processes: Vec<ProcessSample>,
}

pub struct ProcessInventoryService;

impl ProcessInventoryService {
    pub fn scan(filter: ProcessScanFilter) -> CoreResult<ProcessSnapshot> {
        validate_filter(&filter)?;
        let started = Instant::now();
        let platform = current_platform();
        let first = platform.snapshot_processes().map_err(CoreError::from)?;
        std::thread::sleep(SAMPLE_INTERVAL);
        let second = platform.snapshot_processes().map_err(CoreError::from)?;
        let elapsed_ms = (started.elapsed().as_millis() as u64).max(1);
        let logical_cpu_count = std::thread::available_parallelism()
            .map(|count| count.get() as u32)
            .unwrap_or(1);

        let baseline: HashMap<u32, &ProcessMetricsSnapshot> =
            first.iter().map(|process| (process.pid, process)).collect();
        let mut processes = Vec::with_capacity(second.len());
        let mut matched_count = 0_u64;
        let mut new_process_count = 0_u64;
        for current in &second {
            // A PID reused between samples is detected through the start time
            // when the platform provides one; an unknown start time (0) falls
            // back to plain PID identity as the best available evidence.
            let baseline = baseline.get(&current.pid).copied().filter(|previous| {
                previous.started_at_ms == 0
                    || current.started_at_ms == 0
                    || previous.started_at_ms == current.started_at_ms
            });
            let mut sample = ProcessSample::from_platform(current);
            if let Some(previous) = baseline {
                matched_count += 1;
                sample.cpu_percent = cpu_percent(
                    current
                        .cpu_user_ticks
                        .saturating_add(current.cpu_kernel_ticks)
                        .saturating_sub(
                            previous
                                .cpu_user_ticks
                                .saturating_add(previous.cpu_kernel_ticks),
                        ),
                    current.cpu_ticks_per_second,
                    elapsed_ms,
                    logical_cpu_count,
                );
                sample.read_bps = io_rate(
                    previous.io_read_bytes.value().copied(),
                    current.io_read_bytes.value().copied(),
                    elapsed_ms,
                );
                sample.write_bps = io_rate(
                    previous.io_write_bytes.value().copied(),
                    current.io_write_bytes.value().copied(),
                    elapsed_ms,
                );
            } else {
                new_process_count += 1;
            }
            processes.push(sample);
        }
        processes.retain(|sample| matches_filter(sample, &filter));
        processes.sort_by_key(|sample| sample.pid);

        let snapshot = ProcessSnapshot {
            schema_version: PROCESS_SNAPSHOT_SCHEMA_VERSION,
            snapshot_id: format!(
                "process-scan-{}-{}",
                now_ms(),
                SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            captured_at_ms: now_ms(),
            sample_interval_ms: elapsed_ms,
            cpu_ticks_per_second: second
                .first()
                .map_or(0, |process| process.cpu_ticks_per_second),
            logical_cpu_count,
            new_process_count,
            exited_process_count: first.len() as u64 - matched_count,
            processes,
        };
        log::info!(
            "process_scan_finished snapshot_id={} process_count={} new_process_count={} exited_process_count={} elapsed_ms={}",
            snapshot.snapshot_id,
            snapshot.processes.len(),
            snapshot.new_process_count,
            snapshot.exited_process_count,
            started.elapsed().as_millis()
        );
        Ok(snapshot)
    }
}

/// Computes CPU usage between two counter samples as a percentage of the
/// total machine capacity (`100.0` means one fully busy machine across every
/// logical CPU, matching the per-process cap of `100 * logical_cpus`).
pub(crate) fn cpu_percent(
    delta_ticks: u64,
    ticks_per_second: u64,
    elapsed_ms: u64,
    logical_cpus: u32,
) -> Option<f64> {
    if ticks_per_second == 0 || elapsed_ms == 0 || logical_cpus == 0 {
        return None;
    }
    let busy_seconds = delta_ticks as f64 / ticks_per_second as f64;
    let capacity_seconds = f64::from(logical_cpus) * elapsed_ms as f64 / 1000.0;
    Some(busy_seconds / capacity_seconds * 100.0)
}

/// Computes a per-second byte rate between two cumulative counters.
pub(crate) fn bytes_per_second(delta_bytes: u64, elapsed_ms: u64) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    Some(delta_bytes as f64 * 1000.0 / elapsed_ms as f64)
}

fn io_rate(previous: Option<u64>, current: Option<u64>, elapsed_ms: u64) -> Option<f64> {
    bytes_per_second(current?.saturating_sub(previous?), elapsed_ms)
}

fn validate_filter(filter: &ProcessScanFilter) -> CoreResult<()> {
    for text in [&filter.name_contains, &filter.user].into_iter().flatten() {
        if text.trim().is_empty() || text.len() > MAX_FILTER_TEXT_BYTES {
            return Err(CoreError::invalid_input(
                "the process scan filter text is invalid",
            ));
        }
    }
    Ok(())
}

fn matches_filter(sample: &ProcessSample, filter: &ProcessScanFilter) -> bool {
    if let Some(name_contains) = &filter.name_contains {
        if !sample
            .name
            .to_lowercase()
            .contains(&name_contains.to_lowercase())
        {
            return false;
        }
    }
    if let Some(user) = &filter.user {
        let user = user.to_lowercase();
        let name_matches = sample
            .owner_name
            .as_deref()
            .is_some_and(|owner| owner.to_lowercase() == user);
        let uid_matches = sample.owner_uid.is_some_and(|uid| uid.to_string() == user);
        if !name_matches && !uid_matches {
            return false;
        }
    }
    if let Some(min_rss_bytes) = filter.min_rss_bytes {
        if sample.rss_bytes < min_rss_bytes {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_scales_ticks_to_machine_capacity() {
        // 50 ticks at 100 ticks/s over 500 ms on 4 CPUs = 0.5 s busy of 2 s
        // capacity = 25 percent (one fully busy logical CPU).
        assert_eq!(cpu_percent(50, 100, 500, 4), Some(25.0));
        // One logical CPU fully busy on a 4-CPU machine.
        assert_eq!(
            cpu_percent(50, 100, 500, 4).map(|value| value * 4.0),
            Some(100.0)
        );
        assert_eq!(cpu_percent(1, 0, 500, 4), None);
        assert_eq!(cpu_percent(1, 100, 0, 4), None);
        assert_eq!(cpu_percent(1, 100, 500, 0), None);
    }

    #[test]
    fn byte_rates_use_the_sample_interval() {
        assert_eq!(bytes_per_second(500_000, 500), Some(1_000_000.0));
        assert_eq!(bytes_per_second(1, 0), None);
        // Counter resets (process restarted inside the interval) saturate to
        // zero instead of wrapping to a huge rate.
        assert_eq!(io_rate(Some(100), Some(50), 500), Some(0.0));
        assert_eq!(io_rate(None, Some(50), 500), None);
        assert_eq!(io_rate(Some(100), None, 500), None);
    }

    #[test]
    fn filter_matches_name_user_and_rss() {
        let sample = ProcessSample {
            pid: 42,
            ppid: 1,
            name: "Chromium Browser".to_string(),
            executable_path: None,
            executable_path_absence: Some(ProcessMetricAbsence::AccessDenied),
            owner_uid: Some(1000),
            owner_name: Some("Ryan".to_string()),
            owned_by_current_user: Some(true),
            state: ProcessState::Running,
            thread_count: 4,
            cpu_user_ticks: 0,
            cpu_kernel_ticks: 0,
            cpu_ticks_per_second: 100,
            cpu_percent: None,
            rss_bytes: 4096,
            io_read_bytes: None,
            io_write_bytes: None,
            io_absence: Some(ProcessMetricAbsence::AccessDenied),
            read_bps: None,
            write_bps: None,
            open_file_count: None,
            open_files_absence: Some(ProcessMetricAbsence::Unsupported),
            started_at_ms: 1,
        };
        let filter = ProcessScanFilter {
            name_contains: Some("chromium".to_string()),
            user: Some("ryan".to_string()),
            min_rss_bytes: Some(4096),
        };
        assert!(matches_filter(&sample, &filter));
        assert!(matches_filter(
            &sample,
            &ProcessScanFilter {
                user: Some("1000".to_string()),
                ..ProcessScanFilter::default()
            }
        ));
        assert!(!matches_filter(
            &sample,
            &ProcessScanFilter {
                min_rss_bytes: Some(4097),
                ..ProcessScanFilter::default()
            }
        ));
        assert!(!matches_filter(
            &sample,
            &ProcessScanFilter {
                user: Some("root".to_string()),
                ..ProcessScanFilter::default()
            }
        ));
    }

    #[test]
    fn filter_validation_rejects_empty_and_oversized_text() {
        assert!(validate_filter(&ProcessScanFilter::default()).is_ok());
        assert!(validate_filter(&ProcessScanFilter {
            name_contains: Some("  ".to_string()),
            ..ProcessScanFilter::default()
        })
        .is_err());
        assert!(validate_filter(&ProcessScanFilter {
            user: Some("x".repeat(MAX_FILTER_TEXT_BYTES + 1)),
            ..ProcessScanFilter::default()
        })
        .is_err());
    }

    /// Linux smoke test: the two-sample scan computes rates for stable PIDs
    /// and reports the current process with a measured CPU value.
    #[cfg(target_os = "linux")]
    #[test]
    fn live_scan_computes_rates_for_this_process() {
        let snapshot = ProcessInventoryService::scan(ProcessScanFilter::default())
            .expect("the live scan should succeed");
        assert_eq!(snapshot.schema_version, PROCESS_SNAPSHOT_SCHEMA_VERSION);
        assert!(snapshot.sample_interval_ms >= 500);
        assert!(snapshot.logical_cpu_count > 0);
        let own = snapshot
            .processes
            .iter()
            .find(|process| process.pid == std::process::id())
            .expect("the scan must contain the test process");
        assert!(own.cpu_percent.is_some());
        assert_eq!(own.owned_by_current_user, Some(true));
    }
}
