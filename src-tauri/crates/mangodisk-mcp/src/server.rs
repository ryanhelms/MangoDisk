use std::sync::Arc;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo},
    tool_handler, ServerHandler,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    errors,
    execution_tokens::{ExecutionTokenStore, MutationDomain, EXECUTION_TOKEN_TTL},
    redaction,
};

/// Shared adapter state. Token grants are process-local and in-memory only;
/// nothing here survives a restart.
pub(crate) struct AdapterState {
    pub(crate) include_full_paths: bool,
    pub(crate) mutations_enabled: bool,
    tokens: ExecutionTokenStore,
}

impl AdapterState {
    pub(crate) fn new(include_full_paths: bool, mutations_enabled: bool) -> Self {
        Self {
            include_full_paths,
            mutations_enabled,
            tokens: ExecutionTokenStore::new(EXECUTION_TOKEN_TTL),
        }
    }

    /// Fail-closed gate in front of every mutation tool. Mutation tools stay
    /// listed when disabled so clients can discover the capability; calls are
    /// rejected with a stable `mutationsDisabled` error until the operator
    /// restarts the server with `--enable-mutations`.
    ///
    /// The error is boxed because `CallToolResult` is a large type and these
    /// guard rejections are cold paths; an unboxed `Err` trips
    /// `clippy::result_large_err` on every workspace check.
    pub(crate) fn ensure_mutations_enabled(&self) -> Result<(), Box<CallToolResult>> {
        if self.mutations_enabled {
            return Ok(());
        }
        Err(Box::new(errors::tool_error(
            errors::MUTATIONS_DISABLED,
            "mutation tools are disabled; restart mangodisk-mcp with --enable-mutations or MANGODISK_MCP_ENABLE_MUTATIONS=1",
        )))
    }

    /// Execute tools require `confirm: true` so a client must state destructive
    /// intent explicitly; the check runs before the single-use token is
    /// consumed so a malformed call never burns a grant.
    pub(crate) fn ensure_confirmed(confirm: bool) -> Result<(), Box<CallToolResult>> {
        if confirm {
            return Ok(());
        }
        Err(Box::new(errors::tool_error(
            errors::CONFIRMATION_REQUIRED,
            "pass confirm: true to execute after reviewing the scan preview",
        )))
    }

    pub(crate) fn take_token(
        &self,
        token: &str,
        domain: MutationDomain,
    ) -> Result<Value, Box<CallToolResult>> {
        self.tokens.take(token, domain).map_err(|error| {
            log::info!(
                "execution_token_rejected domain={} code={}",
                domain.as_str(),
                error.code()
            );
            Box::new(errors::tool_error(error.code(), error.message()))
        })
    }

    /// Issues an execution grant for a preview when mutations are enabled.
    /// Read-only responses carry no token field at all when disabled so
    /// clients never handle a grant that cannot work.
    pub(crate) fn issue_token(&self, domain: MutationDomain, snapshot: Value) -> Option<String> {
        self.mutations_enabled
            .then(|| self.tokens.issue(domain, snapshot))
    }
}

#[derive(Clone)]
pub(crate) struct MangoDiskServer {
    pub(crate) state: Arc<AdapterState>,
    tool_router: ToolRouter<Self>,
}

impl MangoDiskServer {
    pub(crate) fn new(state: Arc<AdapterState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Serializes a typed Core result, attaches an execution token when one
    /// was issued, and applies path redaction unless the operator opted into
    /// full paths. Serialization of Core results is effectively infallible; a
    /// failure degrades to a stable tool error instead of a protocol error.
    pub(crate) fn respond<T: Serialize>(
        &self,
        result: &T,
        execution_token: Option<String>,
    ) -> CallToolResult {
        let mut value = match serde_json::to_value(result) {
            Ok(value) => value,
            Err(error) => {
                log::error!("mcp_tool_result_serialization_failed error={error}");
                return errors::tool_error(
                    errors::TASK_JOIN_FAILED,
                    "failed to serialize the tool result",
                );
            }
        };
        if let (Value::Object(fields), Some(token)) = (&mut value, execution_token) {
            fields.insert("executionToken".to_string(), Value::String(token));
        }
        if !self.state.include_full_paths {
            redaction::redact_paths(&mut value);
        }
        CallToolResult::structured(value)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MangoDiskServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "mangodisk-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MangoDisk disk analysis and cleanup. Scan tools are read-only. Execution tools \
                 are disabled unless the server was started with --enable-mutations; when enabled, \
                 run the matching scan first and pass its executionToken together with \
                 confirm: true. Paths are redacted unless the server was started with \
                 --include-full-paths."
                    .to_string(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mutations_fail_closed_until_enabled() {
        let state = AdapterState::new(false, false);
        let error = state
            .ensure_mutations_enabled()
            .expect_err("mutations must be rejected by default");
        let json = serde_json::to_value(error).expect("tool errors must serialize");
        assert_eq!(json["isError"], true);
        assert_eq!(
            json["structuredContent"]["error"]["code"],
            "mutationsDisabled"
        );

        let enabled = AdapterState::new(false, true);
        assert!(enabled.ensure_mutations_enabled().is_ok());
    }

    #[test]
    fn execute_calls_require_explicit_confirmation() {
        let error =
            AdapterState::ensure_confirmed(false).expect_err("confirm=false must be rejected");
        let json = serde_json::to_value(error).expect("tool errors must serialize");
        assert_eq!(
            json["structuredContent"]["error"]["code"],
            "confirmationRequired"
        );
        assert!(AdapterState::ensure_confirmed(true).is_ok());
    }

    #[test]
    fn tokens_are_only_issued_when_mutations_are_enabled() {
        let disabled = AdapterState::new(false, false);
        assert!(disabled
            .issue_token(MutationDomain::Cleanup, json!({}))
            .is_none());

        let enabled = AdapterState::new(false, true);
        let token = enabled
            .issue_token(MutationDomain::Cleanup, json!({ "ruleIds": ["a.b"] }))
            .expect("enabled mutations issue a token");
        let snapshot = enabled
            .take_token(&token, MutationDomain::Cleanup)
            .expect("the issued token must be consumable");
        assert_eq!(snapshot["ruleIds"][0], "a.b");
    }

    #[test]
    fn responses_redact_paths_unless_full_paths_are_enabled() {
        let server = MangoDiskServer::new(Arc::new(AdapterState::new(false, false)));
        let result = server.respond(&json!({ "root": "/Users/alice/demo", "scanId": 7 }), None);
        let structured = result
            .structured_content
            .expect("structured content must be present");
        assert!(!structured.to_string().contains("/Users/alice"));
        assert_eq!(structured["scanId"], 7);

        let open = MangoDiskServer::new(Arc::new(AdapterState::new(true, false)));
        let result = open.respond(&json!({ "root": "/Users/alice/demo" }), None);
        assert_eq!(
            result.structured_content.expect("structured content")["root"],
            "/Users/alice/demo"
        );
    }
}
