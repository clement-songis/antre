//! Shared HTTP client and retry header helpers for LLM providers.

/// Keep the transport cause when crossing the string-valued stream error
/// boundary, without leaking credentials or query parameters from the URL.
pub(crate) fn format_transport_error(error: reqwest::Error) -> String {
    let timeout = error.is_timeout();
    let connect = error.is_connect();
    format!(
        "provider transport error (timeout={timeout}, connect={connect}): {:#}",
        anyhow::Error::new(error.without_url())
    )
}

static SHARED_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Initialize the shared provider HTTP client with optional upstream proxy.
///
/// Call once at gateway startup; subsequent calls are no-ops.
pub fn init_shared_http_client(proxy_url: Option<&str>) {
    let _ = SHARED_CLIENT.set(antre_common::http_client::build_http_client(proxy_url));
}

/// Shared HTTP client for LLM providers.
///
/// All providers that don't need custom redirect/proxy settings should
/// reuse this client to share connection pools, DNS cache, and TLS sessions.
///
/// Falls back to a client with default headers (including User-Agent)
/// if [`init_shared_http_client`] was never called (e.g. in tests).
pub fn shared_http_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(antre_common::http_client::build_default_http_client)
}

/// Parse `Retry-After` header as milliseconds.
///
/// `Retry-After` may be either delta-seconds or an HTTP date. We currently
/// consume delta-seconds, which is what providers typically return for 429.
pub(crate) fn retry_after_ms_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?;
    let text = value.to_str().ok()?.trim();
    let seconds = text.parse::<u64>().ok()?;
    seconds.checked_mul(1_000)
}

/// Attach an explicit retry hint marker consumable by runner retry logic.
pub(crate) fn with_retry_after_marker(base: String, retry_after_ms: Option<u64>) -> String {
    match retry_after_ms {
        Some(ms) => format!("{base} (retry_after_ms={ms})"),
        None => base,
    }
}

/// Timezone in which `provider` prints offset-less reset times for
/// `model`, from the model catalog (`rate_limit_tz`); UTC when unknown.
fn rate_limit_tz(provider: &str, model: &str) -> time::UtcOffset {
    antre_model_catalog::global::read(|catalog| catalog.resolve(provider, model, None))
        .spec
        .rate_limit_tz
        .map_or(time::UtcOffset::UTC, |tz| tz.0)
}

/// Build the typed failure for an HTTP error. `message` is what users and
/// logs see; it keeps the legacy `retry_after_ms` marker for callers that
/// still read it.
pub(crate) fn http_failure(
    provider: &str,
    model: &str,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &str,
    message: String,
) -> antre_agents::provider_error::ProviderFailure {
    let message = with_retry_after_marker(message, retry_after_ms_from_headers(headers));
    antre_agents::provider_error::reset::classify_http(
        antre_agents::provider_error::reset::HttpFailure {
            provider,
            status,
            headers,
            body,
            tz: rate_limit_tz(provider, model),
            message: Some(message),
        },
        time::OffsetDateTime::now_utc(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        std::{error::Error as _, time::Duration},
        tokio::io::AsyncReadExt,
    };

    #[test]
    fn http_failure_uses_catalog_timezone_for_glm() {
        use antre_agents::provider_error::{ProviderError, QuotaWindow};
        let body = r#"{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2099-01-01 08:00:00"}}"#;
        let failure = http_failure(
            "zai-code",
            "glm-5.3",
            429,
            &reqwest::header::HeaderMap::new(),
            body,
            format!("HTTP 429: {body}"),
        );
        // Far-future resets are discarded, but the window and kind come through.
        assert_eq!(failure.error, ProviderError::QuotaExhausted {
            resets_at: None,
            window: QuotaWindow::Rolling5h,
        });
        assert_eq!(failure.provider, "zai-code");
        assert_eq!(
            rate_limit_tz("zai-code", "glm-5.3"),
            time::macros::offset!(+8)
        );
        assert_eq!(rate_limit_tz("openai", "gpt-6.2-sol"), time::UtcOffset::UTC);
    }

    #[test]
    fn http_failure_keeps_retry_marker_in_message() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "retry-after",
            reqwest::header::HeaderValue::from_static("4"),
        );
        let failure = http_failure(
            "openai",
            "gpt-6.2-sol",
            429,
            &headers,
            "slow down",
            "HTTP 429: slow down".into(),
        );
        assert_eq!(failure.message, "HTTP 429: slow down (retry_after_ms=4000)");
        assert!(matches!(
            failure.error,
            antre_agents::provider_error::ProviderError::RateLimited { .. }
        ));
    }

    #[tokio::test]
    async fn transport_error_keeps_cause_without_request_url() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/private-path?token=secret-value",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = socket.read(&mut request).await.unwrap();
            assert!(received > 0, "client must start sending its request");
            // Close without a response to produce a genuine transport error.
        });
        let error = reqwest::Client::new()
            .post(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap_err();
        let cause = error.source().unwrap().to_string();
        let flags = format!(
            "timeout={}, connect={}",
            error.is_timeout(),
            error.is_connect()
        );
        let message = format_transport_error(error);
        assert!(message.contains(&cause));
        assert!(message.contains(&flags));
        assert!(!message.contains("secret-value"));
        assert!(!message.contains("private-path"));
        server.await.unwrap();
    }
}
