//! Perplexity backend of the `web_search` tool: the Search API for direct
//! keys, chat completions for OpenRouter and other proxies.

use serde::Deserialize;

use {super::WebSearchTool, crate::Error};

/// Perplexity Search API response (subset).
#[derive(Debug, Deserialize)]
pub(super) struct PerplexitySearchResponse {
    #[serde(default)]
    results: Vec<PerplexitySearchResult>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PerplexitySearchResult {
    title: String,
    url: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    date: Option<String>,
}

/// How a Perplexity search request is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PerplexityRoute {
    /// Direct Perplexity key: `POST /search`. Sonar Chat Completions support
    /// ended on 2026-09-27, so direct keys never use the chat route.
    SearchApi,
    /// OpenRouter or another OpenAI-compatible proxy serving a Sonar model.
    ChatCompletions,
}

pub(super) fn perplexity_route(base_url: &str, api_key: &str) -> PerplexityRoute {
    if api_key.starts_with("pplx-") || base_url.contains("perplexity.ai") {
        PerplexityRoute::SearchApi
    } else {
        PerplexityRoute::ChatCompletions
    }
}

/// Perplexity (OpenAI-compatible) chat completion response (subset).
#[derive(Debug, Deserialize)]
pub(super) struct PerplexityResponse {
    choices: Vec<PerplexityChoice>,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PerplexityChoice {
    message: PerplexityMessage,
}

#[derive(Debug, Deserialize)]
pub(super) struct PerplexityMessage {
    content: String,
}

impl WebSearchTool {
    pub(super) async fn search_perplexity(
        &self,
        query: &str,
        count: u8,
        api_key: &str,
        base_url: &str,
        model: &str,
    ) -> crate::Result<serde_json::Value> {
        if api_key.trim().is_empty() {
            return Ok(serde_json::json!({
                "error": "Perplexity API key not configured",
                "hint": "Set PERPLEXITY_API_KEY or OPENROUTER_API_KEY environment variable, or tools.web.search.perplexity.api_key in config"
            }));
        }

        match perplexity_route(base_url, api_key) {
            PerplexityRoute::SearchApi => {
                self.search_perplexity_api(query, count, api_key, base_url)
                    .await
            },
            PerplexityRoute::ChatCompletions => {
                self.search_perplexity_chat(query, api_key, base_url, model)
                    .await
            },
        }
    }

    async fn search_perplexity_api(
        &self,
        query: &str,
        count: u8,
        api_key: &str,
        base_url: &str,
    ) -> crate::Result<serde_json::Value> {
        let client = crate::shared_http_client();
        let body = serde_json::json!({
            "query": query,
            "max_results": count.clamp(1, 20),
        });
        let resp = client
            .post(format!("{}/search", base_url.trim_end_matches('/')))
            .timeout(self.timeout)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::message(format!(
                "Perplexity Search API returned {status}: {text}"
            )));
        }

        let parsed: PerplexitySearchResponse = resp.json().await?;
        let results: Vec<serde_json::Value> = parsed
            .results
            .into_iter()
            .map(|result| {
                serde_json::json!({
                    "title": result.title,
                    "url": result.url,
                    "description": result.snippet,
                    "date": result.date,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "provider": "perplexity",
            "query": query,
            "results": results,
        }))
    }

    async fn search_perplexity_chat(
        &self,
        query: &str,
        api_key: &str,
        base_url: &str,
        model: &str,
    ) -> crate::Result<serde_json::Value> {
        let client = crate::shared_http_client();

        let body = serde_json::json!({
            "model": model,
            "messages": [
                {"role": "user", "content": query}
            ]
        });

        let resp = client
            .post(format!("{base_url}/chat/completions"))
            .timeout(self.timeout)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::message(format!(
                "Perplexity API returned {status}: {text}"
            )));
        }

        let pplx: PerplexityResponse = resp.json().await?;
        let answer = pplx
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(serde_json::json!({
            "provider": "perplexity",
            "query": query,
            "answer": answer,
            "citations": pplx.citations,
        }))
    }
}

