//! Typed bridge errors with stable machine-readable codes.
//!
//! The `diagnostic` string is for logs and must never carry raw filesystem
//! paths, prompt text, or provider stderr; those may contain private user
//! data. Adapters should branch on [`AcpErrorCode`], not on message text.

use std::{error::Error, fmt};

/// Stable error codes for the ACP bridge.
///
/// These codes cross process boundaries (Tauri commands, UI events), so they
/// are serialized as snake_case strings and must never be renamed without a
/// schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpErrorCode {
    /// The requested provider id is not in the bridge catalog.
    ProviderUnknown,
    /// The provider binary was not found on `PATH` (or the resolved path does
    /// not exist), so the provider cannot be launched.
    ProviderUnavailable,
    /// The provider process could not be spawned even though the binary was
    /// resolved (for example, a permission error).
    SpawnFailed,
    /// The ACP `initialize` or `session/new` handshake did not complete.
    HandshakeFailed,
    /// The connection to the provider was lost while the session was active.
    SessionLost,
    /// The provider process exited unexpectedly.
    ProviderExited,
    /// The agent rejected or failed a prompt turn (the session itself may
    /// still be usable).
    PromptFailed,
    /// An operation did not complete within its configured timeout.
    Timeout,
    /// A permission resolution referenced a request that is no longer pending
    /// (already resolved, timed out, or belonging to a closed session).
    PermissionNotPending,
}

impl AcpErrorCode {
    /// Stable snake_case code string for logs and wire payloads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderUnknown => "provider_unknown",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::SpawnFailed => "spawn_failed",
            Self::HandshakeFailed => "handshake_failed",
            Self::SessionLost => "session_lost",
            Self::ProviderExited => "provider_exited",
            Self::PromptFailed => "prompt_failed",
            Self::Timeout => "timeout",
            Self::PermissionNotPending => "permission_not_pending",
        }
    }
}

/// Error type returned by all bridge operations.
#[derive(Debug)]
pub struct AcpError {
    code: AcpErrorCode,
    diagnostic: String,
}

impl AcpError {
    pub fn new(code: AcpErrorCode, diagnostic: impl Into<String>) -> Self {
        Self {
            code,
            diagnostic: diagnostic.into(),
        }
    }

    pub fn code(&self) -> AcpErrorCode {
        self.code
    }

    /// Privacy-safe diagnostic text. Built only from fixed strings, provider
    /// ids, exit statuses, and typed reason codes; never from raw paths,
    /// prompt contents, or provider stderr.
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    pub fn provider_unknown(provider_id: impl Into<String>) -> Self {
        Self::new(
            AcpErrorCode::ProviderUnknown,
            format!("provider id is not in the catalog: {}", provider_id.into()),
        )
    }

    pub fn provider_unavailable(provider_id: &str) -> Self {
        Self::new(
            AcpErrorCode::ProviderUnavailable,
            format!("provider binary was not found on PATH: {provider_id}"),
        )
    }

    pub fn spawn_failed(diagnostic: impl Into<String>) -> Self {
        Self::new(AcpErrorCode::SpawnFailed, diagnostic)
    }

    pub fn handshake_failed(diagnostic: impl Into<String>) -> Self {
        Self::new(AcpErrorCode::HandshakeFailed, diagnostic)
    }

    pub fn session_lost(diagnostic: impl Into<String>) -> Self {
        Self::new(AcpErrorCode::SessionLost, diagnostic)
    }

    pub fn provider_exited(diagnostic: impl Into<String>) -> Self {
        Self::new(AcpErrorCode::ProviderExited, diagnostic)
    }

    pub fn timeout(diagnostic: impl Into<String>) -> Self {
        Self::new(AcpErrorCode::Timeout, diagnostic)
    }

    pub fn permission_not_pending(request_id: u64) -> Self {
        Self::new(
            AcpErrorCode::PermissionNotPending,
            format!("permission request {request_id} is not pending"),
        )
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.diagnostic)
    }
}

impl Error for AcpError {}

pub type AcpResult<T> = Result<T, AcpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_have_stable_snake_case_strings() {
        assert_eq!(AcpErrorCode::ProviderUnknown.as_str(), "provider_unknown");
        assert_eq!(
            AcpErrorCode::ProviderUnavailable.as_str(),
            "provider_unavailable"
        );
        assert_eq!(AcpErrorCode::SpawnFailed.as_str(), "spawn_failed");
        assert_eq!(AcpErrorCode::HandshakeFailed.as_str(), "handshake_failed");
        assert_eq!(AcpErrorCode::SessionLost.as_str(), "session_lost");
        assert_eq!(AcpErrorCode::ProviderExited.as_str(), "provider_exited");
        assert_eq!(AcpErrorCode::PromptFailed.as_str(), "prompt_failed");
        assert_eq!(AcpErrorCode::Timeout.as_str(), "timeout");
        assert_eq!(
            AcpErrorCode::PermissionNotPending.as_str(),
            "permission_not_pending"
        );
    }

    #[test]
    fn error_codes_serialize_as_snake_case() {
        let json = serde_json::to_string(&AcpErrorCode::HandshakeFailed).unwrap();
        assert_eq!(json, "\"handshake_failed\"");
    }

    #[test]
    fn display_includes_code_and_diagnostic() {
        let error = AcpError::timeout("handshake timed out");
        assert_eq!(error.to_string(), "timeout: handshake timed out");
    }
}
