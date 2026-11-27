/// Current GitHub Copilot model IDs. Live `/models` discovery is preferred;
/// this catalog is used when discovery is unavailable.
pub(super) const COPILOT_MODELS: &[CopilotCatalogModel] = &[
    responses("gpt-6.2-sol", "GPT-6.2 Sol (Copilot)"),
    chat("claude-opus-5.5", "Claude Opus 5.5 (Copilot)"),
    chat("claude-sonnet-5.5", "Claude Sonnet 5.5 (Copilot)"),
    responses("gpt-6.2-astra", "GPT-6.2 Astra (Copilot)"),
    responses("gpt-6.1-sol", "GPT-6.1 Sol (Copilot)"),
    responses("gpt-6.1-terra", "GPT-6.1 Terra (Copilot)"),
    responses("gpt-6-astra", "GPT-6 Astra (Copilot)"),
    responses("gpt-6-sol", "GPT-6 Sol (Copilot)"),
    responses("gpt-6-luna", "GPT-6 Luna (Copilot)"),
    responses("gpt-5.6-sol", "GPT-5.6 Sol (Copilot)"),
    responses("gpt-5.6-luna", "GPT-5.6 Luna (Copilot)"),
    responses("gpt-5.4-nano", "GPT-5.4 nano (Copilot)"),
    responses("gpt-5.3-codex", "GPT-5.3-Codex (Copilot)"),
    chat("claude-fable-5.2", "Claude Fable 5.2 (Copilot)"),
    chat("claude-fable-5.1", "Claude Fable 5.1 (Copilot)"),
    chat("claude-fable-5", "Claude Fable 5 (Copilot)"),
    chat("claude-sonnet-5", "Claude Sonnet 5 (Copilot)"),
    chat("claude-haiku-5.5", "Claude Haiku 5.5 (Copilot)"),
    chat("claude-haiku-4.5", "Claude Haiku 4.5 (Copilot)"),
    chat("gemini-3.8-flash", "Gemini 3.8 Flash (Copilot)"),
    chat("gemini-4-argon", "Gemini 4 Argon (Copilot)"),
    chat("gemini-4-carbon", "Gemini 4 Carbon (Copilot)"),
    chat("gemini-4-hydrogen", "Gemini 4 Hydrogen (Copilot)"),
    chat("gemini-4-neon", "Gemini 4 Neon (Copilot)"),
    chat("kimi-k3", "Kimi K3 (Copilot)"),
];

pub(super) struct CopilotCatalogModel {
    id: &'static str,
    display_name: &'static str,
    requires_responses_api: bool,
}

const fn chat(id: &'static str, display_name: &'static str) -> CopilotCatalogModel {
    CopilotCatalogModel {
        id,
        display_name,
        requires_responses_api: false,
    }
}

const fn responses(id: &'static str, display_name: &'static str) -> CopilotCatalogModel {
    CopilotCatalogModel {
        id,
        display_name,
        requires_responses_api: true,
    }
}

pub fn default_model_catalog() -> Vec<super::super::DiscoveredModel> {
    COPILOT_MODELS
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let mut capabilities = super::super::ModelCapabilities::infer(model.id);
            capabilities.requires_responses_api = model.requires_responses_api;
            if let Some(context_window) =
                super::super::model_capabilities::context_window_fallback_for_model(
                    super::super::model_capabilities::ContextWindowFallbackScope::GitHubCopilot,
                    model.id,
                )
            {
                capabilities.context_window = context_window;
            }
            super::super::DiscoveredModel::new(model.id, model.display_name)
                .with_recommended(index < 3)
                .with_capabilities(capabilities)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_current_copilot_model_ids() {
        for expected in [
            "gpt-6.2-sol",
            "gpt-6-luna",
            "claude-opus-5.5",
            "claude-sonnet-5.5",
            "gemini-3.8-flash",
            "gemini-4-argon",
            "kimi-k3",
        ] {
            assert!(
                COPILOT_MODELS.iter().any(|model| model.id == expected),
                "missing current Copilot model ID: {expected}"
            );
        }
    }

    #[test]
    fn catalog_marks_responses_api_models_as_capability() {
        let catalog = default_model_catalog();
        let Some(luna) = catalog.iter().find(|model| model.id == "gpt-5.6-luna") else {
            panic!("missing gpt-6-luna model");
        };
        let Some(luna_capabilities) = luna.capabilities else {
            panic!("missing gpt-6-luna capabilities");
        };
        assert!(luna_capabilities.requires_responses_api);

        let Some(flash) = catalog.iter().find(|model| model.id == "gemini-3.8-flash") else {
            panic!("missing gemini-3.8-flash model");
        };
        let Some(flash_capabilities) = flash.capabilities else {
            panic!("missing gemini-3.8-flash capabilities");
        };
        assert!(!flash_capabilities.requires_responses_api);
    }

    #[test]
    fn catalog_marks_copilot_extended_context_models_as_capability() {
        let catalog = default_model_catalog();
        let Some(opus) = catalog.iter().find(|model| model.id == "claude-opus-5.5") else {
            panic!("missing claude-opus-5.5 model");
        };
        let Some(opus_capabilities) = opus.capabilities else {
            panic!("missing claude-opus-5.5 capabilities");
        };
        assert_eq!(opus_capabilities.context_window, 1_000_000);

        let Some(haiku) = catalog.iter().find(|model| model.id == "claude-haiku-4.5") else {
            panic!("missing claude-haiku-4.5 model");
        };
        let Some(haiku_capabilities) = haiku.capabilities else {
            panic!("missing claude-haiku-4.5 capabilities");
        };
        assert_eq!(haiku_capabilities.context_window, 200_000);
    }
}
