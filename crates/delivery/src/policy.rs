//! Retry timing.

use std::time::Duration;

use antre_config::schema::DeliveryConfig;

/// Fraction of the delay added or removed at random, so deliveries that
/// failed together do not all retry in the same instant.
const JITTER_RATIO: f64 = 0.2;

#[derive(Debug, Clone, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    /// Longest a single send may take.
    pub attempt_timeout: Duration,
    /// How long a claimed delivery stays `sending` before another worker may
    /// take it back. Longer than the attempt timeout, so a slow send is never
    /// sent twice by two workers.
    pub lease: Duration,
    /// How long delivered and failed records are kept.
    pub retention: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::from(&DeliveryConfig::default())
    }
}

impl From<&DeliveryConfig> for RetryPolicy {
    fn from(config: &DeliveryConfig) -> Self {
        let initial_delay = Duration::from_secs(config.initial_retry_secs.max(1));
        let attempt_timeout = Duration::from_secs(config.attempt_timeout_secs.max(1));
        Self {
            max_attempts: config.max_attempts.max(1),
            initial_delay,
            max_delay: Duration::from_secs(config.max_retry_secs).max(initial_delay),
            attempt_timeout,
            lease: attempt_timeout * 2,
            retention: Duration::try_from(time::Duration::days(i64::from(config.retention_days)))
                .unwrap_or_default(),
        }
    }
}

impl RetryPolicy {
    /// Delay before the retry that follows `attempts` failed attempts,
    /// without jitter: `initial * 2^(attempts-1)`, capped.
    #[must_use]
    pub fn base_delay(&self, attempts: u32) -> Duration {
        let exponent = attempts.saturating_sub(1).min(30);
        self.initial_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay)
    }

    /// [`Self::base_delay`] with ±20 % jitter, still capped.
    #[must_use]
    pub fn retry_delay(&self, attempts: u32) -> Duration {
        use rand::RngExt as _;
        let base = self.base_delay(attempts);
        let factor = rand::rng().random_range((1.0 - JITTER_RATIO)..=(1.0 + JITTER_RATIO));
        base.mul_f64(factor).min(self.max_delay)
    }

    /// `true` when another attempt is allowed after `attempts` failed ones.
    #[must_use]
    pub fn may_retry(&self, attempts: u32) -> bool {
        attempts < self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy::from(&DeliveryConfig {
            max_attempts: 4,
            initial_retry_secs: 2,
            max_retry_secs: 10,
            attempt_timeout_secs: 5,
            retention_days: 1,
            announce_background_tasks: true,
        })
    }

    #[test]
    fn delay_doubles_and_stops_at_the_ceiling() {
        let policy = policy();
        assert_eq!(policy.base_delay(1), Duration::from_secs(2));
        assert_eq!(policy.base_delay(2), Duration::from_secs(4));
        assert_eq!(policy.base_delay(3), Duration::from_secs(8));
        assert_eq!(policy.base_delay(4), Duration::from_secs(10));
        assert_eq!(policy.base_delay(u32::MAX), Duration::from_secs(10));
    }

    #[test]
    fn jitter_stays_within_twenty_percent_and_under_the_ceiling() {
        let policy = policy();
        for _ in 0..200 {
            let delay = policy.retry_delay(2);
            assert!(delay >= Duration::from_millis(3200), "{delay:?}");
            assert!(delay <= Duration::from_millis(4800), "{delay:?}");
            assert!(policy.retry_delay(9) <= Duration::from_secs(10));
        }
    }

    #[test]
    fn attempts_are_bounded_and_zero_values_are_clamped() {
        let policy = policy();
        assert!(policy.may_retry(3));
        assert!(!policy.may_retry(4));
        assert_eq!(policy.lease, Duration::from_secs(10));
        assert_eq!(
            policy.retention,
            Duration::try_from(time::Duration::days(1)).unwrap_or_default()
        );

        let clamped = RetryPolicy::from(&DeliveryConfig {
            max_attempts: 0,
            initial_retry_secs: 0,
            max_retry_secs: 0,
            attempt_timeout_secs: 0,
            retention_days: 0,
            announce_background_tasks: false,
        });
        assert_eq!(clamped.max_attempts, 1);
        assert_eq!(clamped.initial_delay, Duration::from_secs(1));
        assert_eq!(clamped.max_delay, Duration::from_secs(1));
        assert_eq!(clamped.attempt_timeout, Duration::from_secs(1));
    }
}
