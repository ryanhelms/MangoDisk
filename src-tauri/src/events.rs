use serde::Serialize;
use tauri::Emitter;

pub const CLEANUP_SCAN_PROGRESS: &str = "cleanup-scan-progress";
pub const CLEANUP_EXECUTION_PROGRESS: &str = "cleanup-execution-progress";
pub const ANALYSIS_PROGRESS: &str = "analysis-progress";
pub const LARGE_FILES_PROGRESS: &str = "large-files-progress";
pub const DUPLICATE_FILES_PROGRESS: &str = "duplicate-files-progress";
pub const DUPLICATE_FILE_GROUPS: &str = "duplicate-files-groups";
pub const APPLICATION_UNINSTALL_PROGRESS: &str = "application-uninstall-progress";
pub const APPLICATION_UNINSTALL_EXECUTION_PROGRESS: &str =
    "application-uninstall-execution-progress";
#[cfg(target_os = "macos")]
pub const OPEN_ABOUT: &str = "application-menu-open-about";

/// Chat sessions carry one event stream per session, so the channel names are
/// built from the session id instead of being process-wide constants.
pub const CHAT_SESSION_EVENT_PREFIX: &str = "chat-session-event-";
pub const CHAT_SESSION_ENDED_PREFIX: &str = "chat-session-ended-";

pub fn chat_session_event_name(session_id: &str) -> String {
    format!("{CHAT_SESSION_EVENT_PREFIX}{session_id}")
}

pub fn chat_session_ended_event_name(session_id: &str) -> String {
    format!("{CHAT_SESSION_ENDED_PREFIX}{session_id}")
}

/// Emits one typed desktop event and keeps delivery failures in native logs.
/// A closed window must not turn a completed domain operation into a failure.
pub fn emit<S>(app: &tauri::AppHandle, event: &'static str, payload: S)
where
    S: Serialize + Clone,
{
    emit_dynamic(app, event, payload);
}

/// Same delivery contract as [`emit`], for channels whose names are only known
/// at runtime (per-session chat streams).
pub fn emit_dynamic<S>(app: &tauri::AppHandle, event: &str, payload: S)
where
    S: Serialize + Clone,
{
    if let Err(error) = app.emit(event, payload) {
        log::warn!("desktop_event_emit_failed event={event} error={error}");
    }
}
