//! `generate_image` tool backed by OpenAI Codex OAuth image generation or an
//! API-key Images API (OpenAI, Gemini, xAI).
//!
//! The chat runner treats a `screenshot` data URI in a tool result as native
//! image media for channel delivery, so this returns generated images using the
//! same contract as `send_image` and browser/map screenshots.

use {
    antre_agents::tool_registry::AgentTool,
    antre_providers::images_api::{
        GeneratedImage, ImageGenerationRequest, ImageGenerationResult, ImagesApi, ImagesApiProvider,
    },
    async_trait::async_trait,
    serde_json::{Value, json},
};

use crate::{Error, params::str_param};

const DEFAULT_SIZE: &str = "1024x1024";
const DEFAULT_QUALITY: &str = "medium";
const DEFAULT_OUTPUT_FORMAT: &str = "png";
const DEFAULT_BACKGROUND: &str = "opaque";

const SUPPORTED_SIZES: &[&str] = &[
    "1024x1024",
    "1536x1024",
    "1024x1536",
    "2048x2048",
    "2048x1152",
    "3840x2160",
    "2160x3840",
];
const SUPPORTED_QUALITIES: &[&str] = &["low", "medium", "high", "auto"];
const SUPPORTED_FORMATS: &[&str] = &["png", "jpeg", "webp"];
const SUPPORTED_BACKGROUNDS: &[&str] = &["opaque", "transparent", "auto"];

enum Backend {
    #[cfg(feature = "provider-openai-codex")]
    Codex(antre_providers::openai_codex_image::OpenAiCodexImageProvider),
    Api(ImagesApiProvider),
}

pub struct GenerateImageTool {
    backend: Backend,
    description: String,
}

#[cfg(feature = "provider-openai-codex")]
impl Default for GenerateImageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerateImageTool {
    /// Image generation through the OpenAI Codex (ChatGPT) OAuth login.
    #[cfg(feature = "provider-openai-codex")]
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: Backend::Codex(
                antre_providers::openai_codex_image::OpenAiCodexImageProvider::new(),
            ),
            description: "Generate an image from a text prompt using OpenAI gpt-image models via the existing OpenAI Codex OAuth login. Returns channel-sendable image media.".into(),
        }
    }

    /// Image generation through a provider's Images API with an API key.
    #[must_use]
    pub fn with_api(provider: ImagesApiProvider) -> Self {
        let description = format!(
            "Generate an image from a text prompt using {} image models (API key). Returns channel-sendable image media.",
            provider.api().provider()
        );
        Self {
            backend: Backend::Api(provider),
            description,
        }
    }

    /// The catalog provider whose image models this backend serves.
    fn images_api(&self) -> ImagesApi {
        match &self.backend {
            #[cfg(feature = "provider-openai-codex")]
            Backend::Codex(_) => ImagesApi::OpenAi,
            Backend::Api(provider) => provider.api(),
        }
    }

    fn request_from_params(
        api: ImagesApi,
        params: &Value,
    ) -> anyhow::Result<ImageGenerationRequest> {
        let prompt = str_param(params, "prompt")
            .ok_or_else(|| Error::message("missing required parameter: prompt"))?;

        let supported = api.supported_models();
        let model = str_param(params, "model")
            .map(str::to_string)
            .or_else(|| api.default_model())
            .ok_or_else(|| {
                Error::message(format!("no image model known for {}", api.provider()))
            })?;
        if !supported.contains(&model) {
            return Err(Error::message(format!(
                "unsupported image model '{model}' — supported: {}",
                supported.join(", ")
            ))
            .into());
        }

        let size = validate_optional_enum(params, "size", DEFAULT_SIZE, SUPPORTED_SIZES)?;
        let quality =
            validate_optional_enum(params, "quality", DEFAULT_QUALITY, SUPPORTED_QUALITIES)?;
        let output_format = validate_optional_enum(
            params,
            "output_format",
            DEFAULT_OUTPUT_FORMAT,
            SUPPORTED_FORMATS,
        )?;
        let background = validate_optional_enum(
            params,
            "background",
            DEFAULT_BACKGROUND,
            SUPPORTED_BACKGROUNDS,
        )?;

        Ok(ImageGenerationRequest {
            prompt: prompt.to_string(),
            model,
            size: size.to_string(),
            quality: quality.to_string(),
            output_format: output_format.to_string(),
            background: background.to_string(),
        })
    }
}

fn validate_optional_enum<'a>(
    params: &'a Value,
    key: &str,
    default: &'static str,
    allowed: &[&str],
) -> anyhow::Result<&'a str> {
    let Some(value) = str_param(params, key) else {
        return Ok(default);
    };
    if allowed.contains(&value) {
        return Ok(value);
    }
    Err(Error::message(format!(
        "invalid {key} '{value}' — supported: {}",
        allowed.join(", ")
    ))
    .into())
}

