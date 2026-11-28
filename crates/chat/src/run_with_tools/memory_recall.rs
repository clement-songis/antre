#[cfg(feature = "metrics")]
use std::time::Instant;

/// Format recalled context, escaping XML metacharacters to prevent prompt injection.
pub(crate) fn format_recalled_context(results: &[antre_memory::search::SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<recalled_context>\nRecalled from long-term memory as potentially relevant:\n\n",
    );
    for r in results {
        // Truncate long chunks to avoid prompt bloat.
        let text = if r.text.len() > 300 {
            format!("{}…", &r.text[..r.text.floor_char_boundary(300)])
        } else {
            r.text.clone()
        };
        // Escape XML metacharacters to prevent injection through memory content.
        let safe_path = escape_xml(&r.path);
        let safe_text = escape_xml(&text.replace('\n', " "));
        out.push_str(&format!("- [{safe_path}] {safe_text}\n"));
    }
    out.push_str("</recalled_context>");
    out
}

#[cfg(feature = "metrics")]
pub(super) fn record_prefetch_metric(status: &'static str, start: Instant) {
    use antre_metrics::{counter, histogram, labels, memory as mem_metrics};
    counter!(mem_metrics::PREFETCH_TOTAL, labels::STATUS => status).increment(1);
    histogram!(mem_metrics::PREFETCH_DURATION_SECONDS).record(start.elapsed().as_secs_f64());
}

/// Escape XML metacharacters that could break prompt structure.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Longest a turn waits for long-term memory before going on without it.
const PREFETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// After a timeout, turns skip the prefetch for this long, so a stuck memory
/// store or embedding service costs one wait, not one per turn.
const PREFETCH_PAUSE: std::time::Duration = std::time::Duration::from_secs(60);

/// What a bounded memory prefetch produced.
#[derive(Debug, PartialEq)]
pub(crate) enum Recall<T> {
    Found(Vec<T>),
    Failed(String),
    /// The search did not answer in time; prefetch is paused.
    TimedOut,
    /// A recent timeout paused prefetch; the search was not attempted.
    Paused,
}

/// Process-wide pause after a timed-out prefetch. Memory is one store for
/// every agent and session, so one stall must not slow every turn down.
#[derive(Default)]
pub(crate) struct RecallBreaker {
    paused_until: std::sync::Mutex<Option<tokio::time::Instant>>,
}

impl RecallBreaker {
    pub(crate) const fn new() -> Self {
        Self {
            paused_until: std::sync::Mutex::new(None),
        }
    }

    fn is_paused(&self, now: tokio::time::Instant) -> bool {
        let mut paused = self.paused_until.lock().unwrap_or_else(|e| e.into_inner());
        match *paused {
            Some(until) if until > now => true,
            Some(_) => {
                *paused = None;
                false
            },
            None => false,
        }
    }

    fn pause(&self, now: tokio::time::Instant) {
        *self.paused_until.lock().unwrap_or_else(|e| e.into_inner()) = Some(now + PREFETCH_PAUSE);
    }

    /// Run `search` with a deadline, skipping it while paused.
    pub(crate) async fn recall<T, E: std::fmt::Display>(
        &self,
        search: impl Future<Output = Result<Vec<T>, E>>,
    ) -> Recall<T> {
        if self.is_paused(tokio::time::Instant::now()) {
            return Recall::Paused;
        }
        match tokio::time::timeout(PREFETCH_TIMEOUT, search).await {
            Ok(Ok(results)) => Recall::Found(results),
            Ok(Err(error)) => Recall::Failed(error.to_string()),
            Err(_) => {
                self.pause(tokio::time::Instant::now());
                Recall::TimedOut
            },
        }
    }
}

pub(crate) static PREFETCH_BREAKER: RecallBreaker = RecallBreaker::new();

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod breaker_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_stuck_search_times_out_then_pauses_prefetch() {
        let breaker = RecallBreaker::new();
        let stuck = breaker
            .recall(std::future::pending::<Result<Vec<u8>, String>>())
            .await;
        assert_eq!(stuck, Recall::TimedOut);

        // While paused the search is not even started.
        let skipped = breaker
            .recall(async { panic!("must not run while paused") as Result<Vec<u8>, String> })
            .await;
        assert_eq!(skipped, Recall::Paused);

        tokio::time::advance(PREFETCH_PAUSE).await;
        let resumed = breaker.recall(async { Ok::<_, String>(vec![1u8]) }).await;
        assert_eq!(resumed, Recall::Found(vec![1]));
    }

    #[tokio::test]
    async fn errors_are_reported_without_pausing() {
        let breaker = RecallBreaker::new();
        let failed = breaker
            .recall(async { Err::<Vec<u8>, _>("database is locked") })
            .await;
        assert_eq!(failed, Recall::Failed("database is locked".into()));
        let found = breaker
            .recall(async { Ok::<_, String>(Vec::<u8>::new()) })
            .await;
        assert_eq!(found, Recall::Found(Vec::new()));
    }
}
