use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde_json::Value;

/// Default lifetime of an execution token. Long enough for an agent to review
/// a preview, short enough that a stale snapshot cannot be executed later.
pub(crate) const EXECUTION_TOKEN_TTL: Duration = Duration::from_secs(600);

/// Domains that issue execution tokens. The tag binds a token to exactly one
/// execute tool so a cleanup preview can never authorize a permanent delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MutationDomain {
    Cleanup,
    PermanentDelete,
    ApplicationUninstall,
    ApplicationLeftovers,
    Startup,
    SystemSettings,
    ProcessEnd,
}

impl MutationDomain {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cleanup => "cleanup",
            Self::PermanentDelete => "permanentDelete",
            Self::ApplicationUninstall => "applicationUninstall",
            Self::ApplicationLeftovers => "applicationLeftovers",
            Self::Startup => "startup",
            Self::SystemSettings => "systemSettings",
            Self::ProcessEnd => "processEnd",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionTokenError {
    Unknown,
    Expired,
    DomainMismatch,
}

impl ExecutionTokenError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Unknown => super::errors::TOKEN_UNKNOWN,
            Self::Expired => super::errors::TOKEN_EXPIRED,
            Self::DomainMismatch => super::errors::TOKEN_DOMAIN_MISMATCH,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unknown => "the execution token is unknown or was already used",
            Self::Expired => {
                "the execution token has expired; run the matching scan again to preview fresh state"
            }
            Self::DomainMismatch => {
                "the execution token belongs to a different operation; pass the token returned by the matching scan"
            }
        }
    }
}

struct IssuedToken {
    domain: MutationDomain,
    snapshot: Value,
    expires_at: Instant,
}

/// In-memory store that turns a preview result into a single-use execution
/// grant. Tokens are opaque, expire quickly, and are removed on use so a
/// captured token cannot be replayed. Nothing is persisted.
pub(crate) struct ExecutionTokenStore {
    ttl: Duration,
    tokens: Mutex<HashMap<String, IssuedToken>>,
}

impl ExecutionTokenStore {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            tokens: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn issue(&self, domain: MutationDomain, snapshot: Value) -> String {
        let token = format!("mdx_{}", uuid::Uuid::new_v4().simple());
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        purge_expired(&mut tokens, Instant::now());
        tokens.insert(
            token.clone(),
            IssuedToken {
                domain,
                snapshot,
                expires_at: Instant::now() + self.ttl,
            },
        );
        token
    }

    /// Consumes a token for the given domain. A successful take removes the
    /// entry, which makes every token single-use. A domain mismatch leaves the
    /// token intact so the correct tool can still consume it, but returns an
    /// error before any mutation can be considered.
    pub(crate) fn take(
        &self,
        token: &str,
        domain: MutationDomain,
    ) -> Result<Value, ExecutionTokenError> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(issued) = tokens.get(token) else {
            return Err(ExecutionTokenError::Unknown);
        };
        if issued.domain != domain {
            log::info!(
                "execution_token_rejected reason=domain_mismatch expected={} actual={}",
                domain.as_str(),
                issued.domain.as_str()
            );
            return Err(ExecutionTokenError::DomainMismatch);
        }
        if Instant::now() >= issued.expires_at {
            tokens.remove(token);
            return Err(ExecutionTokenError::Expired);
        }
        let issued = tokens.remove(token).expect("the token was present above");
        Ok(issued.snapshot)
    }
}

fn purge_expired(tokens: &mut HashMap<String, IssuedToken>, now: Instant) {
    tokens.retain(|_, issued| issued.expires_at > now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> ExecutionTokenStore {
        ExecutionTokenStore::new(EXECUTION_TOKEN_TTL)
    }

    #[test]
    fn issued_token_returns_its_snapshot_once() {
        let store = store();
        let token = store.issue(MutationDomain::Cleanup, json!({ "ruleIds": ["a.b"] }));

        let snapshot = store
            .take(&token, MutationDomain::Cleanup)
            .expect("a fresh token must be accepted");

        assert_eq!(snapshot["ruleIds"][0], "a.b");
    }

    #[test]
    fn tokens_are_single_use() {
        let store = store();
        let token = store.issue(MutationDomain::Cleanup, json!({}));
        store
            .take(&token, MutationDomain::Cleanup)
            .expect("the first take consumes the token");

        let error = store
            .take(&token, MutationDomain::Cleanup)
            .expect_err("a consumed token must be rejected");

        assert_eq!(error, ExecutionTokenError::Unknown);
    }

    #[test]
    fn expired_tokens_are_rejected() {
        let store = ExecutionTokenStore::new(Duration::ZERO);
        let token = store.issue(MutationDomain::Startup, json!({}));

        let error = store
            .take(&token, MutationDomain::Startup)
            .expect_err("a zero-TTL token must expire immediately");

        assert_eq!(error, ExecutionTokenError::Expired);
        assert_eq!(error.code(), "tokenExpired");
    }

    #[test]
    fn tokens_do_not_cross_domains_and_survive_a_wrong_domain() {
        let store = store();
        let token = store.issue(MutationDomain::Cleanup, json!({}));

        let error = store
            .take(&token, MutationDomain::PermanentDelete)
            .expect_err("a cleanup token must not authorize a permanent delete");

        assert_eq!(error, ExecutionTokenError::DomainMismatch);
        assert!(
            store.take(&token, MutationDomain::Cleanup).is_ok(),
            "a mismatch must not consume the token"
        );
    }

    #[test]
    fn process_end_tokens_follow_the_same_domain_rules() {
        let store = store();
        let token = store.issue(
            MutationDomain::ProcessEnd,
            json!({ "processes": [{ "pid": 42, "startedAtMs": 7 }] }),
        );

        let error = store
            .take(&token, MutationDomain::Cleanup)
            .expect_err("a process end token must not authorize a cleanup");
        assert_eq!(error, ExecutionTokenError::DomainMismatch);

        let snapshot = store
            .take(&token, MutationDomain::ProcessEnd)
            .expect("the token must be consumable in its own domain");
        assert_eq!(snapshot["processes"][0]["pid"], 42);
        assert_eq!(MutationDomain::ProcessEnd.as_str(), "processEnd");
    }

    #[test]
    fn unknown_tokens_are_rejected() {
        let store = store();

        let error = store
            .take("mdx_does_not_exist", MutationDomain::Cleanup)
            .expect_err("an unknown token must be rejected");

        assert_eq!(error, ExecutionTokenError::Unknown);
        assert_eq!(error.code(), "tokenUnknown");
    }
}
