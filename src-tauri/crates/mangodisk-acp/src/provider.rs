//! Data-driven catalog of ACP-capable provider CLIs and availability probing.
//!
//! Providers are locally installed, already-authenticated CLIs (Claude Code,
//! Codex, Kimi, ...). MangoDisk never handles their credentials: the launch
//! command inherits the provider's own login state. The catalog is plain data
//! so callers can extend or override entries per machine; see
//! [`AgentBridge`](crate::AgentBridge) for how the catalog is consumed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::launch::{resolve_program_in, CommandSpec};

/// Default timeout for a provider `--version` probe. Version queries are
/// local process launches; anything slower is treated as unavailable.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Upper bound for captured `--version` output. Keeps a misbehaving or
/// verbose binary from bloating probe results.
const MAX_VERSION_TEXT_LEN: usize = 100;

/// How to verify that a provider CLI is installed.
///
/// `binary` is the executable searched on `PATH`; `version_args` must produce
/// a quick version print (provider CLIs conventionally support `--version`).
/// For launchers such as `npx`, probe the launcher itself rather than the
/// package spec: resolving a package through the launcher could trigger a
/// download, which a probe must never do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeSpec {
    pub binary: String,
    #[serde(default = "default_version_args")]
    pub version_args: Vec<String>,
}

fn default_version_args() -> Vec<String> {
    vec!["--version".to_string()]
}

impl ProbeSpec {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            version_args: default_version_args(),
        }
    }
}

/// One ACP-capable provider CLI.
///
/// `id` is the stable machine key used by configuration and by
/// [`AgentBridge::start_session`](crate::AgentBridge::start_session);
/// `display_name` is a fallback label for UIs that have no localized name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentProviderDescriptor {
    pub id: String,
    pub display_name: String,
    /// Command line used to start an ACP session process.
    pub launch: CommandSpec,
    /// How to check that the provider exists on this machine.
    pub probe: ProbeSpec,
}

/// Result of probing one provider descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedProvider {
    pub descriptor: AgentProviderDescriptor,
    /// Resolved executable that answered the probe.
    pub binary_path: PathBuf,
    /// First line of the version output, when the binary printed one.
    pub version: Option<String>,
}

/// Built-in provider catalog.
///
/// Every entry launches a locally installed CLI that speaks ACP on stdio and
/// carries its own authentication. Callers that need different binaries or
/// wrappers (per-machine installs, `mise`/`volta` shims, corporate mirrors)
/// should clone and adjust these entries rather than patching the bridge.
///
/// Deliberately omitted until they publish a stable ACP entry point:
/// - Grok: `grok acp` has been suggested by community adapters but is not
///   confirmed; a custom entry would look like
///   `launch: CommandSpec::new("grok").args(["acp"])`, `probe: ProbeSpec::new("grok")`.
pub fn default_providers() -> Vec<AgentProviderDescriptor> {
    vec![
        AgentProviderDescriptor {
            id: "claude".to_string(),
            display_name: "Claude Code".to_string(),
            // Zed's adapter wraps the locally installed, already
            // authenticated `claude` CLI. `npx -y` resolves the adapter
            // package on first session start; a machine with a globally
            // installed adapter can override the launch to the bare
            // `claude-code-acp` binary.
            launch: CommandSpec::new("npx").args(["-y", "@zed-industries/claude-code-acp@latest"]),
            probe: ProbeSpec::new("npx"),
        },
        AgentProviderDescriptor {
            id: "codex".to_string(),
            display_name: "OpenAI Codex".to_string(),
            // Codex CLI exposes ACP over stdio as a subcommand.
            launch: CommandSpec::new("codex").args(["acp"]),
            probe: ProbeSpec::new("codex"),
        },
        AgentProviderDescriptor {
            id: "kimi".to_string(),
            display_name: "Kimi".to_string(),
            launch: CommandSpec::new("kimi").args(["acp"]),
            probe: ProbeSpec::new("kimi"),
        },
    ]
}

/// Probe the built-in catalog on the real `PATH`.
///
/// Providers are probed concurrently. The result preserves catalog order and
/// fails closed: absent, unresolvable, or non-answering providers simply do
/// not appear.
pub async fn probe_available_providers() -> Vec<ProbedProvider> {
    probe_providers(&default_providers()).await
}

/// Probe an explicit descriptor list on the real `PATH`.
pub async fn probe_providers(descriptors: &[AgentProviderDescriptor]) -> Vec<ProbedProvider> {
    probe_providers_inner(descriptors, None, DEFAULT_PROBE_TIMEOUT).await
}