fn build_tool_result(request: &ImageGenerationRequest, image: &GeneratedImage) -> Value {
    json!({
        "sent": true,
        "screenshot": image.data_uri(),
        "caption": format!("Generated image: {}", request.prompt.trim()),
        "model": request.model,
        "size": request.size,
        "quality": request.quality,
        "output_format": request.output_format,
        "background": request.background,
    })
}

#[async_trait]
impl AgentTool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Detailed image generation prompt."
                },
                "model": {
                    "type": "string",
                    "enum": self.images_api().supported_models(),
                    "description": format!(
                        "Image model to use. Defaults to {}.",
                        self.images_api().default_model().unwrap_or_default()
                    )
                },
                "size": {
                    "type": "string",
                    "enum": SUPPORTED_SIZES,
                    "description": "Output size. Defaults to 1024x1024."
                },
                "quality": {
                    "type": "string",
                    "enum": SUPPORTED_QUALITIES,
                    "description": "Generation quality. Defaults to medium."
                },
                "output_format": {
                    "type": "string",
                    "enum": SUPPORTED_FORMATS,
                    "description": "Image format. Defaults to png."
                },
                "background": {
                    "type": "string",
                    "enum": SUPPORTED_BACKGROUNDS,
                    "description": "Background handling. Defaults to opaque."
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        let request = Self::request_from_params(self.images_api(), &params)?;
        let result: ImageGenerationResult = match &self.backend {
            #[cfg(feature = "provider-openai-codex")]
            Backend::Codex(provider) => provider.generate_image(request.clone()).await?,
            Backend::Api(provider) => provider.generate_image(request.clone()).await?,
        };
        let image = result
            .images
            .first()
            .ok_or_else(|| Error::message("image generation returned no images"))?;
        Ok(build_tool_result(&request, image))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, base64::Engine as _, serde_json::json};

    #[test]
    fn request_from_params_applies_defaults() {
        let request = GenerateImageTool::request_from_params(
            ImagesApi::OpenAi,
            &json!({
                "prompt": "a neon fox"
            }),
        )
        .unwrap();

        assert_eq!(request.prompt, "a neon fox");
        assert_eq!(Some(request.model), ImagesApi::OpenAi.default_model());
        assert_eq!(request.size, DEFAULT_SIZE);
        assert_eq!(request.quality, DEFAULT_QUALITY);
        assert_eq!(request.output_format, DEFAULT_OUTPUT_FORMAT);
        assert_eq!(request.background, DEFAULT_BACKGROUND);
    }

    #[test]
    fn request_from_params_accepts_every_supported_model() {
        for model in ImagesApi::OpenAi.supported_models() {
            let request = GenerateImageTool::request_from_params(
                ImagesApi::OpenAi,
                &json!({
                    "prompt": "a neon fox",
                    "model": model
                }),
            )
            .unwrap();
            assert_eq!(request.model, model);
        }
    }

    #[test]
    fn request_from_params_rejects_retired_model() {
        let err = GenerateImageTool::request_from_params(
            ImagesApi::OpenAi,
            &json!({
                "prompt": "a neon fox",
                "model": "gpt-image-2"
            }),
        )
        .expect_err("retired model should fail");

        assert!(err.to_string().contains("unsupported image model"));
    }

    #[test]
    fn request_from_params_rejects_invalid_quality() {
        let err = GenerateImageTool::request_from_params(
            ImagesApi::OpenAi,
            &json!({
                "prompt": "a neon fox",
                "quality": "ultra"
            }),
        )
        .expect_err("invalid quality should fail");

        assert!(err.to_string().contains("invalid quality"));
    }

    #[test]
    fn tool_result_uses_channel_screenshot_contract() {
        let request = ImageGenerationRequest::new("a neon fox");
        let data = base64::engine::general_purpose::STANDARD.encode(b"png");
        let image = GeneratedImage {
            media_type: "image/png".into(),
            data,
        };

        let result = build_tool_result(&request, &image);

        assert_eq!(result["sent"], true);
        assert!(
            result["screenshot"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(result["caption"], "Generated image: a neon fox");
    }

    #[tokio::test]
    async fn api_backend_generates_with_the_provider_default_model() {
        let mut server = mockito::Server::new_async().await;
        let default = ImagesApi::Xai.default_model().unwrap();
        let mock = server
            .mock("POST", "/images/generations")
            .match_body(mockito::Matcher::PartialJson(json!({"model": default})))
            .with_body(r#"{"data":[{"b64_json":"iVBORw0KGgo="}]}"#)
            .create_async()
            .await;
        let tool = GenerateImageTool::with_api(ImagesApiProvider::new(
            ImagesApi::Xai,
            server.url(),
            secrecy::Secret::new("x".into()),
        ));
        assert!(tool.description().contains("xai"));
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["model"]["enum"][0], default);

        let result = tool.execute(json!({"prompt": "a fox"})).await.unwrap();
        mock.assert_async().await;
        assert!(
            result["screenshot"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );

        let err = tool
            .execute(json!({"prompt": "a fox", "model": "gpt-image-2.6-flare"}))
            .await
            .expect_err("openai model on the xai backend");
        assert!(err.to_string().contains("unsupported image model"));
    }
}
