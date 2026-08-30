use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use mangodisk_platform::{current_platform, InstalledApplication, Platform, ProcessMetricAbsence};
use serde::{Deserialize, Serialize};

use super::ProcessSample;

/// One node in the process parent/child tree.
///
/// `synthetic` marks the pid-0 root that Core creates when the real kernel
/// process is absent from the snapshot; its only purpose is to give orphaned
/// processes a single documented parent instead of many disconnected roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTreeNode {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub synthetic: bool,
    pub children: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTree {
    pub nodes: BTreeMap<u32, ProcessTreeNode>,
    pub roots: Vec<u32>,
}

/// Links processes through `ppid`. Processes whose parent is missing from the
/// snapshot (parent exited, or the snapshot started mid-tree) are reparented
/// to the pid-0 node, which is synthesized when the platform did not report
/// one. Node and child ordering follows numeric pid order for deterministic
/// adapter rendering.
pub fn build_process_tree(processes: &[ProcessSample]) -> ProcessTree {
    let known: BTreeSet<u32> = processes.iter().map(|process| process.pid).collect();
    let mut nodes: BTreeMap<u32, ProcessTreeNode> = processes
        .iter()
        .map(|process| {
            (
                process.pid,
                ProcessTreeNode {
                    pid: process.pid,
                    ppid: process.ppid,
                    name: process.name.clone(),
                    synthetic: false,
                    children: Vec::new(),
                },
            )
        })
        .collect();
    let has_orphans = processes
        .iter()
        .any(|process| process.ppid != process.pid && !known.contains(&process.ppid));
    if has_orphans && !nodes.contains_key(&0) {
        nodes.insert(
            0,
            ProcessTreeNode {
                pid: 0,
                ppid: 0,
                name: "system".to_string(),
                synthetic: true,
                children: Vec::new(),
            },
        );
    }
    let reparented: Vec<(u32, u32)> = processes
        .iter()
        .filter(|process| process.pid != 0)
        .map(|process| {
            let parent = if nodes.contains_key(&process.ppid) && process.ppid != process.pid {
                process.ppid
            } else {
                0
            };
            (process.pid, parent)
        })
        .collect();
    for (pid, parent) in reparented {
        if let Some(node) = nodes.get_mut(&pid) {
            node.ppid = parent;
        }
        if let Some(parent_node) = nodes.get_mut(&parent) {
            parent_node.children.push(pid);
        }
    }
    for node in nodes.values_mut() {
        node.children.sort_unstable();
    }
    let mut roots: Vec<u32> = nodes
        .values()
        .filter(|node| node.pid == 0 || node.ppid == node.pid)
        .map(|node| node.pid)
        .collect();
    roots.sort_unstable();
    ProcessTree { nodes, roots }
}

/// Whether the installed-application inventory could back name/path matching.
/// Association never enumerates applications itself; it reuses the shared
/// Core inventory session, which may legitimately be unavailable (Linux) or
/// stale, in which case every process reports `Unmatched` and the status
/// tells adapters why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessAssociationInventoryStatus {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessApplicationMatch {
    pub pid: u32,
    pub application_identifier: Option<String>,
    pub application_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessApplicationAssociations {
    pub inventory_status: ProcessAssociationInventoryStatus,
    pub matches: Vec<ProcessApplicationMatch>,
}

/// Associates processes with the installed-application inventory the platform
/// already provides. Matching rules, in priority order:
///
/// 1. the process executable path exactly equals a catalog executable path;
/// 2. the process executable path lives inside the application bundle path
///    (macOS `.app` helpers, packaged Windows apps);
/// 3. the executable file name equals a catalog executable file name, which
///    covers inventory paths written with a different root syntax.
pub fn associate_applications(processes: &[ProcessSample]) -> ProcessApplicationAssociations {
    associate_with(
        processes,
        &crate::applications::catalog::ScanContext::capture(),
    )
}

fn associate_with(
    processes: &[ProcessSample],
    context: &crate::applications::catalog::ScanContext,
) -> ProcessApplicationAssociations {
    let applications = context.inventory.installed_applications();
    if applications.is_empty() {
        return ProcessApplicationAssociations {
            inventory_status: ProcessAssociationInventoryStatus::Unavailable,
            matches: Vec::new(),
        };
    }
    // Deterministic tie-breaking when two catalog records match one process.
    let mut ordered: Vec<&InstalledApplication> = applications.iter().collect();
    ordered.sort_by(|left, right| left.catalog_identifier.cmp(&right.catalog_identifier));
    let platform = current_platform();
    let matches = processes
        .iter()
        .map(|process| {
            let application = process
                .executable_path
                .as_deref()
                .and_then(|path| match_application(path, &ordered, &platform));
            ProcessApplicationMatch {
                pid: process.pid,
                application_identifier: application
                    .map(|application| application.catalog_identifier.clone()),
                application_name: application.map(|application| application.name.clone()),
            }
        })
        .collect();
    ProcessApplicationAssociations {
        inventory_status: ProcessAssociationInventoryStatus::Available,
        matches,
    }
}

