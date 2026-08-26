//! Provider CLI launch specifications and executable resolution.
//!
//! This module only inspects `PATH` and builds command lines; it never reads
//! or writes user files. Resolved executable paths are logged nowhere: they
//! are machine-local and can reveal user directory layouts.

use std::ffi::OsStr;
#[cfg(test)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Command line used to launch a provider CLI (or to probe it).
///
/// `program` may be an absolute path or a bare executable name resolved
/// against `PATH` at launch time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommandSpec {
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }
}

/// Resolve a program name or path to an executable file.
///
/// A `program` containing a path separator is treated as an explicit path and
/// must exist as a file; a bare name is searched on `PATH` (via `which`, which
/// handles Windows `PATHEXT` extensions). Returns `None` when nothing
/// executable-looking is found; callers map that to
/// [`AcpErrorCode::ProviderUnavailable`](crate::AcpErrorCode::ProviderUnavailable).
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    resolve_program_in(program, None)
}

/// PATH-searchable variant of [`resolve_program`]; `path_override` replaces
/// the process `PATH` when set. Exists so probes and tests can run against a
/// synthetic search path without mutating process-global environment.
pub(crate) fn resolve_program_in(program: &str, path_override: Option<&OsStr>) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        let path = PathBuf::from(program);
        return path.is_file().then_some(path);
    }

    let cwd = std::env::current_dir().ok()?;
    match path_override {
        Some(paths) => which::which_in(program, Some(paths), cwd).ok(),
        None => which::which(program).ok(),
    }
}

/// Build a `std::process::Command` for a resolved executable.
///
/// On Windows, `.cmd`/`.bat` shims (the common form of `npx` and other
/// npm-installed launchers) cannot be started directly by `CreateProcess`;
/// they are wrapped in `cmd.exe /C`. The wrapper is invisible to the ACP
/// transport because stdio pipes pass straight through.
pub fn std_command(spec: &CommandSpec, resolved_program: &Path) -> std::process::Command {
    let mut command = platform_command(resolved_program, &spec.args);
    command.envs(spec.env.iter().map(|(name, value)| (name, value)));
    command
}

/// Produce the command line to spawn for `spec` once its program has been
/// resolved to `resolved_program`.
///
/// Applies the same Windows `.cmd`/`.bat` wrapping as [`std_command`], but
/// returns plain data so callers can feed process builders that only accept a
/// program path plus argv (such as the ACP SDK's `AcpAgentConfig`).
pub fn spawn_spec(spec: &CommandSpec, resolved_program: &Path) -> CommandSpec {
    #[cfg(windows)]
    {
        let needs_cmd_wrapper = resolved_program
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
        if needs_cmd_wrapper {
            let mut args = vec![
                "/C".to_string(),
                resolved_program.to_string_lossy().into_owned(),
            ];
            args.extend(spec.args.iter().cloned());
            return CommandSpec {
                program: "cmd.exe".to_string(),
                args,
                env: spec.env.clone(),
            };
        }
    }

    CommandSpec {
        program: resolved_program.to_string_lossy().into_owned(),
        args: spec.args.clone(),
        env: spec.env.clone(),
    }
}

#[cfg(windows)]
fn platform_command(program: &Path, args: &[String]) -> std::process::Command {
    use std::os::windows::process::CommandExt as _;

    let needs_cmd_wrapper = program
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));

    let mut command = if needs_cmd_wrapper {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.arg("/C").arg(program.as_os_str());
        cmd
    } else {
        std::process::Command::new(program.as_os_str())
    };
    command.args(args);
    // Provider CLIs are background services of a desktop app; never pop a console.
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    command
}

#[cfg(not(windows))]
fn platform_command(program: &Path, args: &[String]) -> std::process::Command {
    let mut command = std::process::Command::new(program.as_os_str());
    command.args(args);
    command
}

/// Join search paths back into a `PATH`-style string for `which_in`.
#[cfg(test)]
pub(crate) fn join_search_paths(paths: &[PathBuf]) -> OsString {
    std::env::join_paths(paths).expect("test search paths must not contain separators")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_must_exist() {
        assert!(resolve_program("/definitely/not/a/real/binary").is_none());
        // The current test executable is a file and resolves.
        let current = std::env::current_exe().unwrap();
        let resolved = resolve_program(current.to_str().unwrap());
        assert_eq!(resolved.as_deref(), Some(current.as_path()));
    }

    #[test]
    fn bare_name_uses_search_path() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("mangodisk-fake-cli");
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        make_executable(&binary);

        let path_value = join_search_paths(&[dir.path().to_path_buf()]);
        let resolved = resolve_program_in("mangodisk-fake-cli", Some(path_value.as_os_str()));
        assert_eq!(resolved.as_deref(), Some(binary.as_path()));

        let missing = resolve_program_in("mangodisk-missing-cli", Some(path_value.as_os_str()));
        assert!(missing.is_none());
    }

    /// `which` requires the executable bit on unix; no-op elsewhere.
    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = path;
    }
}
