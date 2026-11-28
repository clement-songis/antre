//! Image generation through OpenAI-shaped `/images/generations` endpoints
//! with an API key: OpenAI, Gemini (OpenAI compatibility layer) and xAI.

use std::collections::HashMap;

use {
    antre_config::schema::ProvidersConfig,
    antre_model_catalog::{ModelKind, global},
    secrecy::{ExposeSecret, Secret},
    serde::Deserialize,
};

/// Used only if the catalog has no OpenAI image model at all.
const FALLBACK_OPENAI_IMAGE_MODEL: &str = "gpt-image-2.6-flare";
const DEFAULT_SIZE: &str = "1024x1024";
const DEFAULT_QUALITY: &str = "medium";
const DEFAULT_OUTPUT_FORMAT: &str = "png";
const DEFAULT_BACKGROUND: &str = "opaque";
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub model: String,
    pub size: String,
    pub quality: String,
    pub output_format: String,
    pub background: String,
}

impl ImageGenerationRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: default_image_model(),
            size: DEFAULT_SIZE.to_string(),
            quality: DEFAULT_QUALITY.to_string(),
            output_format: DEFAULT_OUTPUT_FORMAT.to_string(),
            background: DEFAULT_BACKGROUND.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub media_type: String,
    pub data: String,
}

impl GeneratedImage {
    #[must_use]
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationResult {
    pub model: String,
    pub images: Vec<GeneratedImage>,
}

/// OpenAI's image role from the model catalog.
#[must_use]
pub fn default_image_model() -> String {
    global::role_id_or("openai", ModelKind::Image, FALLBACK_OPENAI_IMAGE_MODEL)
}

/// Active OpenAI image models the catalog knows, the default first.
#[must_use]
pub fn supported_image_models() -> Vec<String> {
    ImagesApi::OpenAi.supported_models()
}

/// A provider serving `/images/generations` with an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagesApi {
    OpenAi,
    Gemini,
    Xai,
}

impl ImagesApi {
    /// Tried in this order when picking a backend from configured keys.
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::Gemini, Self::Xai];

    /// Provider name in `[providers]` and in the model catalog.
    #[must_use]
    pub fn provider(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Xai => "xai",
        }
    }

    fn env_key(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Xai => "XAI_API_KEY",
        }
    }

    fn env_base_url_key(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_BASE_URL",
            Self::Gemini => "GEMINI_BASE_URL",
            Self::Xai => "XAI_BASE_URL",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            Self::Xai => "https://api.x.ai/v1",
        }
    }

    /// The catalog's image pick for this provider.
    #[must_use]
    pub fn default_model(self) -> Option<String> {
        match self {
            Self::OpenAi => Some(default_image_model()),
            Self::Gemini | Self::Xai => {
                global::pick_role(self.provider(), ModelKind::Image).map(|m| m.id().to_string())
            },
        }
    }

    /// Active image models the catalog knows for this provider, the default
    /// first.
    #[must_use]
    pub fn supported_models(self) -> Vec<String> {
        let default = self.default_model();
        let mut models: Vec<String> = global::read(|catalog| {
            catalog
                .models_of_kind(self.provider(), ModelKind::Image)
                .into_iter()
                .filter(|model| !model.status().is_deprecated())
                .map(|model| model.spec.id)
                .collect()
        });
        if let Some(default) = default {
            models.retain(|id| *id != default);
            models.insert(0, default);
        }
        models
    }

    /// Only OpenAI's gpt-image models take quality, format and background;
    /// the others return a base64 image when asked for `b64_json`.
    fn body(self, request: &ImageGenerationRequest) -> serde_json::Value {
        match self {
            Self::OpenAi => serde_json::json!({
                "model": request.model,
                "prompt": request.prompt.trim(),
                "n": 1,
                "size": request.size,
                "quality": request.quality,
                "output_format": request.output_format,
                "background": request.background,
            }),
            Self::Gemini | Self::Xai => serde_json::json!({
                "model": request.model,
                "prompt": request.prompt.trim(),
                "n": 1,
                "response_format": "b64_json",
            }),
        }
    }
}

/// Image generation over an OpenAI-shaped Images API.
pub struct ImagesApiProvider {
    api: ImagesApi,
    base_url: String,
    api_key: Secret<String>,
    client: &'static reqwest::Client,
}

