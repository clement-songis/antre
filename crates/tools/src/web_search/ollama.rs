//! Ollama hosted web search (`POST https://ollama.com/api/web_search`), with
//! the same API key as Ollama Cloud.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Error;

pub(super) const BASE_URL: &str = "https://ollama.com";

/// The API rejects more than 10 results.
const MAX_RESULTS: u8 = 10;

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    max_results: u8,
}

#[derive(Debug, Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<RawResult>,
}

#[derive(Debug, Default, Deserialize)]
struct RawResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct SearchResult {
    title: String,
    url: String,
    description: String,
}

pub(super) async fn search(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    query: &str,
    count: u8,
    timeout: Duration,
) -> crate::Result<serde_json::Value> {
    if api_key.trim().is_empty() {
        return Ok(serde_json::json!({
            "error": "Ollama API key not configured",
            "hint": "Set OLLAMA_API_KEY (an ollama.com key) or tools.web.search.api_key"
        }));
    }
    let response = client
        .post(format!("{}/api/web_search", base_url.trim_end_matches('/')))
        .timeout(timeout)
        .bearer_auth(api_key)
        .json(&SearchRequest {
            query,
            max_results: count.clamp(1, MAX_RESULTS),
        })
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body: String = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(500)
            .collect();
        return Err(Error::message(format!(
            "Ollama web search returned {status}: {body}"
        )));
    }
    let body: SearchResponse = response.json().await?;
    Ok(serde_json::json!({
        "provider": "ollama",
        "query": query,
        "results": parse_results(body),
    }))
}

fn parse_results(body: SearchResponse) -> Vec<SearchResult> {
    body.results
        .into_iter()
        .filter_map(|result| {
            let url = result.url.as_deref().map(str::trim).unwrap_or("");
            if url.is_empty() {
                return None;
            }
            let title = result
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(url);
            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                description: super::keenable::compact_snippet(
                    result.content.as_deref().unwrap_or(""),
                ),
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_bearer_key_and_maps_results() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/web_search")
            .match_header("authorization", "Bearer ol-test")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"query": "rust 2026", "max_results": 10}),
            ))
            .with_body(
                r#"{"results":[
                    {"title":"Rust","url":"https://rust-lang.org","content":"The  Rust\nlanguage"},
                    {"title":"","url":"https://example.com","content":""},
                    {"title":"No url","content":"dropped"}
                ]}"#,
            )
            .create_async()
            .await;
        let value = search(
            &reqwest::Client::new(),
            &server.url(),
            "ol-test",
            "rust 2026",
            25,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        mock.assert_async().await;
        assert_eq!(value["provider"], "ollama");
        let results = value["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["description"], "The Rust language");
        assert_eq!(results[1]["title"], "https://example.com");
    }

    #[tokio::test]
    async fn missing_key_returns_a_hint_without_calling() {
        let value = search(
            &reqwest::Client::new(),
            "http://127.0.0.1:9",
            " ",
            "q",
            5,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(value["error"].as_str().unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn http_errors_surface_the_status() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/web_search")
            .with_status(401)
            .with_body("unauthorized")
            .create_async()
            .await;
        let error = search(
            &reqwest::Client::new(),
            &server.url(),
            "bad",
            "q",
            5,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("401"), "{error}");
    }

    #[test]
    fn registers_only_with_a_key_or_the_fallback() {
        use {
            super::super::WebSearchTool,
            antre_config::schema::{SearchProvider, WebSearchConfig},
            std::collections::HashMap,
        };

        if std::env::var_os("OLLAMA_API_KEY").is_some() {
            return;
        }
        let config = WebSearchConfig {
            provider: SearchProvider::Ollama,
            duckduckgo_fallback: false,
            ..WebSearchConfig::default()
        };
        assert!(WebSearchTool::from_config_with_env_overrides(&config, &HashMap::new()).is_none());
        let env = HashMap::from([("OLLAMA_API_KEY".to_string(), "ol".to_string())]);
        assert!(WebSearchTool::from_config_with_env_overrides(&config, &env).is_some());
    }
}
