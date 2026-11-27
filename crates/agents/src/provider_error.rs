//! Typed provider errors.
//!
//! Each provider builds a [`ProviderFailure`] from the HTTP status, headers
//! and body of a failed request (see [`reset`] for reset-time parsing). It
//! travels inside `anyhow::Error` and in [`StreamEvent::Failure`]
//! (crate::model::StreamEvent), so the failover chain, the retry loop and the
//! chat layer match on the variant instead of searching the message text.

pub mod reset;

use std::time::Duration;

use time::OffsetDateTime;

/// What ran out when a provider rate-limits a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitScope {
    Requests,
    InputTokens,
    OutputTokens,
    Tokens,
    Concurrent,
    Unknown,
}

/// The period a quota covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuotaWindow {
    /// Subscription windows such as Claude, Codex or GLM's five hours.
    Rolling5h,
    Daily,
    Weekly,
    Monthly,
    Unknown,
}

impl QuotaWindow {
    /// Classify a window length in minutes, as Codex reports it.
    #[must_use]
    pub fn from_minutes(minutes: u64) -> Self {
        match minutes {
            240..=360 => Self::Rolling5h,
            1_380..=1_500 => Self::Daily,
            9_900..=10_300 => Self::Weekly,
            40_000..=45_000 => Self::Monthly,
            _ => Self::Unknown,
        }
    }
}

/// Why a provider request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Short-term throttling; retrying after `retry_after` usually works.
    RateLimited {
        retry_after: Option<Duration>,
        scope: LimitScope,
    },
    /// A plan or account quota is used up until `resets_at` (UTC).
    QuotaExhausted {
        resets_at: Option<OffsetDateTime>,
        window: QuotaWindow,
    },
    ContextWindowExceeded {
        limit: Option<u32>,
    },
    Auth,
    /// Out of credit or payment problem; waiting does not help.
    Billing,
    Server {
        status: u16,
    },
    Network,
    /// The request itself is wrong; it would fail on every provider.
    InvalidRequest,
    Other,
}

impl ProviderError {
    /// When the request can be retried, if the provider said so.
    #[must_use]
    pub fn retry_instant(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        match self {
            Self::RateLimited {
                retry_after: Some(delay),
                ..
            } => time::Duration::try_from(*delay).ok().map(|d| now + d),
            Self::QuotaExhausted {
                resets_at: Some(at),
                ..
            } => Some(*at),
            _ => None,
        }
    }

    /// Whether waiting (rather than changing the request) can fix it.
    #[must_use]
    pub fn is_temporary_limit(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::QuotaExhausted { .. })
    }
}

/// A failed provider request: the typed cause plus the provider's own
/// message, kept for logs and for the error card shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub error: ProviderError,
    pub provider: String,
    pub message: String,
}

impl ProviderFailure {
    #[must_use]
    pub fn new(
        error: ProviderError,
        provider: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            error,
            provider: provider.into(),
            message: message.into(),
        }
    }

    /// The typed failure carried by an `anyhow::Error`, if any.
    #[must_use]
    pub fn find(error: &anyhow::Error) -> Option<&Self> {
        error.chain().find_map(|cause| cause.downcast_ref::<Self>())
    }
}

impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderFailure {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn failure_survives_anyhow_context() {
        let failure = ProviderFailure::new(
            ProviderError::QuotaExhausted {
                resets_at: None,
                window: QuotaWindow::Weekly,
            },
            "zai-code",
            "Usage limit reached",
        );
        let error = anyhow::Error::new(failure.clone()).context("run failed");
        assert_eq!(ProviderFailure::find(&error), Some(&failure));
        assert_eq!(failure.to_string(), "Usage limit reached");
        assert!(ProviderFailure::find(&anyhow::anyhow!("plain")).is_none());
    }

    #[test]
    fn retry_instant_and_temporary_limits() {
        let now = time::macros::datetime!(2026-11-27 12:00 UTC);
        let rate = ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
            scope: LimitScope::Requests,
        };
        assert_eq!(
            rate.retry_instant(now),
            Some(time::macros::datetime!(2026-11-27 12:00:30 UTC))
        );
        assert!(rate.is_temporary_limit());
        let quota = ProviderError::QuotaExhausted {
            resets_at: Some(time::macros::datetime!(2026-11-27 18:00 UTC)),
            window: QuotaWindow::Rolling5h,
        };
        assert_eq!(
            quota.retry_instant(now),
            Some(time::macros::datetime!(2026-11-27 18:00 UTC))
        );
        assert_eq!(ProviderError::Billing.retry_instant(now), None);
        assert!(!ProviderError::Auth.is_temporary_limit());
    }

    #[test]
    fn quota_window_from_minutes() {
        assert_eq!(QuotaWindow::from_minutes(300), QuotaWindow::Rolling5h);
        assert_eq!(QuotaWindow::from_minutes(10_080), QuotaWindow::Weekly);
        assert_eq!(QuotaWindow::from_minutes(1_440), QuotaWindow::Daily);
        assert_eq!(QuotaWindow::from_minutes(43_200), QuotaWindow::Monthly);
        assert_eq!(QuotaWindow::from_minutes(17), QuotaWindow::Unknown);
    }
}