#[derive(Deserialize)]
struct ImagesResponse {
    #[serde(default)]
    data: Vec<ImageData>,
    #[serde(default)]
    output_format: Option<String>,
}

#[derive(Deserialize)]
struct ImageData {
    #[serde(default)]
    b64_json: Option<String>,
}

impl ImagesApiProvider {
    #[must_use]
    pub fn new(api: ImagesApi, base_url: impl Into<String>, api_key: Secret<String>) -> Self {
        Self {
            api,
            base_url: base_url.into(),
            api_key,
            client: crate::shared_http_client(),
        }
    }

    /// The first of OpenAI, Gemini and xAI that has an API key (config or
    /// environment) and is enabled.
    #[must_use]
    pub fn from_config(
        config: &ProvidersConfig,
        env_overrides: &HashMap<String, String>,
    ) -> Option<Self> {
        ImagesApi::ALL.into_iter().find_map(|api| {
            let name = api.provider();
            if !config.is_enabled(name) {
                return None;
            }
            let key =
                crate::config_helpers::resolve_api_key(config, name, api.env_key(), env_overrides)
                    .or_else(|| {
                        (api == ImagesApi::Gemini)
                            .then(|| {
                                crate::config_helpers::env_value(env_overrides, "GOOGLE_API_KEY")
                            })
                            .flatten()
                            .map(Secret::new)
                    })?;
            let base_url = config
                .get(name)
                .and_then(|entry| entry.base_url.clone())
                .or_else(|| crate::config_helpers::env_value(env_overrides, api.env_base_url_key()))
                .unwrap_or_else(|| api.default_base_url().to_string());
            Some(Self::new(api, base_url, key))
        })
    }

    #[must_use]
    pub fn api(&self) -> ImagesApi {
        self.api
    }

    pub async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> anyhow::Result<ImageGenerationResult> {
        if request.prompt.trim().is_empty() {
            anyhow::bail!("image prompt is required");
        }
        let response = self
            .client
            .post(format!(
                "{}/images/generations",
                self.base_url.trim_end_matches('/')
            ))
            .bearer_auth(self.api_key.expose_secret())
            .json(&self.api.body(&request))
            .send()
            .await?;
        let status = response.status();
        let retry_after_ms = crate::http::retry_after_ms_from_headers(response.headers());
        let body = read_limited(response).await?;
        if !status.is_success() {
            let snippet: String = String::from_utf8_lossy(&body).chars().take(500).collect();
            anyhow::bail!(
                "{}",
                crate::http::with_retry_after_marker(
                    format!(
                        "{} image generation API error HTTP {status}: {snippet}",
                        self.api.provider()
                    ),
                    retry_after_ms,
                )
            );
        }
        let parsed: ImagesResponse = serde_json::from_slice(&body)?;
        let format = parsed
            .output_format
            .as_deref()
            .unwrap_or(match self.api {
                ImagesApi::OpenAi => request.output_format.as_str(),
                ImagesApi::Gemini | ImagesApi::Xai => "png",
            })
            .to_string();
        let images: Vec<GeneratedImage> = parsed
            .data
            .into_iter()
            .filter_map(|item| item.b64_json)
            .filter(|data| !data.is_empty())
            .map(|data| GeneratedImage {
                media_type: media_type_for(&data, &format).to_string(),
                data,
            })
            .collect();
        if images.is_empty() {
            anyhow::bail!(
                "{} image generation returned no images",
                self.api.provider()
            );
        }
        Ok(ImageGenerationResult {
            model: request.model,
            images,
        })
    }
}

/// The declared format, corrected by the base64 magic bytes when they say
/// otherwise (providers that ignore `output_format`).
fn media_type_for(data: &str, format: &str) -> &'static str {
    if data.starts_with("iVBOR") {
        return "image/png";
    }
    if data.starts_with("/9j/") {
        return "image/jpeg";
    }
    if data.starts_with("UklGR") {
        return "image/webp";
    }
    match format.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

