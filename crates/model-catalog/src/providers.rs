//! Mapping between Moltis provider ids and models.dev provider ids.

/// models.dev provider ids holding metadata for a Moltis provider, most
/// specific first.
#[must_use]
pub fn models_dev_ids(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &["anthropic"],
        "openai" | "openai-codex" => &["openai"],
        "github-copilot" => &["github-copilot"],
        "gemini" => &["google"],
        "mistral" => &["mistral"],
        "cerebras" => &["cerebras"],
        "groq" => &["groq"],
        "xai" => &["xai"],
        "deepseek" => &["deepseek"],
        "moonshot" => &["moonshotai"],
        "kimi-code" => &["kimi-code-plan-global", "moonshotai"],
        "zai" => &["zai"],
        "zai-code" => &["zai-coding-plan", "zai"],
        "minimax" => &["minimax"],
        "fireworks" => &["fireworks-ai"],
        "deepinfra" => &["deepinfra"],
        "openrouter" => &["openrouter"],
        "venice" => &["venice"],
        "nearai" => &["nearai"],
        "lmstudio" => &["lmstudio"],
        "alibaba-coding" => &["alibaba-coding-plan"],
        "opencode-zen" => &["opencode"],
        "perplexity" => &["perplexity"],
        _ => &[],
    }
}

/// models.dev providers whose models are first-party, searched by id when a
/// custom provider (or one without a mapping) serves a well-known model.
pub const FIRST_PARTY_MODELS_DEV_IDS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "xai",
    "mistral",
    "deepseek",
    "moonshotai",
    "zai",
    "minimax",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_providers_map_to_models_dev() {
        for provider in [
            "anthropic",
            "openai",
            "gemini",
            "zai-code",
            "kimi-code",
            "groq",
            "xai",
        ] {
            assert!(!models_dev_ids(provider).is_empty(), "{provider}");
        }
        assert!(models_dev_ids("custom-foo").is_empty());
        assert_eq!(models_dev_ids("gemini"), ["google"]);
    }
}
