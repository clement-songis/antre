//! Spend limits for `sampling/createMessage` and the per-call context that
//! ties a server request to the chat session that triggered it.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::Serialize;

pub type SamplingConfig = moltis_config::schema::McpSamplingConfig;

/// Key used for requests that arrive outside any chat session.
const NO_SESSION: &str = "*";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingUsage {
    pub requests: u32,
    pub tokens: u64,
}

/// Why a sampling request was refused by the budget.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    #[error("sampling request limit reached ({limit} per session)")]
    Requests { limit: u32 },
    #[error("sampling token limit reached ({limit} per session)")]
    Tokens { limit: u64 },
}

/// In-memory sampling usage per (server, session).
#[derive(Default)]
pub struct SamplingBudget {
    usage: Mutex<HashMap<(String, String), SamplingUsage>>,
}

impl SamplingBudget {
    fn key(server: &str, session_key: Option<&str>) -> (String, String) {
        (
            server.to_string(),
            session_key.unwrap_or(NO_SESSION).to_string(),
        )
    }

    /// Count a request against the budget and return the `maxTokens` it may
    /// use: the smallest of the request, the per-request cap, and what is
    /// left of the session budget.
    pub fn reserve(
        &self,
        server: &str,
        session_key: Option<&str>,
        config: &SamplingConfig,
        requested_max_tokens: u32,
    ) -> Result<u32, BudgetError> {
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let entry = usage.entry(Self::key(server, session_key)).or_default();
        if entry.requests >= config.max_requests_per_session {
            return Err(BudgetError::Requests {
                limit: config.max_requests_per_session,
            });
        }
        let remaining = config.max_tokens_per_session.saturating_sub(entry.tokens);
        let max_tokens =
            u64::from(requested_max_tokens.min(config.max_tokens_per_request)).min(remaining);
        if max_tokens == 0 {
            return Err(BudgetError::Tokens {
                limit: config.max_tokens_per_session,
            });
        }
        entry.requests += 1;
        Ok(u32::try_from(max_tokens).unwrap_or(u32::MAX))
    }

    /// Record tokens actually used by a completed request.
    pub fn record(&self, server: &str, session_key: Option<&str>, tokens: u64) {
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let entry = usage.entry(Self::key(server, session_key)).or_default();
        entry.tokens = entry.tokens.saturating_add(tokens);
    }

    #[must_use]
    pub fn usage(&self, server: &str, session_key: Option<&str>) -> SamplingUsage {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::key(server, session_key))
            .copied()
            .unwrap_or_default()
    }
}

/// Who triggered the tool call during which a server sent a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallContext {
    pub session_key: Option<String>,
}

/// Active tool calls per server. Server requests (sampling, elicitation)
/// carry no session, so they are attributed to the most recent active call
/// on the same server.
#[derive(Clone, Default)]
pub struct CallContexts {
    active: Arc<Mutex<HashMap<String, Vec<(u64, CallContext)>>>>,
    next_id: Arc<AtomicU64>,
}

/// Removes its context when the tool call ends.
pub struct CallContextGuard {
    contexts: CallContexts,
    server: String,
    id: u64,
}

impl Drop for CallContextGuard {
    fn drop(&mut self) {
        let mut active = self
            .contexts
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(stack) = active.get_mut(&self.server) {
            stack.retain(|(id, _)| *id != self.id);
            if stack.is_empty() {
                active.remove(&self.server);
            }
        }
    }
}

impl CallContexts {
    #[must_use]
    pub fn enter(&self, server: &str, context: CallContext) -> CallContextGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(server.to_string())
            .or_default()
            .push((id, context));
        CallContextGuard {
            contexts: self.clone(),
            server: server.to_string(),
            id,
        }
    }

    #[must_use]
    pub fn current(&self, server: &str) -> Option<CallContext> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(server)
            .and_then(|stack| stack.last())
            .map(|(_, context)| context.clone())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SamplingConfig {
        SamplingConfig {
            enabled: true,
            max_tokens_per_request: 100,
            max_requests_per_session: 2,
            max_tokens_per_session: 150,
            model: None,
        }
    }

    #[test]
    fn reserve_clamps_and_counts_requests() {
        let budget = SamplingBudget::default();
        assert_eq!(budget.reserve("s", Some("a"), &config(), 500).unwrap(), 100);
        budget.record("s", Some("a"), 120);
        assert_eq!(budget.reserve("s", Some("a"), &config(), 500).unwrap(), 30);
        assert_eq!(
            budget.reserve("s", Some("a"), &config(), 10),
            Err(BudgetError::Requests { limit: 2 })
        );
        assert_eq!(budget.usage("s", Some("a")), SamplingUsage {
            requests: 2,
            tokens: 120
        });
        // Other sessions and servers have their own budget.
        assert_eq!(budget.reserve("s", Some("b"), &config(), 10).unwrap(), 10);
        assert_eq!(budget.reserve("t", None, &config(), 10).unwrap(), 10);
    }

    #[test]
    fn reserve_refuses_when_tokens_exhausted() {
        let budget = SamplingBudget::default();
        budget.record("s", None, 150);
        assert_eq!(
            budget.reserve("s", None, &config(), 10),
            Err(BudgetError::Tokens { limit: 150 })
        );
        assert_eq!(budget.usage("s", None).requests, 0);
    }

    #[test]
    fn call_contexts_track_latest_active_call() {
        let contexts = CallContexts::default();
        assert!(contexts.current("srv").is_none());
        let first = contexts.enter("srv", CallContext {
            session_key: Some("one".into()),
        });
        {
            let _second = contexts.enter("srv", CallContext {
                session_key: Some("two".into()),
            });
            assert_eq!(
                contexts.current("srv").unwrap().session_key.as_deref(),
                Some("two")
            );
        }
        assert_eq!(
            contexts.current("srv").unwrap().session_key.as_deref(),
            Some("one")
        );
        drop(first);
        assert!(contexts.current("srv").is_none());
    }
}
