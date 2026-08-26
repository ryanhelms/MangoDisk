use mangodisk_core::{CoreError, CoreErrorCode};
use rmcp::model::CallToolResult;
use serde_json::{json, Value};

/// Stable adapter error codes. They extend the Core command error vocabulary
/// with guard-specific failures so MCP clients can branch on `code` instead of
/// parsing free-form messages.
pub(crate) const MUTATIONS_DISABLED: &str = "mutationsDisabled";
pub(crate) const CONFIRMATION_REQUIRED: &str = "confirmationRequired";
pub(crate) const TOKEN_UNKNOWN: &str = "tokenUnknown";
pub(crate) const TOKEN_EXPIRED: &str = "tokenExpired";
pub(crate) const TOKEN_DOMAIN_MISMATCH: &str = "tokenDomainMismatch";
pub(crate) const PLAN_MISMATCH: &str = "planMismatch";
pub(crate) const TASK_JOIN_FAILED: &str = "taskJoinFailed";

/// Builds a tool-level error (`isError: true`) with a stable machine-readable
/// envelope. Messages must stay free of filesystem paths and native
/// diagnostics; rule and candidate identifiers are stable and safe to include.
pub(crate) fn tool_error(code: &'static str, message: impl Into<String>) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "error": {
            "code": code,
            "message": message.into(),
        }
    }))
}

/// Translates a Core failure into the adapter error protocol. The native
/// diagnostic can contain private paths, so only the stable code and reason
/// cross the MCP boundary; the diagnostic itself stays out of logs as well
/// because this process forwards stderr to the operator's terminal.
pub(crate) fn core_failure(operation: &'static str, error: CoreError) -> CallToolResult {
    let (code, message, retryable) = match error.code() {
        CoreErrorCode::InvalidInput => {
            ("invalidInput", "the request was rejected as invalid", false)
        }
        CoreErrorCode::OperationBusy => (
            "operationBusy",
            "another MangoDisk operation is already running",
            true,
        ),
        CoreErrorCode::OperationCancelled => {
            ("operationCancelled", "the operation was cancelled", false)
        }
        CoreErrorCode::PermissionDenied => (
            "permissionDenied",
            "the operating system denied access to a required resource",
            false,
        ),
        CoreErrorCode::Persistence => (
            "persistenceFailed",
            "the operation could not read or write MangoDisk state",
            true,
        ),
        CoreErrorCode::OperationFailed | CoreErrorCode::Platform => {
            ("operationFailed", "the operation failed", true)
        }
    };
    if error.code() == CoreErrorCode::OperationBusy {
        log::info!("mcp_tool_deferred operation={operation} reason=operation_busy");
    } else {
        log::error!(
            "mcp_tool_failed operation={operation} code={:?}",
            error.code()
        );
    }
    let mut envelope = json!({
        "code": code,
        "message": message,
        "operation": operation,
        "retryable": retryable,
    });
    if let Some(reason) = error.reason() {
        envelope["reason"] = Value::String(reason.as_str().to_string());
    }
    CallToolResult::structured_error(json!({ "error": envelope }))
}

/// String-only Core failures (plan validation, duplicate paging) already use
/// privacy-safe static messages, but they are still reduced to a stable code
/// here so the wire contract never depends on free-form text.
pub(crate) fn validation_failure(operation: &'static str, detail: String) -> CallToolResult {
    log::info!("mcp_tool_rejected operation={operation} reason=validation");
    CallToolResult::structured_error(json!({
        "error": {
            "code": "invalidInput",
            "message": detail,
            "operation": operation,
            "retryable": false,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_failures_never_leak_native_diagnostics() {
        let result = core_failure(
            "cleanup_execute",
            CoreError::operation_failed("failed to remove /home/user/private/file.bin"),
        );
        let json = serde_json::to_value(&result).expect("tool errors must serialize");

        assert_eq!(json["isError"], true);
        let text = json.to_string();
        assert!(text.contains("operationFailed"));
        assert!(!text.contains("private"));
        assert!(!text.contains("/home/user"));
    }

    #[test]
    fn busy_failures_are_marked_retryable() {
        let result = core_failure("cleanup_scan", CoreError::operation_busy("lock held"));
        let json = serde_json::to_value(&result).expect("tool errors must serialize");

        assert_eq!(json["structuredContent"]["error"]["code"], "operationBusy");
        assert_eq!(
            json["structuredContent"]["error"]["retryable"],
            serde_json::json!(true)
        );
    }
}