fn match_application<'a>(
    executable_path: &Path,
    applications: &[&'a InstalledApplication],
    platform: &impl Platform,
) -> Option<&'a InstalledApplication> {
    applications
        .iter()
        .find(|application| {
            application
                .executable_paths
                .iter()
                .any(|candidate| platform.paths_equal(candidate, executable_path))
        })
        .or_else(|| {
            applications.iter().find(|application| {
                application
                    .bundle_path
                    .as_deref()
                    .is_some_and(|bundle| platform.path_is_same_or_child(executable_path, bundle))
            })
        })
        .or_else(|| {
            let name = executable_path
                .file_name()?
                .to_string_lossy()
                .to_lowercase();
            applications.iter().find(|application| {
                application
                    .executable_paths
                    .iter()
                    .filter_map(|candidate| candidate.file_name())
                    .any(|candidate| candidate.to_string_lossy().eq_ignore_ascii_case(&name))
            })
        })
        .copied()
}

/// Product classification driving presentation and kill-guard policy.
///
/// Rules are evaluated in order and are identical in spirit across operating
/// systems; the OS-specific facts they consume are documented per rule:
///
/// 1. `CriticalSystem`: pid 0 or 1 everywhere, pid 4 (System) on Windows, and
///    any process without a userspace image (kernel threads on Linux,
///    kernel_task on macOS). These are never endable.
/// 2. `SystemService`: unix processes with effective uid 0 (init/systemd/
///    launchd-owned daemons), processes owned by anyone but the current user
///    (Windows services such as svchost, or other users' processes), and
///    processes whose ownership cannot be proven (fail closed).
/// 3. `UserApplication`: current-user processes associated with an installed
///    application from the platform inventory.
/// 4. `UserBackground`: every other current-user process (agents, CLI tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessClassification {
    CriticalSystem,
    SystemService,
    UserApplication,
    UserBackground,
}

/// Platform-neutral facts classification consumes, so the rule matrix is
/// testable without a live operating-system snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessClassificationFacts {
    pub pid: u32,
    pub owner_uid: Option<u32>,
    pub owned_by_current_user: Option<bool>,
    pub executable_path_absence: Option<ProcessMetricAbsence>,
    pub application_associated: bool,
}

impl ProcessClassificationFacts {
    pub fn from_sample(sample: &ProcessSample, application_associated: bool) -> Self {
        Self {
            pid: sample.pid,
            owner_uid: sample.owner_uid,
            owned_by_current_user: sample.owned_by_current_user,
            executable_path_absence: sample.executable_path_absence,
            application_associated,
        }
    }
}

pub fn classify_process(facts: &ProcessClassificationFacts) -> ProcessClassification {
    if facts.pid <= 1 || (cfg!(windows) && facts.pid == 4) {
        return ProcessClassification::CriticalSystem;
    }
    if facts.executable_path_absence == Some(ProcessMetricAbsence::NotApplicable) {
        return ProcessClassification::CriticalSystem;
    }
    if facts.owner_uid == Some(0) {
        return ProcessClassification::SystemService;
    }
    match facts.owned_by_current_user {
        Some(true) => {
            if facts.application_associated {
                ProcessClassification::UserApplication
            } else {
                ProcessClassification::UserBackground
            }
        }
        Some(false) | None => ProcessClassification::SystemService,
    }
}

/// Highest-CPU processes, ordered by computed utilization. Processes without
/// a rate (new between samples) rank below every measured process.
pub fn top_processes_by_cpu(samples: &[ProcessSample], limit: usize) -> Vec<&ProcessSample> {
    let mut ranked: Vec<&ProcessSample> = samples.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .cpu_percent
            .unwrap_or(f64::MIN)
            .total_cmp(&left.cpu_percent.unwrap_or(f64::MIN))
            .then(left.pid.cmp(&right.pid))
    });
    ranked.truncate(limit);
    ranked
}

/// Highest-memory processes, ordered by resident bytes.
pub fn top_processes_by_rss(samples: &[ProcessSample], limit: usize) -> Vec<&ProcessSample> {
    let mut ranked: Vec<&ProcessSample> = samples.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .rss_bytes
            .cmp(&left.rss_bytes)
            .then(left.pid.cmp(&right.pid))
    });
    ranked.truncate(limit);
    ranked
}

