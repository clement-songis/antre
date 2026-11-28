use {
    super::*,
    crate::schema::{MemoryReranker, MoltisConfig},
};

/// Voice fallback chains and memory reranker settings.
pub(super) fn check_media_settings(config: &MoltisConfig, diagnostics: &mut Vec<Diagnostic>) {
    if config.memory.reranker == Some(MemoryReranker::Custom)
        && config.memory.rerank_base_url.is_none()
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            category: "invalid-value",
            path: "memory.rerank_base_url".into(),
            message: "memory.reranker = \"custom\" needs memory.rerank_base_url".into(),
        });
    }

    // Redundant voice fallback entries: the active provider or a repeat
    // would be tried at most once anyway.
    check_voice_fallback(
        "STT",
        "voice.stt.fallback",
        config.voice.stt.provider,
        &config.voice.stt.fallback,
        |provider| provider.as_str(),
        diagnostics,
    );
    check_voice_fallback(
        "TTS",
        "voice.tts.fallback",
        config.voice.tts.provider,
        &config.voice.tts.fallback,
        |provider| provider.as_str(),
        diagnostics,
    );
}

fn check_voice_fallback<P: Copy + PartialEq>(
    kind: &str,
    path: &str,
    active: Option<P>,
    fallback: &[P],
    name: impl Fn(P) -> &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (idx, provider) in fallback.iter().enumerate() {
        let is_active = active == Some(*provider);
        let is_repeat = fallback[..idx].contains(provider);
        if !is_active && !is_repeat {
            continue;
        }
        let reason = if is_active {
            "is already the active provider"
        } else {
            "is listed more than once"
        };
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            category: "invalid-value",
            path: format!("{path}[{idx}]"),
            message: format!(
                "{kind} fallback provider \"{}\" {reason}; it is tried only once",
                name(*provider)
            ),
        });
    }
}
