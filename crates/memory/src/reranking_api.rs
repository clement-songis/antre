//! Rerank APIs (Cohere, Jina, Voyage, llama.cpp/vLLM `/rerank`).
//!
//! All four take a query and a list of documents and return
//! `{index, relevance_score}` pairs; Voyage names the list `data` and the
//! limit `top_k`, the others `results` and `top_n`.

use {
    async_trait::async_trait,
    secrecy::{ExposeSecret, Secret},
    serde::Deserialize,
    tracing::{debug, warn},
};

use crate::{
    error::{Error, Result},
    reranking::RerankerProvider,
    search::SearchResult,
};

pub type RerankApi = antre_config::MemoryReranker;

/// Documents longer than this are cut before sending; rerankers chunk or
/// truncate long inputs anyway and billing counts characters.
const MAX_DOCUMENT_CHARS: usize = 4_000;

pub struct ApiReranker {
    api: RerankApi,
    endpoint: String,
    api_key: Option<Secret<String>>,
    model: String,
    client: reqwest::Client,
    max_candidates: usize,
}

#[derive(Deserialize)]
struct RerankResponse {
    #[serde(alias = "data")]
    results: Vec<RerankHit>,
}

#[derive(Deserialize)]
struct RerankHit {
    index: usize,
    relevance_score: f32,
}

impl ApiReranker {
    /// `None` for [`RerankApi::Llm`], which is not an HTTP rerank API, or
    /// when no base URL is known.
    #[must_use]
    pub fn new(
        api: RerankApi,
        base_url: Option<&str>,
        api_key: Option<Secret<String>>,
        model: impl Into<String>,
    ) -> Option<Self> {
        if api == RerankApi::Llm {
            return None;
        }
        let base = base_url.or_else(|| api.default_base_url())?;
        Some(Self {
            api,
            endpoint: format!("{}/rerank", base.trim_end_matches('/')),
            api_key: api_key.filter(|key| !key.expose_secret().is_empty()),
            model: model.into(),
            client: reqwest::Client::new(),
            max_candidates: 50,
        })
    }

    #[must_use]
    pub fn with_max_candidates(mut self, max: usize) -> Self {
        self.max_candidates = max.max(1);
        self
    }

    fn body(&self, query: &str, documents: &[String], top_k: usize) -> serde_json::Value {
        let limit_field = match self.api {
            RerankApi::Voyage => "top_k",
            RerankApi::Cohere | RerankApi::Jina | RerankApi::Custom | RerankApi::Llm => "top_n",
        };
        serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": documents,
            limit_field: top_k,
        })
    }

    async fn scores(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankHit>> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .json(&self.body(query, documents, top_k));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose_secret());
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::Reranking(format!("{} rerank request: {e}", self.api.as_str())))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            return Err(Error::Reranking(format!(
                "{} rerank HTTP {status}: {snippet}",
                self.api.as_str()
            )));
        }
        let parsed: RerankResponse = response
            .json()
            .await
            .map_err(|e| Error::Reranking(format!("{} rerank response: {e}", self.api.as_str())))?;
        Ok(parsed.results)
    }
}

/// Reorder `candidates` by `hits`, then append candidates the API did not
/// score and the `rest`, keeping at most `top_k`.
fn apply_hits(
    candidates: Vec<SearchResult>,
    rest: Vec<SearchResult>,
    mut hits: Vec<RerankHit>,
    top_k: usize,
) -> Vec<SearchResult> {
    hits.sort_by(|a, b| b.relevance_score.total_cmp(&a.relevance_score));
    let mut slots: Vec<Option<SearchResult>> = candidates.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(top_k);
    for hit in hits {
        if let Some(mut result) = slots.get_mut(hit.index).and_then(Option::take) {
            result.score = hit.relevance_score;
            ordered.push(result);
        }
    }
    ordered.extend(slots.into_iter().flatten());
    ordered.extend(rest);
    ordered.truncate(top_k);
    ordered
}

