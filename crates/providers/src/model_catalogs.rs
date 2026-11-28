//! Static model catalogs and OpenAI-compatible provider definitions.

use crate::openai::{
    CacheControlPolicy, OpenAiProviderCapabilities, ProbeFallbackPolicy, ProbeOutputCapPolicy,
    RateLimitPolicy, ReasoningEffortPolicy, SystemMessageRewriteStrategy,
};

/// Known Anthropic Claude models (model_id, display_name).
/// Current models listed first, then legacy models.
pub(crate) const ANTHROPIC_MODELS: &[(&str, &str)] = &[
    // Snapshot of GET /v1/models (2026-11-26), newest first, in API order.
    // Anthropic documents current generations using alias IDs rather than
    // dated snapshot IDs; keep the aliases instead of inventing snapshot
    // suffixes that the API rejects.
    ("claude-fable-5-2", "Claude Fable 5.2"),
    ("claude-haiku-5-5", "Claude Haiku 5.5"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ("claude-opus-5-5", "Claude Opus 5.5"),
    ("claude-fable-5-1", "Claude Fable 5.1"),
    ("claude-opus-5", "Claude Opus 5"),
    ("claude-sonnet-5", "Claude Sonnet 5"),
    ("claude-fable-5", "Claude Fable 5"),
    ("claude-opus-4-8", "Claude Opus 4.8"),
    ("claude-opus-4-7", "Claude Opus 4.7"),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("claude-opus-4-6", "Claude Opus 4.6"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
];

/// Known Mistral models.
pub(crate) const MISTRAL_MODELS: &[(&str, &str)] = &[
    ("mistral-large-4", "Mistral Large 4"),
    ("mistral-small-2603", "Mistral Small 4"),
    ("codestral-2508", "Codestral 25.08"),
];

/// Known Cerebras models.
pub(crate) const CEREBRAS_MODELS: &[(&str, &str)] = &[
    ("gpt-oss-120b", "GPT OSS 120B"),
    ("qwen-3.8-27b", "Qwen 3.8 27B"),
];

/// Known Groq models.
/// See: <https://console.groq.com/docs/models>
pub(crate) const GROQ_MODELS: &[(&str, &str)] = &[
    ("openai/gpt-oss-120b", "GPT OSS 120B"),
    ("openai/gpt-oss-20b", "GPT OSS 20B"),
    ("qwen/qwen3.8-27b", "Qwen 3.8 27B"),
];

/// Known xAI models.
/// See: <https://docs.x.ai/developers/models>
pub(crate) const XAI_MODELS: &[(&str, &str)] =
    &[("grok-4.7", "Grok 4.7"), ("grok-4.3", "Grok 4.3")];

/// Known MiniMax models.
/// See: <https://platform.minimax.io/docs/api-reference/text-openai-api>
pub(crate) const MINIMAX_MODELS: &[(&str, &str)] = &[
    ("MiniMax-M3.1-Flash", "MiniMax M3.1 Flash"),
    ("MiniMax-M3", "MiniMax M3"),
];

/// Known Z.AI (Zhipu) models.
/// See: <https://docs.z.ai/api-reference/llm/chat-completion>
pub(crate) const ZAI_MODELS: &[(&str, &str)] = &[
    ("glm-5.3", "GLM-5.3"),
    ("glm-5.3-flash", "GLM-5.3 Flash"),
    ("glm-5.3-flashx", "GLM-5.3 FlashX"),
    ("glm-5.2", "GLM-5.2"),
    ("glm-5", "GLM-5"),
    ("glm-4.7", "GLM-4.7"),
    ("glm-4.7-flash", "GLM-4.7 Flash"),
    ("glm-4.7-flashx", "GLM-4.7 FlashX"),
    ("glm-4.6", "GLM-4.6"),
    ("glm-4.6v", "GLM-4.6V (Vision)"),
    ("glm-4.6v-flash", "GLM-4.6V Flash"),
    ("glm-4.5", "GLM-4.5"),
    ("glm-4.5-air", "GLM-4.5 Air"),
    ("glm-4.5-flash", "GLM-4.5 Flash"),
    ("glm-4.5v", "GLM-4.5V (Vision)"),
    ("glm-4-32b-0414-128k", "GLM-4 32B 128K"),
];

/// Known Z.AI Coding Plan models (a subset of the general API's).
pub(crate) const ZAI_CODE_MODELS: &[(&str, &str)] =
    &[("glm-5.3", "GLM-5.3"), ("glm-5.3-flash", "GLM-5.3 Flash")];

/// Fireworks Kimi model-ID prefixes.
///
/// These models proxy through Fireworks to Moonshot's Kimi API, which has
/// different schema and message requirements (no strict tools, needs
/// `reasoning_content`). Issue #810.
const FIREWORKS_KIMI_ROUTER_PREFIXES: &[&str] = &[
    "accounts/fireworks/routers/kimi",
    "accounts/fireworks/models/kimi",
];

/// Known Fireworks models.
/// Kimi K2.6 is on-demand only; Kimi K3 is the serverless replacement.
/// See: <https://fireworks.ai/models/fireworks/kimi-k3>
pub(crate) const FIREWORKS_MODELS: &[(&str, &str)] = &[
    ("accounts/fireworks/models/kimi-k3", "Kimi K3"),
    ("accounts/fireworks/models/kimi-k3p1-pro", "Kimi K3.1 Pro"),
    (
        "accounts/fireworks/models/minimax-m3p1-flash",
        "MiniMax M3.1 Flash",
    ),
];

/// Known Alibaba Cloud Coding Plan models.
/// See: <https://www.alibabacloud.com/help/en/model-studio/coding-plan>
pub(crate) const ALIBABA_CODING_MODELS: &[(&str, &str)] = &[
    ("qwen3.7-plus", "Qwen 3.7 Plus"),
    ("qwen3.6-plus", "Qwen 3.6 Plus"),
    ("glm-5", "GLM-5"),
    ("MiniMax-M2.5", "MiniMax M2.5"),
    ("kimi-k2.5", "Kimi K2.5"),
];

/// Known DeepInfra models.
/// See: <https://deepinfra.com/models>
pub(crate) const DEEPINFRA_MODELS: &[(&str, &str)] = &[
    ("deepseek-ai/DeepSeek-V4.1-Flash", "DeepSeek V4.1 Flash"),
    ("moonshotai/Kimi-K3.1-Pro", "Kimi K3.1 Pro"),
    ("Qwen/Qwen4-Max", "Qwen4 Max"),
    ("zai-org/GLM-5.3-Flash", "GLM-5.3 Flash"),
];

/// Known DeepSeek models.
pub(crate) const DEEPSEEK_MODELS: &[(&str, &str)] = &[
    ("deepseek-flash", "DeepSeek Flash"),
    ("deepseek-v4-pro", "DeepSeek V4 Pro"),
];

/// Known Moonshot models.
pub(crate) const MOONSHOT_MODELS: &[(&str, &str)] = &[
    ("kimi-k3.1-pro", "Kimi K3.1 Pro"),
    ("kimi-k3", "Kimi K3"),
    ("kimi-k3.1-pro-highspeed", "Kimi K3.1 Pro Highspeed"),
];

/// Known Google Gemini models.
/// See: <https://ai.google.dev/gemini-api/docs/models>
pub(crate) const GEMINI_MODELS: &[(&str, &str)] = &[
    ("gemini-4-carbon", "Gemini 4 Carbon"),
    ("gemini-4-argon", "Gemini 4 Argon"),
    ("gemini-3.8-flash", "Gemini 3.8 Flash"),
    ("gemini-4-neon", "Gemini 4 Neon"),
    ("gemini-4-hydrogen", "Gemini 4 Hydrogen"),
];

/// OpenAI-compatible provider definition for table-driven registration.
pub(crate) struct OpenAiCompatDef {
    pub(crate) config_name: &'static str,
    pub(crate) env_key: &'static str,
    pub(crate) env_base_url_key: &'static str,
    pub(crate) default_base_url: &'static str,
    pub(crate) models: &'static [(&'static str, &'static str)],
    /// Whether to attempt `/models` discovery by default. Providers whose API
    /// does not expose a models endpoint (e.g. MiniMax returns 404) should set
    /// this to `false` so the static catalog is used without a noisy warning.
    /// Users can still override via `fetch_models = true` in config.
    pub(crate) supports_model_discovery: bool,
    /// When `false`, a dummy API key (the provider name) is used if none is
    /// configured. Intended for local servers that don't authenticate.
    pub(crate) requires_api_key: bool,
    /// Local-only providers (Ollama, LM Studio) are skipped unless the user
    /// has an explicit `[providers.<name>]` entry, a `_BASE_URL` env var, or
    /// configured models. This avoids probing localhost when nothing is running.
    /// Also ensures model discovery is always attempted (never short-circuited
    /// by the empty-catalog heuristic).
    pub(crate) local_only: bool,
    /// Explicit provider behavior policies. Never inferred from provider name or URL.
    pub(crate) capabilities: OpenAiProviderCapabilities,
    /// Name shown on the onboarding card. Empty for providers whose card is
    /// declared by hand in `moltis-provider-setup`.
    pub(crate) display_name: &'static str,
    /// Placeholders in `default_base_url` filled from the environment
    /// (Cloudflare account id, AWS region, Azure resource name).
    pub(crate) base_url_vars: &'static [BaseUrlVar],
    /// Fall back to the catalog's chat models for this provider when `/models`
    /// is unavailable, instead of registering nothing.
    pub(crate) catalog_fallback: bool,
    /// The provider serves user-named deployments, so models must be listed
    /// in config (Azure).
    pub(crate) requires_model: bool,
}

/// A `{placeholder}` in a default base URL, read from an environment variable.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BaseUrlVar {
    pub(crate) placeholder: &'static str,
    pub(crate) env_key: &'static str,
    pub(crate) default: Option<&'static str>,
}

/// Values substituted into a hostname or path segment: anything else could
/// redirect the request to another host.
fn is_safe_url_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl OpenAiCompatDef {
    const DEFAULT: Self = Self {
        config_name: "",
        env_key: "",
        env_base_url_key: "",
        default_base_url: "",
        models: &[],
        supports_model_discovery: true,
        requires_api_key: true,
        local_only: false,
        capabilities: OpenAiProviderCapabilities::DEFAULT,
        display_name: "",
        base_url_vars: &[],
        catalog_fallback: false,
        requires_model: false,
    };

    /// Base URL for this provider: config `base_url`, then the
    /// `<NAME>_BASE_URL` variable, then the default with its placeholders
    /// filled. `None` when a placeholder has no (valid) value.
    pub(crate) fn resolve_base_url(
        &self,
        config: &moltis_config::schema::ProvidersConfig,
        env_overrides: &std::collections::HashMap<String, String>,
    ) -> Option<String> {
        if let Some(url) = config
            .get(self.config_name)
            .and_then(|entry| entry.base_url.clone())
            .or_else(|| crate::config_helpers::env_value(env_overrides, self.env_base_url_key))
        {
            return Some(url);
        }
        let mut url = self.default_base_url.to_string();
        for var in self.base_url_vars {
            let value = crate::config_helpers::env_value(env_overrides, var.env_key)
                .or_else(|| var.default.map(str::to_string))?;
            if !is_safe_url_segment(&value) {
                tracing::warn!(
                    provider = self.config_name,
                    env = var.env_key,
                    "ignoring provider: value must be letters, digits, '-' or '_'"
                );
                return None;
            }
            url = url.replace(var.placeholder, &value);
        }
        Some(url)
    }

    /// Catalog chat models for this provider, curated picks first, used when
    /// live discovery is unavailable.
    pub(crate) fn catalog_models(&self) -> Vec<crate::discovered_model::DiscoveredModel> {
        if !self.catalog_fallback {
            return Vec::new();
        }
        catalog_chat_models(self.config_name, CATALOG_FALLBACK_LIMIT)
    }
}

const CATALOG_FALLBACK_LIMIT: usize = 40;

/// Active chat models the catalog knows for `provider`: the curated role
/// picks first (marked recommended), then the newest others.
pub(crate) fn catalog_chat_models(
    provider: &str,
    limit: usize,
) -> Vec<crate::discovered_model::DiscoveredModel> {
    use moltis_model_catalog::{ModelKind, global};

    let curated: Vec<String> = global::roles()
        .recommended(provider, ModelKind::Chat)
        .to_vec();
    global::read(|catalog| {
        let mut models: Vec<crate::discovered_model::DiscoveredModel> = Vec::new();
        let mut push = |id: &str, recommended: bool| {
            if models.len() >= limit || models.iter().any(|m| m.id == id) {
                return;
            }
            let resolved = catalog.resolve(provider, id, None);
            if resolved.kind() != ModelKind::Chat || resolved.status().is_deprecated() {
                return;
            }
            models.push(
                crate::discovered_model::DiscoveredModel::new(id, resolved.display_name().name)
                    .with_recommended(recommended),
            );
        };
        for id in &curated {
            push(id, true);
        }
        for model in catalog.models_of_kind(provider, ModelKind::Chat) {
            push(model.id(), false);
        }
        models
    })
}

/// Onboarding card data for a table-driven provider.
#[derive(Debug, Clone, Copy)]
pub struct HostedProviderCard {
    pub name: &'static str,
    pub display_name: &'static str,
    pub env_key: &'static str,
    pub default_base_url: &'static str,
    pub requires_model: bool,
}

/// Cards for the table-driven providers that declare a display name.
#[must_use]
pub fn hosted_provider_cards() -> Vec<HostedProviderCard> {
    OPENAI_COMPAT_PROVIDERS
        .iter()
        .filter(|def| !def.display_name.is_empty())
        .map(|def| HostedProviderCard {
            name: def.config_name,
            display_name: def.display_name,
            env_key: def.env_key,
            default_base_url: def.default_base_url,
            requires_model: def.requires_model,
        })
        .collect()
}

/// Hosts and routers proxy many backends: `strict` tool schemas and the
/// message `name` field are not reliably accepted.
const HOSTED_CAPABILITIES: OpenAiProviderCapabilities = OpenAiProviderCapabilities {
    supports_user_name: false,
    default_strict_tools: false,
    ..OpenAiProviderCapabilities::DEFAULT
};

pub(crate) const OPENAI_COMPAT_PROVIDERS: &[OpenAiCompatDef] = &[
    OpenAiCompatDef {
        config_name: "mistral",
        env_key: "MISTRAL_API_KEY",
        env_base_url_key: "MISTRAL_BASE_URL",
        default_base_url: "https://api.mistral.ai/v1",
        models: MISTRAL_MODELS,
        capabilities: OpenAiProviderCapabilities {
            supports_user_name: false,
            rate_limit_policy: RateLimitPolicy::Mistral,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "openrouter",
        env_key: "OPENROUTER_API_KEY",
        env_base_url_key: "OPENROUTER_BASE_URL",
        default_base_url: "https://openrouter.ai/api/v1",
        capabilities: OpenAiProviderCapabilities {
            default_strict_tools: false,
            cache_control_policy: CacheControlPolicy::OpenRouterAnthropic,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "cerebras",
        env_key: "CEREBRAS_API_KEY",
        env_base_url_key: "CEREBRAS_BASE_URL",
        default_base_url: "https://api.cerebras.ai/v1",
        models: CEREBRAS_MODELS,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "groq",
        env_key: "GROQ_API_KEY",
        env_base_url_key: "GROQ_BASE_URL",
        default_base_url: "https://api.groq.com/openai/v1",
        models: GROQ_MODELS,
        ..OpenAiCompatDef::DEFAULT
    },
    // xAI marks Chat Completions as legacy in favour of Responses but has
    // announced no shutdown; Chat Completions keeps tool calling working.
    OpenAiCompatDef {
        config_name: "xai",
        env_key: "XAI_API_KEY",
        env_base_url_key: "XAI_BASE_URL",
        default_base_url: "https://api.x.ai/v1",
        models: XAI_MODELS,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "minimax",
        env_key: "MINIMAX_API_KEY",
        env_base_url_key: "MINIMAX_BASE_URL",
        default_base_url: "https://api.minimax.io/v1",
        models: MINIMAX_MODELS,
        // MiniMax API does not expose a /models endpoint (returns 404).
        supports_model_discovery: false,
        capabilities: OpenAiProviderCapabilities {
            supports_user_name: false,
            system_message_rewrite: SystemMessageRewriteStrategy::InlineIntoFirstUser,
            // Flash can omit thinking on simple tool calls. Replay reasoning
            // when present, without synthesizing it from visible text.
            optional_reasoning_content_model_prefixes: &["minimax-m3.1-flash"],
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "moonshot",
        env_key: "MOONSHOT_API_KEY",
        env_base_url_key: "MOONSHOT_BASE_URL",
        default_base_url: "https://api.moonshot.ai/v1",
        models: MOONSHOT_MODELS,
        capabilities: OpenAiProviderCapabilities {
            default_reasoning_content_on_tool_messages: true,
            reasoning_effort_policy: ReasoningEffortPolicy::KimiMax,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "zai",
        env_key: "Z_API_KEY",
        env_base_url_key: "Z_BASE_URL",
        default_base_url: "https://api.z.ai/api/paas/v4",
        models: ZAI_MODELS,
        capabilities: OpenAiProviderCapabilities {
            reasoning_effort_policy: ReasoningEffortPolicy::Zai,
            optional_reasoning_content_model_prefixes: &["glm-5.3", "glm-5.2"],
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "zai-code",
        env_key: "Z_CODE_API_KEY",
        env_base_url_key: "Z_CODE_BASE_URL",
        default_base_url: "https://api.z.ai/api/coding/paas/v4",
        models: ZAI_CODE_MODELS,
        capabilities: OpenAiProviderCapabilities {
            reasoning_effort_policy: ReasoningEffortPolicy::Zai,
            optional_reasoning_content_model_prefixes: &["glm-5.3", "glm-5.2"],
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "venice",
        env_key: "VENICE_API_KEY",
        env_base_url_key: "VENICE_BASE_URL",
        default_base_url: "https://api.venice.ai/api/v1",
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "nearai",
        env_key: "NEARAI_API_KEY",
        env_base_url_key: "NEARAI_BASE_URL",
        default_base_url: "https://cloud-api.near.ai/v1",
        models: &[],
        supports_model_discovery: true,
        capabilities: OpenAiProviderCapabilities {
            // NEAR AI does not support the `strict` field in tool schemas.
            default_strict_tools: false,
            omits_strict_tool_field: true,
            probe_output_cap_policy: ProbeOutputCapPolicy::MaxTokens,
            reasoning_effort_policy: ReasoningEffortPolicy::Unsupported,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "deepinfra",
        env_key: "DEEPINFRA_API_KEY",
        env_base_url_key: "DEEPINFRA_BASE_URL",
        default_base_url: "https://api.deepinfra.com/v1/openai",
        models: DEEPINFRA_MODELS,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "deepseek",
        env_key: "DEEPSEEK_API_KEY",
        env_base_url_key: "DEEPSEEK_BASE_URL",
        default_base_url: "https://api.deepseek.com",
        models: DEEPSEEK_MODELS,
        capabilities: OpenAiProviderCapabilities {
            reasoning_effort_policy: ReasoningEffortPolicy::DeepSeek,
            reasoning_content_model_prefixes: &["deepseek-v4", "deepseek-flash"],
            // Thinking with tools requires replay on all assistant turns,
            // including preceding answers that did not call a tool.
            optional_reasoning_content_model_prefixes: &["deepseek-v4", "deepseek-flash"],
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "fireworks",
        env_key: "FIREWORKS_API_KEY",
        env_base_url_key: "FIREWORKS_BASE_URL",
        default_base_url: "https://api.fireworks.ai/inference/v1",
        models: FIREWORKS_MODELS,
        capabilities: OpenAiProviderCapabilities {
            rejects_null_in_enums: true,
            // Kimi router models proxy to Moonshot which requires
            // reasoning_content and rejects strict tool schemas (#810).
            reasoning_content_model_prefixes: FIREWORKS_KIMI_ROUTER_PREFIXES,
            non_strict_tools_model_prefixes: FIREWORKS_KIMI_ROUTER_PREFIXES,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "ollama",
        env_key: "OLLAMA_API_KEY",
        env_base_url_key: "OLLAMA_BASE_URL",
        default_base_url: "http://localhost:11434/v1",
        requires_api_key: false,
        local_only: true,
        capabilities: OpenAiProviderCapabilities {
            probe_fallback_policy: ProbeFallbackPolicy::OllamaNativeShow,
            qwen_models_require_single_leading_system: true,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "lmstudio",
        env_key: "LMSTUDIO_API_KEY",
        env_base_url_key: "LMSTUDIO_BASE_URL",
        default_base_url: "http://127.0.0.1:1234/v1",
        requires_api_key: false,
        local_only: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "alibaba-coding",
        env_key: "ALIBABA_CODING_API_KEY",
        env_base_url_key: "ALIBABA_CODING_BASE_URL",
        default_base_url: "https://coding-intl.dashscope.aliyuncs.com/v1",
        models: ALIBABA_CODING_MODELS,
        capabilities: OpenAiProviderCapabilities {
            qwen_models_require_single_leading_system: true,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "gemini",
        env_key: "GEMINI_API_KEY",
        env_base_url_key: "GEMINI_BASE_URL",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: GEMINI_MODELS,
        capabilities: OpenAiProviderCapabilities {
            default_strict_tools: false,
            requires_gemini_tool_call_extra_content: true,
            ..OpenAiProviderCapabilities::DEFAULT
        },
        ..OpenAiCompatDef::DEFAULT
    },
    // ── Hosted OpenAI-compatible providers: models come from `/models` or,
    // failing that, from the catalog.
    OpenAiCompatDef {
        config_name: "together",
        display_name: "Together AI",
        env_key: "TOGETHER_API_KEY",
        env_base_url_key: "TOGETHER_BASE_URL",
        default_base_url: "https://api.together.ai/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "sambanova",
        display_name: "SambaNova",
        env_key: "SAMBANOVA_API_KEY",
        env_base_url_key: "SAMBANOVA_BASE_URL",
        default_base_url: "https://api.sambanova.ai/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "huggingface",
        display_name: "Hugging Face",
        env_key: "HF_TOKEN",
        env_base_url_key: "HUGGINGFACE_BASE_URL",
        default_base_url: "https://router.huggingface.co/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "nvidia",
        display_name: "NVIDIA",
        env_key: "NVIDIA_API_KEY",
        env_base_url_key: "NVIDIA_BASE_URL",
        default_base_url: "https://integrate.api.nvidia.com/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "novita",
        display_name: "Novita",
        env_key: "NOVITA_API_KEY",
        env_base_url_key: "NOVITA_BASE_URL",
        default_base_url: "https://api.novita.ai/openai",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "chutes",
        display_name: "Chutes",
        env_key: "CHUTES_API_KEY",
        env_base_url_key: "CHUTES_BASE_URL",
        default_base_url: "https://llm.chutes.ai/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "nebius",
        display_name: "Nebius Token Factory",
        env_key: "NEBIUS_API_KEY",
        env_base_url_key: "NEBIUS_BASE_URL",
        default_base_url: "https://api.tokenfactory.nebius.com/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "vercel",
        display_name: "Vercel AI Gateway",
        env_key: "AI_GATEWAY_API_KEY",
        env_base_url_key: "VERCEL_BASE_URL",
        default_base_url: "https://ai-gateway.vercel.sh/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "requesty",
        display_name: "Requesty",
        env_key: "REQUESTY_API_KEY",
        env_base_url_key: "REQUESTY_BASE_URL",
        default_base_url: "https://router.requesty.ai/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "cloudflare",
        display_name: "Cloudflare Workers AI",
        env_key: "CLOUDFLARE_API_KEY",
        env_base_url_key: "CLOUDFLARE_BASE_URL",
        default_base_url: "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1",
        // Workers AI has no OpenAI-style `/models` listing.
        supports_model_discovery: false,
        base_url_vars: &[BaseUrlVar {
            placeholder: "{account_id}",
            env_key: "CLOUDFLARE_ACCOUNT_ID",
            default: None,
        }],
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "baseten",
        display_name: "Baseten",
        env_key: "BASETEN_API_KEY",
        env_base_url_key: "BASETEN_BASE_URL",
        default_base_url: "https://inference.baseten.co/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "stepfun",
        display_name: "StepFun",
        env_key: "STEPFUN_API_KEY",
        env_base_url_key: "STEPFUN_BASE_URL",
        default_base_url: "https://api.stepfun.ai/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "byteplus",
        display_name: "BytePlus ModelArk",
        env_key: "BYTEPLUS_API_KEY",
        env_base_url_key: "BYTEPLUS_BASE_URL",
        default_base_url: "https://ark.ap-southeast.bytepluses.com/api/v3",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "tencent",
        display_name: "Tencent TokenHub",
        env_key: "TENCENT_TOKENHUB_API_KEY",
        env_base_url_key: "TENCENT_BASE_URL",
        default_base_url: "https://tokenhub-intl.tencentcloudmaas.com/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "xiaomi",
        display_name: "Xiaomi MiMo",
        env_key: "XIAOMI_API_KEY",
        env_base_url_key: "XIAOMI_BASE_URL",
        default_base_url: "https://api.xiaomimimo.com/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "longcat",
        display_name: "LongCat",
        env_key: "LONGCAT_API_KEY",
        env_base_url_key: "LONGCAT_BASE_URL",
        default_base_url: "https://api.longcat.chat/openai",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "alibaba",
        display_name: "Alibaba Model Studio",
        env_key: "DASHSCOPE_API_KEY",
        env_base_url_key: "ALIBABA_BASE_URL",
        default_base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        capabilities: OpenAiProviderCapabilities {
            qwen_models_require_single_leading_system: true,
            ..HOSTED_CAPABILITIES
        },
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "cohere",
        display_name: "Cohere",
        env_key: "COHERE_API_KEY",
        env_base_url_key: "COHERE_BASE_URL",
        default_base_url: "https://api.cohere.ai/compatibility/v1",
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "bedrock",
        display_name: "Amazon Bedrock",
        env_key: "AWS_BEARER_TOKEN_BEDROCK",
        env_base_url_key: "BEDROCK_BASE_URL",
        default_base_url: "https://bedrock-mantle.{region}.api.aws/v1",
        // Bedrock API keys are bearer tokens on the OpenAI-compatible
        // mantle endpoint; SigV4 credentials are not supported here.
        base_url_vars: &[BaseUrlVar {
            placeholder: "{region}",
            env_key: "AWS_REGION",
            default: Some("us-east-1"),
        }],
        capabilities: HOSTED_CAPABILITIES,
        catalog_fallback: true,
        ..OpenAiCompatDef::DEFAULT
    },
    OpenAiCompatDef {
        config_name: "azure",
        display_name: "Azure OpenAI",
        env_key: "AZURE_API_KEY",
        env_base_url_key: "AZURE_BASE_URL",
        default_base_url: "https://{resource}.openai.azure.com/openai/v1",
        // `model` is the deployment name, which neither `/models` nor the
        // catalog knows: models must be listed in config.
        supports_model_discovery: false,
        catalog_fallback: false,
        requires_model: true,
        base_url_vars: &[BaseUrlVar {
            placeholder: "{resource}",
            env_key: "AZURE_RESOURCE_NAME",
            default: None,
        }],
        capabilities: HOSTED_CAPABILITIES,
        ..OpenAiCompatDef::DEFAULT
    },
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, crate::catalog_to_discovered};

    #[test]
    fn model_lists_not_empty() {
        assert!(!ANTHROPIC_MODELS.is_empty());
        assert!(!crate::openai::default_model_catalog().is_empty());
        assert!(!MISTRAL_MODELS.is_empty());
        assert!(!CEREBRAS_MODELS.is_empty());
        assert!(!MINIMAX_MODELS.is_empty());
        assert!(!ZAI_MODELS.is_empty());
        assert!(!FIREWORKS_MODELS.is_empty());
        assert!(!DEEPINFRA_MODELS.is_empty());
        assert!(!MOONSHOT_MODELS.is_empty());
        assert!(!GEMINI_MODELS.is_empty());
        assert!(!GROQ_MODELS.is_empty());
        assert!(!XAI_MODELS.is_empty());
    }

    #[test]
    fn fallback_lists_drop_retired_ids() {
        let retired = [
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "mistral-large-latest",
            "codestral-latest",
            "llama-4-scout-17b-16e-instruct",
            "deepseek-chat",
            "deepseek-reasoner",
            "grok-3-mini",
        ];
        for def in OPENAI_COMPAT_PROVIDERS {
            for (id, _) in def.models {
                assert!(
                    !retired.contains(id),
                    "{} still lists retired model {id}",
                    def.config_name
                );
            }
        }
    }

    /// Every id a static fallback list ships must exist in the bundled
    /// catalog for that provider and must not be deprecated there.
    #[test]
    fn fallback_ids_exist_in_catalog_and_are_active() {
        let catalog = moltis_model_catalog::ModelCatalog::bundled().expect("bundled catalog");
        let mut lists: Vec<(&str, Vec<String>)> = OPENAI_COMPAT_PROVIDERS
            .iter()
            .map(|def| {
                (
                    def.config_name,
                    def.models.iter().map(|(id, _)| (*id).to_string()).collect(),
                )
            })
            .collect();
        lists.push((
            "anthropic",
            ANTHROPIC_MODELS
                .iter()
                .map(|(id, _)| (*id).to_string())
                .collect(),
        ));
        lists.push((
            "openai",
            crate::openai::default_model_catalog()
                .into_iter()
                .map(|m| m.id)
                .collect(),
        ));
        #[cfg(feature = "provider-openai-codex")]
        lists.push((
            "openai-codex",
            crate::openai_codex::default_model_catalog()
                .into_iter()
                .map(|m| m.id)
                .collect(),
        ));
        #[cfg(feature = "provider-github-copilot")]
        lists.push((
            "github-copilot",
            crate::github_copilot::default_model_catalog()
                .into_iter()
                .map(|m| m.id)
                .collect(),
        ));

        let mut problems = Vec::new();
        for (provider, ids) in lists {
            if moltis_model_catalog::providers::models_dev_ids(provider).is_empty() {
                continue;
            }
            let known = catalog.known_ids(provider);
            for id in ids {
                if !known.contains(&id) {
                    problems.push(format!("{provider}::{id} is not in the catalog"));
                    continue;
                }
                if catalog
                    .resolve(provider, &id, None)
                    .status()
                    .is_deprecated()
                {
                    problems.push(format!("{provider}::{id} is deprecated"));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "stale fallback ids:\n{}",
            problems.join("\n")
        );
    }

    #[test]
    fn groq_and_xai_use_openai_compat_table() {
        let groq = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|def| def.config_name == "groq")
            .expect("groq entry");
        assert_eq!(groq.default_base_url, "https://api.groq.com/openai/v1");
        let xai = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|def| def.config_name == "xai")
            .expect("xai entry");
        assert_eq!(xai.default_base_url, "https://api.x.ai/v1");
        assert!(xai.models.iter().all(|(id, _)| id.starts_with("grok-4")));
    }

    #[test]
    fn model_lists_have_unique_ids() {
        let openai_models = crate::openai::default_model_catalog();
        let mut openai_ids: Vec<&str> = openai_models.iter().map(|m| m.id.as_str()).collect();
        openai_ids.sort();
        openai_ids.dedup();
        assert_eq!(
            openai_ids.len(),
            openai_models.len(),
            "duplicate OpenAI model IDs found"
        );

        for models in [
            ANTHROPIC_MODELS,
            MISTRAL_MODELS,
            CEREBRAS_MODELS,
            DEEPINFRA_MODELS,
            MINIMAX_MODELS,
            ZAI_MODELS,
            FIREWORKS_MODELS,
            MOONSHOT_MODELS,
            GEMINI_MODELS,
        ] {
            let mut ids: Vec<&str> = models.iter().map(|(id, _)| *id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), models.len(), "duplicate model IDs found");
        }
    }

    #[test]
    fn minimax_catalog_starts_with_target_models() {
        assert_eq!(MINIMAX_MODELS, &[
            ("MiniMax-M3.1-Flash", "MiniMax M3.1 Flash"),
            ("MiniMax-M3", "MiniMax M3"),
        ]);
    }

    #[test]
    fn openai_catalog_includes_gpt_6_models_and_drops_shutdown_ones() {
        let models = crate::openai::default_model_catalog();
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();

        for model_id in [
            "gpt-6.2-sol",
            "gpt-6.2-astra",
            "gpt-6.1-terra",
            "gpt-6-luna",
        ] {
            assert!(
                ids.contains(&model_id),
                "missing {model_id} in OpenAI defaults"
            );
        }
        for shutdown in ["gpt-5-mini", "gpt-5.2", "gpt-5.2-chat-latest"] {
            assert!(!ids.contains(&shutdown), "{shutdown} shuts down 2026-12-11");
        }
    }

    #[test]
    fn anthropic_catalog_uses_documented_claude_46_aliases() {
        let anthropic_ids: Vec<&str> = ANTHROPIC_MODELS.iter().map(|(id, _)| *id).collect();

        assert!(anthropic_ids.contains(&"claude-opus-4-6"));
        assert!(anthropic_ids.contains(&"claude-sonnet-4-6"));
        assert!(!anthropic_ids.contains(&"claude-opus-4-6-20260301"));
        assert!(!anthropic_ids.contains(&"claude-sonnet-4-6-20260301"));
        assert!(!anthropic_ids.contains(&"claude-haiku-4-6-20260301"));
    }

    fn hosted(name: &str) -> &'static OpenAiCompatDef {
        OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|def| def.config_name == name)
            .expect("hosted provider")
    }

    #[test]
    fn base_url_placeholders_are_filled_from_env() {
        let config = moltis_config::schema::ProvidersConfig::default();
        let env = std::collections::HashMap::from([
            ("AWS_REGION".to_string(), "eu-west-3".to_string()),
            ("CLOUDFLARE_ACCOUNT_ID".to_string(), "abc_123".to_string()),
        ]);
        assert_eq!(
            hosted("bedrock").resolve_base_url(&config, &env).as_deref(),
            Some("https://bedrock-mantle.eu-west-3.api.aws/v1")
        );
        assert_eq!(
            hosted("cloudflare")
                .resolve_base_url(&config, &env)
                .as_deref(),
            Some("https://api.cloudflare.com/client/v4/accounts/abc_123/ai/v1")
        );
    }

    #[test]
    fn base_url_placeholder_defaults_and_missing_values() {
        let config = moltis_config::schema::ProvidersConfig::default();
        let env = std::collections::HashMap::new();
        if std::env::var_os("AWS_REGION").is_none() {
            assert_eq!(
                hosted("bedrock").resolve_base_url(&config, &env).as_deref(),
                Some("https://bedrock-mantle.us-east-1.api.aws/v1")
            );
        }
        if std::env::var_os("AZURE_RESOURCE_NAME").is_none() {
            assert_eq!(hosted("azure").resolve_base_url(&config, &env), None);
        }
    }

    #[test]
    fn base_url_placeholder_rejects_values_that_change_the_host() {
        let config = moltis_config::schema::ProvidersConfig::default();
        for bad in ["evil.com/x", "a.b", "x?y", "", "a b"] {
            let env =
                std::collections::HashMap::from([("AZURE_RESOURCE_NAME".to_string(), bad.into())]);
            assert_eq!(
                hosted("azure").resolve_base_url(&config, &env),
                None,
                "{bad:?} accepted"
            );
        }
    }

    #[test]
    fn configured_base_url_wins_over_the_template() {
        let mut config = moltis_config::schema::ProvidersConfig::default();
        config
            .providers
            .insert("azure".into(), moltis_config::schema::ProviderEntry {
                base_url: Some("https://contoso.openai.azure.com/openai/v1".into()),
                ..Default::default()
            });
        assert_eq!(
            hosted("azure")
                .resolve_base_url(&config, &std::collections::HashMap::new())
                .as_deref(),
            Some("https://contoso.openai.azure.com/openai/v1")
        );
    }

    #[test]
    fn catalog_fallback_lists_active_chat_models() {
        let models = hosted("together").catalog_models();
        assert!(!models.is_empty());
        assert!(models.len() <= CATALOG_FALLBACK_LIMIT);
        moltis_model_catalog::global::read(|catalog| {
            for model in &models {
                let resolved = catalog.resolve("together", &model.id, None);
                assert_eq!(resolved.kind(), moltis_model_catalog::ModelKind::Chat);
                assert!(!resolved.status().is_deprecated(), "{}", model.id);
            }
        });
        assert!(hosted("azure").catalog_models().is_empty());
        assert!(hosted("venice").catalog_models().is_empty());
    }

    #[test]
    fn every_hosted_provider_maps_to_models_dev_and_has_a_card() {
        let cards = hosted_provider_cards();
        for def in OPENAI_COMPAT_PROVIDERS
            .iter()
            .filter(|def| !def.display_name.is_empty())
        {
            assert!(
                !moltis_model_catalog::providers::models_dev_ids(def.config_name).is_empty(),
                "{} has no models.dev mapping",
                def.config_name
            );
            assert!(cards.iter().any(|card| card.name == def.config_name));
            let template_vars = def.default_base_url.matches('{').count();
            assert_eq!(
                template_vars,
                def.base_url_vars.len(),
                "{}",
                def.config_name
            );
        }
    }

    #[test]
    fn catalog_fallback_providers_have_catalog_models() {
        let catalog = moltis_model_catalog::ModelCatalog::bundled().expect("bundled catalog");
        for def in OPENAI_COMPAT_PROVIDERS
            .iter()
            .filter(|def| def.catalog_fallback)
        {
            assert!(
                !catalog
                    .models_of_kind(def.config_name, moltis_model_catalog::ModelKind::Chat)
                    .is_empty(),
                "{} has no chat models in the bundled catalog",
                def.config_name
            );
        }
    }

    #[test]
    fn openai_compat_providers_have_unique_names() {
        let mut names: Vec<&str> = OPENAI_COMPAT_PROVIDERS
            .iter()
            .map(|d| d.config_name)
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), OPENAI_COMPAT_PROVIDERS.len());
    }

    #[test]
    fn openai_compat_providers_have_valid_urls() {
        for def in OPENAI_COMPAT_PROVIDERS {
            assert!(
                def.default_base_url.starts_with("http://")
                    || def.default_base_url.starts_with("https://"),
                "{}: invalid base URL: {}",
                def.config_name,
                def.default_base_url
            );
        }
    }

    #[test]
    fn openai_compat_providers_env_keys_not_empty() {
        for def in OPENAI_COMPAT_PROVIDERS {
            assert!(
                !def.env_key.is_empty(),
                "{}: env_key is empty",
                def.config_name
            );
            assert!(
                !def.env_base_url_key.is_empty(),
                "{}: env_base_url_key is empty",
                def.config_name
            );
        }
    }

    #[test]
    fn strict_name_providers_disable_user_names() {
        let mistral = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "mistral")
            .expect("mistral entry must exist");
        let minimax = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "minimax")
            .expect("minimax entry must exist");

        assert!(!mistral.capabilities.supports_user_name);
        assert!(!minimax.capabilities.supports_user_name);
    }

    #[test]
    fn deepseek_preserves_reasoning_for_current_and_legacy_models() {
        let deepseek = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "deepseek")
            .expect("deepseek entry must exist");

        assert!(
            !deepseek
                .capabilities
                .default_reasoning_content_on_tool_messages
        );
        assert_eq!(deepseek.capabilities.reasoning_content_model_prefixes, &[
            "deepseek-v4",
            "deepseek-flash"
        ]);
        assert_eq!(
            deepseek
                .capabilities
                .optional_reasoning_content_model_prefixes,
            deepseek.capabilities.reasoning_content_model_prefixes
        );
    }

    #[test]
    fn alibaba_coding_provider_exists() {
        let alibaba = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "alibaba-coding")
            .expect("alibaba-coding entry must exist");
        assert_eq!(alibaba.env_key, "ALIBABA_CODING_API_KEY");
        assert_eq!(
            alibaba.default_base_url,
            "https://coding-intl.dashscope.aliyuncs.com/v1"
        );
        assert!(alibaba.requires_api_key);
        assert!(!alibaba.local_only);
        assert!(alibaba.supports_model_discovery);
    }

    #[test]
    fn nearai_provider_exists() {
        let nearai = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "nearai")
            .expect("nearai entry must exist");
        assert_eq!(nearai.env_key, "NEARAI_API_KEY");
        assert_eq!(nearai.env_base_url_key, "NEARAI_BASE_URL");
        assert_eq!(nearai.default_base_url, "https://cloud-api.near.ai/v1");
        assert!(nearai.requires_api_key);
        assert!(!nearai.local_only);
        assert!(nearai.supports_model_discovery);
        assert!(nearai.capabilities.omits_strict_tool_field);
        assert_eq!(
            nearai.capabilities.probe_output_cap_policy,
            ProbeOutputCapPolicy::MaxTokens
        );
        assert_eq!(
            nearai.capabilities.reasoning_effort_policy,
            ReasoningEffortPolicy::Unsupported
        );
    }

    #[test]
    fn alibaba_coding_models_no_duplicates() {
        let mut ids: Vec<&str> = ALIBABA_CODING_MODELS.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            ALIBABA_CODING_MODELS.len(),
            "duplicate model IDs"
        );
    }

    #[test]
    fn alibaba_coding_models_have_recommended() {
        let discovered = catalog_to_discovered(ALIBABA_CODING_MODELS, 4);
        let recommended_count = discovered.iter().filter(|m| m.recommended).count();
        assert_eq!(recommended_count, 4);
    }

    #[test]
    fn ollama_default_base_url_uses_localhost() {
        let ollama = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "ollama")
            .expect("ollama entry must exist");
        assert!(
            ollama.default_base_url.contains("localhost"),
            "expected 'localhost' in Ollama default_base_url, got: {}",
            ollama.default_base_url,
        );
    }

    #[test]
    fn fireworks_kimi_router_prefixes_cover_router_and_catalog_models() {
        let fireworks = OPENAI_COMPAT_PROVIDERS
            .iter()
            .find(|d| d.config_name == "fireworks")
            .expect("fireworks entry must exist");

        let prefixes = fireworks.capabilities.reasoning_content_model_prefixes;
        let matches = |id: &str| prefixes.iter().any(|p| id.starts_with(p));

        assert!(matches("accounts/fireworks/routers/kimi-k2p5-turbo"));
        assert!(matches("accounts/fireworks/models/kimi-k2p6"));
        assert!(matches("accounts/fireworks/models/kimi-k3"));
        assert!(matches("accounts/fireworks/routers/kimi-k3-fast"));
        assert!(matches("accounts/fireworks/routers/kimi-k3-us"));
        assert!(!matches("accounts/fireworks/models/gpt-oss-120b"));

        // non_strict_tools_model_prefixes should use the same prefixes
        assert_eq!(
            fireworks.capabilities.reasoning_content_model_prefixes,
            fireworks.capabilities.non_strict_tools_model_prefixes,
        );
    }

    /// Cross-validate that every provider registered in this crate appears in
    /// the canonical `KNOWN_PROVIDER_NAMES` list in `moltis-config`.
    ///
    /// If this test fails, you added a provider to `moltis-providers` without
    /// updating `crates/config/src/schema/providers.rs::KNOWN_PROVIDER_NAMES`.
    #[test]
    fn all_registered_providers_in_canonical_known_list() {
        use moltis_config::schema::KNOWN_PROVIDER_NAMES;

        // Built-in providers
        let mut provider_names: Vec<&str> = vec!["anthropic", "openai"];

        // OpenAI-compatible table-driven providers
        for def in OPENAI_COMPAT_PROVIDERS {
            provider_names.push(def.config_name);
        }

        // Feature-gated providers (always check names, regardless of feature).
        //
        // NOTE: This list must be maintained manually because `#[cfg(feature)]`
        // attributes make it impossible to discover these names at test time
        // when the feature is disabled.  When adding a new feature-gated
        // provider registration in `registry/registration.rs` (e.g. a new
        // `register_*_providers` method gated behind a cargo feature), add its
        // config name here too.
        provider_names.extend_from_slice(&[
            "github-copilot",
            "kimi-code",
            "local-llm",
            "openai-codex",
        ]);

        for name in &provider_names {
            assert!(
                KNOWN_PROVIDER_NAMES.contains(name),
                "provider \"{name}\" is registered in moltis-providers but missing from \
                 KNOWN_PROVIDER_NAMES in crates/config/src/schema/providers.rs — add it there"
            );
        }
    }

    /// Ensure `KNOWN_PROVIDER_NAMES` has no duplicates.
    #[test]
    fn canonical_known_list_has_no_duplicates() {
        use moltis_config::schema::KNOWN_PROVIDER_NAMES;

        let mut sorted: Vec<&str> = KNOWN_PROVIDER_NAMES.to_vec();
        sorted.sort();
        for window in sorted.windows(2) {
            assert_ne!(
                window[0], window[1],
                "duplicate entry \"{0}\" in KNOWN_PROVIDER_NAMES",
                window[0]
            );
        }
    }
}
