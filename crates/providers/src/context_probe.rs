//! Opt-in context-window probe for OpenAI-compatible endpoints that publish
//! no window: a request asking for an absurd `max_tokens` is rejected before
//! inference with "maximum context length is N". Replicas behind one
//! endpoint can report different windows, so the probe repeats and keeps
//! the largest.

use std::time::Duration;

use secrecy::ExposeSecret;

/// Far beyond any real window, so a validating server always rejects it.
const PROBE_MAX_TOKENS: u64 = 100_000_000;
const PROBE_ATTEMPTS: usize = 3;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Probe one model; `None` when the server never states a window (or,
/// unexpectedly, accepts the request).
pub async fn probe_context_window(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &secrecy::Secret<String>,
    headers: reqwest::header::HeaderMap,
    model: &str,
) -> Option<u32> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": PROBE_MAX_TOKENS,
        "stream": false,
    });
    let mut best = None;
    for _ in 0..PROBE_ATTEMPTS {
        let mut request = client
            .post(&url)
            .timeout(PROBE_TIMEOUT)
            .headers(headers.clone())
            .json(&body);
        if !api_key.expose_secret().trim().is_empty() {
            request = request.bearer_auth(api_key.expose_secret().trim());
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(model, %error, "context probe request failed");
                continue;
            },
        };
        if response.status().is_success() {
            tracing::warn!(
                model,
                "context probe: server accepted an oversized request; cannot probe"
            );
            return best;
        }
        let text = response.text().await.unwrap_or_default();
        if let Some(limit) = moltis_agents::provider_error::reset::context_limit(&text) {
            best = Some(best.map_or(limit, |current: u32| current.max(limit)));
        }
    }
    best
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_the_largest_window_across_replicas() {
        let mut server = mockito::Server::new_async().await;
        let small = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer k")
            .match_header("user-agent", "probe-test")
            .with_status(400)
            .with_body(r#"{"error":{"message":"This model's maximum context length is 32768 tokens. However, you requested 100000004 tokens."}}"#)
            .expect(2)
            .create_async()
            .await;
        let large = server
            .mock("POST", "/v1/chat/completions")
            .with_status(400)
            .with_body("maximum context length of 131072")
            .expect(1)
            .create_async()
            .await;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "user-agent",
            reqwest::header::HeaderValue::from_static("probe-test"),
        );
        let window = probe_context_window(
            &reqwest::Client::new(),
            &format!("{}/v1/", server.url()),
            &secrecy::Secret::new("k".into()),
            headers,
            "m",
        )
        .await;
        small.assert_async().await;
        large.assert_async().await;
        assert_eq!(window, Some(131_072));
    }

    #[tokio::test]
    async fn accepted_or_silent_servers_give_no_window() {
        let mut server = mockito::Server::new_async().await;
        let _ok = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let window = probe_context_window(
            &reqwest::Client::new(),
            &server.url(),
            &secrecy::Secret::new(String::new()),
            reqwest::header::HeaderMap::new(),
            "m",
        )
        .await;
        assert_eq!(window, None);
    }
}