pub(super) fn resolve_perplexity_base_url(
    base_url_override: Option<&str>,
    api_key: &str,
) -> String {
    if let Some(base_url) = base_url_override {
        return base_url.to_string();
    }
    if api_key.starts_with("pplx-") {
        "https://api.perplexity.ai".to_string()
    } else {
        // Assume OpenRouter for sk-or- or other keys.
        "https://openrouter.ai/api/v1".to_string()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn perplexity_tool() -> WebSearchTool {
        WebSearchTool::perplexity_for_tests()
    }

    #[tokio::test]
    async fn test_perplexity_missing_api_key_returns_hint() {
        let tool = perplexity_tool();
        let result = tool
            .search_perplexity("test", 5, "", "https://api.perplexity.ai", "sonar-pro")
            .await
            .unwrap();
        assert!(result["error"].as_str().unwrap().contains("not configured"));
        assert!(
            result["hint"]
                .as_str()
                .unwrap()
                .contains("PERPLEXITY_API_KEY")
        );
    }

    #[test]
    fn test_perplexity_response_parsing() {
        let json = serde_json::json!({
            "choices": [{"message": {"content": "Answer text"}}],
            "citations": ["https://example.com"]
        });
        let resp: PerplexityResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.choices[0].message.content, "Answer text");
        assert_eq!(resp.citations.len(), 1);
    }

    #[test]
    fn test_perplexity_route_uses_search_api_for_direct_keys() {
        assert_eq!(
            perplexity_route("https://api.perplexity.ai", "pplx-abc"),
            PerplexityRoute::SearchApi
        );
        assert_eq!(
            perplexity_route("https://api.perplexity.ai", "other"),
            PerplexityRoute::SearchApi
        );
        assert_eq!(
            perplexity_route("https://openrouter.ai/api/v1", "sk-or-abc"),
            PerplexityRoute::ChatCompletions
        );
        assert_eq!(
            perplexity_route("https://proxy.example/v1", "sk-proxy"),
            PerplexityRoute::ChatCompletions
        );
    }

    #[tokio::test]
    async fn test_perplexity_search_api_maps_results() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/search")
            .match_header("authorization", "Bearer pplx-test")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "query": "rust async",
                "max_results": 3
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "id": "abc",
                    "results": [
                        {"title": "Tokio", "url": "https://tokio.rs", "snippet": "Runtime", "date": "2026-11-01"},
                        {"title": "Async book", "url": "https://rust-lang.github.io/async-book/"}
                    ]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let tool = perplexity_tool();
        let result = tool
            .search_perplexity_api("rust async", 3, "pplx-test", &server.url())
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(result["provider"], "perplexity");
        assert_eq!(result["results"][0]["url"], "https://tokio.rs");
        assert_eq!(result["results"][0]["description"], "Runtime");
        assert_eq!(result["results"][1]["description"], "");
        assert!(result["results"][1]["date"].is_null());
    }

    #[tokio::test]
    async fn test_perplexity_search_api_error_status() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/search")
            .with_status(401)
            .with_body("unauthorized")
            .create_async()
            .await;

        let err = perplexity_tool()
            .search_perplexity_api("q", 5, "pplx-bad", &server.url())
            .await
            .expect_err("401 should fail");
        assert!(
            err.to_string()
                .contains("Perplexity Search API returned 401")
        );
    }

    #[test]
    fn test_resolve_perplexity_base_url_pplx_prefix() {
        let url = resolve_perplexity_base_url(None, "pplx-abc123");
        assert!(url.contains("perplexity.ai"));
    }

    #[test]
    fn test_resolve_perplexity_base_url_openrouter_prefix() {
        let url = resolve_perplexity_base_url(None, "sk-or-abc123");
        assert!(url.contains("openrouter.ai"));
    }

    #[test]
    fn test_resolve_perplexity_base_url_respects_override() {
        let url = resolve_perplexity_base_url(Some("https://custom.example"), "pplx-abc123");
        assert_eq!(url, "https://custom.example");
    }
}