/// Highest disk writers, ordered by computed write rate. Processes without an
/// IO capability or baseline rank below every measured process.
pub fn top_processes_by_write_rate(samples: &[ProcessSample], limit: usize) -> Vec<&ProcessSample> {
    let mut ranked: Vec<&ProcessSample> = samples.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .write_bps
            .unwrap_or(f64::MIN)
            .total_cmp(&left.write_bps.unwrap_or(f64::MIN))
            .then(left.pid.cmp(&right.pid))
    });
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, ppid: u32, name: &str) -> ProcessSample {
        ProcessSample {
            pid,
            ppid,
            name: name.to_string(),
            executable_path: None,
            executable_path_absence: None,
            owner_uid: None,
            owner_name: None,
            owned_by_current_user: None,
            state: mangodisk_platform::ProcessState::Running,
            thread_count: 1,
            cpu_user_ticks: 0,
            cpu_kernel_ticks: 0,
            cpu_ticks_per_second: 100,
            cpu_percent: None,
            rss_bytes: 0,
            io_read_bytes: None,
            io_write_bytes: None,
            io_absence: None,
            read_bps: None,
            write_bps: None,
            open_file_count: None,
            open_files_absence: None,
            started_at_ms: 1,
        }
    }

    #[test]
    fn tree_reparents_orphans_to_a_synthetic_root() {
        let tree = build_process_tree(&[
            sample(2, 0, "kthreadd"),
            sample(10, 2, "child"),
            // Parent 99 is missing from the snapshot.
            sample(20, 99, "orphan"),
        ]);
        let root = tree.nodes.get(&0).expect("a synthetic root must exist");
        assert!(root.synthetic);
        assert!(root.children.contains(&2));
        assert!(root.children.contains(&20));
        assert_eq!(tree.nodes.get(&20).expect("orphan node").ppid, 0);
        assert_eq!(tree.nodes.get(&10).expect("child node").ppid, 2);
        assert_eq!(tree.roots, vec![0]);
    }

    #[test]
    fn tree_uses_the_real_pid_zero_when_present() {
        let tree = build_process_tree(&[sample(0, 0, "kernel"), sample(5, 42, "orphan")]);
        assert!(!tree.nodes.get(&0).expect("pid 0 node").synthetic);
        assert_eq!(tree.nodes.get(&5).expect("orphan").ppid, 0);
        assert_eq!(tree.roots, vec![0]);
    }

    fn facts(
        pid: u32,
        owner_uid: Option<u32>,
        owned_by_current_user: Option<bool>,
        executable_path_absence: Option<ProcessMetricAbsence>,
        application_associated: bool,
    ) -> ProcessClassificationFacts {
        ProcessClassificationFacts {
            pid,
            owner_uid,
            owned_by_current_user,
            executable_path_absence,
            application_associated,
        }
    }

    #[test]
    fn classification_matrix_covers_the_documented_rules() {
        // pid 1 (init / launchd) is always critical.
        assert_eq!(
            classify_process(&facts(1, Some(0), Some(false), None, false)),
            ProcessClassification::CriticalSystem
        );
        // A kernel thread has no userspace image.
        assert_eq!(
            classify_process(&facts(
                42,
                Some(0),
                Some(false),
                Some(ProcessMetricAbsence::NotApplicable),
                false,
            )),
            ProcessClassification::CriticalSystem
        );
        // A root-owned daemon with a normal executable.
        assert_eq!(
            classify_process(&facts(42, Some(0), Some(false), None, false)),
            ProcessClassification::SystemService
        );
        // Another user's process.
        assert_eq!(
            classify_process(&facts(42, Some(1001), Some(false), None, false)),
            ProcessClassification::SystemService
        );
        // Unknown ownership fails closed.
        assert_eq!(
            classify_process(&facts(42, None, None, None, false)),
            ProcessClassification::SystemService
        );
        // Current-user processes split on application association.
        assert_eq!(
            classify_process(&facts(42, Some(1000), Some(true), None, true)),
            ProcessClassification::UserApplication
        );
        assert_eq!(
            classify_process(&facts(42, Some(1000), Some(true), None, false)),
            ProcessClassification::UserBackground
        );
        // Access-denied executables do not imply a kernel thread.
        assert_eq!(
            classify_process(&facts(
                42,
                Some(1000),
                Some(true),
                Some(ProcessMetricAbsence::AccessDenied),
                false,
            )),
            ProcessClassification::UserBackground
        );
    }

    #[test]
    fn aggregates_rank_measured_processes_first() {
        let mut low = sample(1, 0, "low");
        low.cpu_percent = Some(1.0);
        low.rss_bytes = 10;
        let mut high = sample(2, 0, "high");
        high.cpu_percent = Some(50.0);
        high.rss_bytes = 20;
        high.write_bps = Some(1024.0);
        let unrated = sample(3, 0, "new");
        let samples = vec![low, high, unrated];

        let by_cpu = top_processes_by_cpu(&samples, 2);
        assert_eq!(by_cpu[0].pid, 2);
        assert_eq!(by_cpu[1].pid, 1);
        let by_rss = top_processes_by_rss(&samples, 2);
        assert_eq!(by_rss[0].pid, 2);
        let by_write = top_processes_by_write_rate(&samples, 2);
        assert_eq!(by_write[0].pid, 2);
        assert_eq!(by_write[1].pid, 1);
    }
}