#[async_trait]
impl RerankerProvider for ApiReranker {
    async fn rerank(
        &self,
        query: &str,
        mut results: Vec<SearchResult>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        if results.len() < 2 {
            results.truncate(top_k);
            return Ok(results);
        }
        let rest = results.split_off(results.len().min(self.max_candidates));
        let documents: Vec<String> = results
            .iter()
            .map(|result| result.text.chars().take(MAX_DOCUMENT_CHARS).collect())
            .collect();
        match self.scores(query, &documents, results.len()).await {
            Ok(hits) => {
                debug!(
                    api = self.api.as_str(),
                    hits = hits.len(),
                    "reranked memory results"
                );
                Ok(apply_hits(results, rest, hits, top_k))
            },
            Err(error) => {
                warn!(%error, "rerank failed, keeping search order");
                results.extend(rest);
                results.truncate(top_k);
                Ok(results)
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn result(id: &str, score: f32) -> SearchResult {
        SearchResult {
            chunk_id: id.into(),
            path: format!("{id}.md"),
            source: "memory".into(),
            start_line: 1,
            end_line: 2,
            score,
            text: format!("text of {id}"),
        }
    }

    fn ids(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.chunk_id.as_str()).collect()
    }

    #[tokio::test]
    async fn cohere_shape_reorders_and_sends_bearer_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v2/rerank")
            .match_header("authorization", "Bearer co-test")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "model": "rerank-v5.0-fast",
                "query": "where is the key",
                "documents": ["text of a", "text of b", "text of c"],
                "top_n": 3
            })))
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"results":[{"index":2,"relevance_score":0.9},{"index":0,"relevance_score":0.4}]}"#,
            )
            .create_async()
            .await;
        let reranker = ApiReranker::new(
            RerankApi::Cohere,
            Some(&format!("{}/v2", server.url())),
            Some(Secret::new("co-test".into())),
            "rerank-v5.0-fast",
        )
        .unwrap();
        let out = reranker
            .rerank(
                "where is the key",
                vec![result("a", 0.5), result("b", 0.4), result("c", 0.3)],
                2,
            )
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(ids(&out), ["c", "a"]);
        assert!((out[0].score - 0.9).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn voyage_shape_uses_top_k_and_data() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/rerank")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"top_k": 2, "model": "rerank-3"}),
            ))
            .with_body(
                r#"{"data":[{"index":1,"relevance_score":0.8},{"index":0,"relevance_score":0.1}]}"#,
            )
            .create_async()
            .await;
        let reranker = ApiReranker::new(
            RerankApi::Voyage,
            Some(&format!("{}/v1", server.url())),
            None,
            "rerank-3",
        )
        .unwrap();
        let out = reranker
            .rerank("q", vec![result("a", 0.9), result("b", 0.2)], 2)
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(ids(&out), ["b", "a"]);
    }

    #[tokio::test]
    async fn http_error_keeps_the_search_order() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/rerank")
            .with_status(503)
            .create_async()
            .await;
        let reranker =
            ApiReranker::new(RerankApi::Custom, Some(&server.url()), None, "bge").unwrap();
        let out = reranker
            .rerank(
                "q",
                vec![result("a", 0.9), result("b", 0.2), result("c", 0.1)],
                2,
            )
            .await
            .unwrap();
        assert_eq!(ids(&out), ["a", "b"]);
    }

    #[tokio::test]
    async fn candidates_beyond_the_cap_follow_the_reranked_ones() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/rerank")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"documents": ["text of a", "text of b"]}),
            ))
            .with_body(r#"{"results":[{"index":1,"relevance_score":0.7},{"index":9,"relevance_score":0.9}]}"#)
            .create_async()
            .await;
        let reranker = ApiReranker::new(
            RerankApi::Jina,
            Some(&server.url()),
            None,
            "jina-reranker-v3",
        )
        .unwrap()
        .with_max_candidates(2);
        let out = reranker
            .rerank(
                "q",
                vec![result("a", 0.9), result("b", 0.5), result("c", 0.4)],
                3,
            )
            .await
            .unwrap();
        assert_eq!(ids(&out), ["b", "a", "c"]);
    }

    #[test]
    fn llm_and_unknown_base_urls_build_nothing() {
        assert!(ApiReranker::new(RerankApi::Llm, None, None, "m").is_none());
        assert!(ApiReranker::new(RerankApi::Custom, None, None, "m").is_none());
        assert!(ApiReranker::new(RerankApi::Cohere, None, None, "m").is_some());
    }
}
