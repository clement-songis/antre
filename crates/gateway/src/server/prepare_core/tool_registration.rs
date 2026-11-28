#[cfg(feature = "telephony")]
pub(super) async fn register_voice_call_tool(
    tool_registry: &mut antre_agents::tool_registry::ToolRegistry,
    state: &crate::state::GatewayState,
) {
    let webhook_base = state
        .config
        .server
        .effective_external_url()
        .unwrap_or_default();
    if let Some(ref tp) = state.services.telephony_plugin {
        let voice_tool =
            antre_telephony::VoiceCallTool::new(webhook_base, std::sync::Arc::clone(tp));
        tool_registry.register(Box::new(voice_tool));
    }
}

/// `generate_image`: Codex OAuth first (covered by the ChatGPT plan), then
/// the first of OpenAI, Gemini and xAI with an API key.
pub(super) fn register_image_generation_tool(
    tool_registry: &mut antre_agents::tool_registry::ToolRegistry,
    providers: &antre_config::schema::ProvidersConfig,
    env_overrides: &std::collections::HashMap<String, String>,
) {
    #[cfg(feature = "provider-openai-codex")]
    if antre_providers::openai_codex::has_stored_tokens() {
        tool_registry.register(Box::new(
            antre_tools::image_generation::GenerateImageTool::new(),
        ));
        return;
    }
    if let Some(images) =
        antre_providers::images_api::ImagesApiProvider::from_config(providers, env_overrides)
    {
        tool_registry.register(Box::new(
            antre_tools::image_generation::GenerateImageTool::with_api(images),
        ));
    }
}