async fn read_limited(response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    use futures::TryStreamExt as _;

    let mut buf = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.try_next().await? {
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("image generation response exceeded size limit");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, antre_config::schema::ProviderEntry};

    fn request(model: &str) -> ImageGenerationRequest {
        ImageGenerationRequest {
            model: model.into(),
            ..ImageGenerationRequest::new("a neon fox")
        }
    }

    #[tokio::test]
    async fn openai_sends_gpt_image_options_and_decodes_b64() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/images/generations")
            .match_header("authorization", "Bearer sk-test")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "model": "gpt-image-2.6-flare",
                "prompt": "a neon fox",
                "quality": "medium",
                "output_format": "png",
                "background": "opaque"
            })))
            .with_body(r#"{"data":[{"b64_json":"iVBORw0KGgo="}],"output_format":"png"}"#)
            .create_async()
            .await;
        let provider = ImagesApiProvider::new(
            ImagesApi::OpenAi,
            format!("{}/v1", server.url()),
            Secret::new("sk-test".into()),
        );
        let result = provider
            .generate_image(request("gpt-image-2.6-flare"))
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
        assert!(
            result.images[0]
                .data_uri()
                .starts_with("data:image/png;base64,iVBOR")
        );
    }

    #[tokio::test]
    async fn gemini_asks_for_b64_json_without_gpt_image_options() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images/generations")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"response_format": "b64_json", "n": 1}),
            ))
            .with_body(r#"{"data":[{"b64_json":"/9j/4AAQ"}]}"#)
            .create_async()
            .await;
        let provider =
            ImagesApiProvider::new(ImagesApi::Gemini, server.url(), Secret::new("g".into()));
        let body = ImagesApi::Gemini.body(&request("gemini-3-pro-image"));
        assert!(body.get("quality").is_none());
        let result = provider
            .generate_image(request("gemini-3-pro-image"))
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(result.images[0].media_type, "image/jpeg");
    }

    #[tokio::test]
    async fn http_errors_and_empty_results_fail() {
        let mut server = mockito::Server::new_async().await;
        let _error = server
            .mock("POST", "/images/generations")
            .with_status(400)
            .with_body(r#"{"error":{"message":"bad prompt"}}"#)
            .create_async()
            .await;
        let provider =
            ImagesApiProvider::new(ImagesApi::Xai, server.url(), Secret::new("x".into()));
        let error = provider
            .generate_image(request("grok-imagine-image"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 400"), "{error}");
        assert!(error.contains("bad prompt"), "{error}");

        let mut empty_server = mockito::Server::new_async().await;
        let _empty = empty_server
            .mock("POST", "/images/generations")
            .with_body(r#"{"data":[]}"#)
            .create_async()
            .await;
        let provider =
            ImagesApiProvider::new(ImagesApi::Xai, empty_server.url(), Secret::new("x".into()));
        assert!(provider.generate_image(request("m")).await.is_err());
        assert!(
            provider
                .generate_image(ImageGenerationRequest::new("  "))
                .await
                .is_err()
        );
    }

    #[test]
    fn from_config_picks_the_first_provider_with_a_key() {
        let mut config = ProvidersConfig::default();
        config.providers.insert("xai".into(), ProviderEntry {
            api_key: Some(Secret::new("x".into())),
            ..ProviderEntry::default()
        });
        let env = HashMap::from([("GEMINI_API_KEY".to_string(), "g".to_string())]);
        if std::env::var_os("OPENAI_API_KEY").is_none() {
            let provider = ImagesApiProvider::from_config(&config, &env).unwrap();
            assert_eq!(provider.api(), ImagesApi::Gemini);
            let provider = ImagesApiProvider::from_config(&config, &HashMap::new());
            if std::env::var_os("GEMINI_API_KEY").is_none()
                && std::env::var_os("GOOGLE_API_KEY").is_none()
            {
                assert_eq!(provider.unwrap().api(), ImagesApi::Xai);
            }
        }
        let mut openai = ProvidersConfig::default();
        openai.providers.insert("openai".into(), ProviderEntry {
            api_key: Some(Secret::new("sk".into())),
            base_url: Some("https://proxy.example/v1".into()),
            ..ProviderEntry::default()
        });
        let provider = ImagesApiProvider::from_config(&openai, &HashMap::new()).unwrap();
        assert_eq!(provider.api(), ImagesApi::OpenAi);
        assert_eq!(provider.base_url, "https://proxy.example/v1");
    }

    #[test]
    fn catalog_models_back_every_api() {
        for api in ImagesApi::ALL {
            let models = api.supported_models();
            assert!(!models.is_empty(), "{api:?} has no image models");
            assert_eq!(api.default_model().as_ref(), models.first());
        }
    }
}