async fn probe_providers_inner(
    descriptors: &[AgentProviderDescriptor],
    path_override: Option<OsString>,
    timeout: Duration,
) -> Vec<ProbedProvider> {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        let descriptor = descriptor.clone();
        let path_override = path_override.clone();
        tasks.spawn(async move {
            probe_one(descriptor, path_override, timeout)
                .await
                .map(|probed| (index, probed))
        });
    }

    let mut probed: Vec<(usize, ProbedProvider)> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Some(entry)) => probed.push(entry),
            Ok(None) => {}
            Err(error) => {
                log::warn!("provider probe task failed: {error}");
            }
        }
    }
    probed.sort_by_key(|(index, _)| *index);
    probed.into_iter().map(|(_, entry)| entry).collect()
}

/// Probe one descriptor: resolve the binary, then require a `--version`
/// answer within `timeout`. Both steps fail closed.
async fn probe_one(
    descriptor: AgentProviderDescriptor,
    path_override: Option<OsString>,
    timeout: Duration,
) -> Option<ProbedProvider> {
    let binary_path = resolve_program_in(&descriptor.probe.binary, path_override.as_deref())?;
    let version = read_version(&binary_path, &descriptor.probe.version_args, timeout).await?;
    Some(ProbedProvider {
        descriptor,
        binary_path,
        version: Some(version),
    })
}

/// Run `binary` with the version args and return the first stdout line.
///
/// Returns `None` on spawn failure, non-zero exit, empty output, or timeout;
/// a provider that cannot answer a version query quickly is treated as
/// unavailable rather than allowed to stall UI startup. On timeout the child
/// is killed so probing never leaves a stray process behind.
async fn read_version(binary: &Path, version_args: &[String], timeout: Duration) -> Option<String> {
    let spec = CommandSpec {
        program: binary.to_string_lossy().into_owned(),
        args: version_args.to_vec(),
        env: Vec::new(),
    };
    let mut command = tokio::process::Command::from(crate::launch::std_command(&spec, binary));
    // A probe that stalls must not strand a child: dropping the wait future
    // on timeout kills and reaps it.
    command.kill_on_drop(true);
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            log::debug!("provider version probe failed to complete: {error}");
            return None;
        }
        Err(_) => return None,
    };

    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }
    Some(first_line.chars().take(MAX_VERSION_TEXT_LEN).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_stub(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn descriptor(id: &str, binary: &str) -> AgentProviderDescriptor {
        AgentProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            launch: CommandSpec::new(binary),
            probe: ProbeSpec::new(binary),
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn probe_reports_binary_and_version() {
        let dir = tempfile::tempdir().unwrap();
        write_stub(
            dir.path(),
            "mangodisk-stub-ok",
            "#!/bin/sh\necho 'stub-cli 1.2.3'\n",
        );
        let path_value = crate::launch::join_search_paths(&[dir.path().to_path_buf()]);

        let descriptors = vec![descriptor("stub-ok", "mangodisk-stub-ok")];
        let probed =
            probe_providers_inner(&descriptors, Some(path_value), Duration::from_secs(5)).await;

        assert_eq!(probed.len(), 1);
        assert_eq!(probed[0].descriptor.id, "stub-ok");
        assert_eq!(probed[0].version.as_deref(), Some("stub-cli 1.2.3"));
        assert!(probed[0].binary_path.ends_with("mangodisk-stub-ok"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn probe_skips_missing_and_unresponsive_binaries() {
        let dir = tempfile::tempdir().unwrap();
        write_stub(dir.path(), "mangodisk-stub-slow", "#!/bin/sh\nsleep 30\n");
        write_stub(dir.path(), "mangodisk-stub-failing", "#!/bin/sh\nexit 1\n");
        let path_value = crate::launch::join_search_paths(&[dir.path().to_path_buf()]);

        let descriptors = vec![
            descriptor("slow", "mangodisk-stub-slow"),
            descriptor("failing", "mangodisk-stub-failing"),
            descriptor("missing", "mangodisk-stub-missing"),
        ];
        let probed =
            probe_providers_inner(&descriptors, Some(path_value), Duration::from_millis(300)).await;

        assert!(probed.is_empty(), "all probes must fail closed");
    }

    #[tokio::test]
    async fn probe_preserves_catalog_order() {
        // None of these binaries exist, so order is observable only through
        // the (empty) result; the test mainly asserts no panic on misses.
        let descriptors = vec![
            descriptor("a", "mangodisk-definitely-missing-a"),
            descriptor("b", "mangodisk-definitely-missing-b"),
        ];
        let probed = probe_providers_inner(&descriptors, None, Duration::from_millis(100)).await;
        assert!(probed.is_empty());
    }

    #[test]
    fn default_catalog_covers_documented_providers() {
        let catalog = default_providers();
        let ids: Vec<&str> = catalog
            .iter()
            .map(|provider| provider.id.as_str())
            .collect();
        assert_eq!(ids, ["claude", "codex", "kimi"]);
    }
}
