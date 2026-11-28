use super::*;

#[test]
fn unknown_voice_tts_list_provider_warned() {
    let toml = r#"
[voice.tts]
providers = ["openai-tts", "not-a-provider"]
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "voice.tts.providers[1]");
    assert!(
        warning.is_some(),
        "expected warning for unknown voice.tts.providers entry"
    );
}

#[test]
fn unknown_voice_stt_list_provider_warned() {
    let toml = r#"
[voice.stt]
providers = ["whisper", "not-a-provider"]
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "voice.stt.providers[1]");
    assert!(
        warning.is_some(),
        "expected warning for unknown voice.stt.providers entry"
    );
}

#[test]
fn known_voice_provider_list_entries_not_warned() {
    let toml = r#"
[voice.tts]
providers = ["openai", "google-tts", "coqui"]

[voice.stt]
providers = ["elevenlabs", "whisper-cli", "sherpa-onnx"]
"#;
    let result = validate_toml_str(toml);
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| {
            d.category == "unknown-field"
                && (d.path.starts_with("voice.tts.providers")
                    || d.path.starts_with("voice.stt.providers"))
        })
        .collect();
    assert!(
        warnings.is_empty(),
        "known voice provider list values should not warn: {warnings:?}"
    );
}

#[test]
fn gemini_stt_section_is_known() {
    let toml = r#"
[voice.stt]
provider = "gemini"
providers = ["gemini"]

[voice.stt.gemini]
enabled = true
model = "gemini-3.5-transcribe"
language = "fr-FR"
"#;
    let result = validate_toml_str(toml);
    let gemini_diagnostics: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("voice.stt"))
        .collect();
    assert!(
        gemini_diagnostics.is_empty(),
        "gemini STT config should validate cleanly: {gemini_diagnostics:?}"
    );

    let config: AntreConfig = toml::from_str(toml).unwrap();
    assert_eq!(
        config.voice.stt.provider,
        Some(crate::schema::VoiceSttProvider::Gemini)
    );
    assert_eq!(
        config.voice.stt.gemini.model.as_deref(),
        Some("gemini-3.5-transcribe")
    );
}

#[test]
fn voice_stt_fallback_accepts_known_providers() {
    let toml = r#"
[voice.stt]
provider = "google"
fallback = ["whisper", "groq"]
"#;
    let result = validate_toml_str(toml);
    let fallback_diags: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("voice.stt.fallback"))
        .collect();
    assert!(
        fallback_diags.is_empty(),
        "valid fallback list should not produce diagnostics: {fallback_diags:?}"
    );
}

#[test]
fn voice_stt_streaming_fields_are_known() {
    let toml = r#"
[voice.stt]
streaming = true
max_stream_seconds = 120

[voice.stt.whisper]
realtime_model = "gpt-live-transcribe"

[voice.stt.gemini]
realtime_model = "gemini-3.5-transcribe-live"
"#;
    let result = validate_toml_str(toml);
    let unknown: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("voice.stt"))
        .collect();
    assert!(unknown.is_empty(), "unexpected diagnostics: {unknown:?}");
}

#[test]
fn zero_max_stream_seconds_warned() {
    let toml = r#"
[voice.stt]
max_stream_seconds = 0
"#;
    let result = validate_toml_str(toml);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.path == "voice.stt.max_stream_seconds"),
        "expected warning for max_stream_seconds = 0"
    );
}

#[test]
fn voice_stt_fallback_warns_on_active_and_repeated_provider() {
    let toml = r#"
[voice.stt]
provider = "google"
fallback = ["google", "whisper", "whisper"]
"#;
    let result = validate_toml_str(toml);
    let paths: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("voice.stt.fallback"))
        .map(|d| d.path.as_str())
        .collect();
    assert_eq!(paths, ["voice.stt.fallback[0]", "voice.stt.fallback[2]"]);
}

#[test]
fn voice_stt_fallback_rejects_unknown_provider() {
    let toml = r#"
[voice.stt]
fallback = ["not-a-provider"]
"#;
    let result = validate_toml_str(toml);
    assert!(
        result.has_errors(),
        "unknown fallback provider should fail typed deserialization: {:?}",
        result.diagnostics
    );
}

#[test]
fn voice_tts_fallback_accepts_known_providers_and_warns_on_repeats() {
    let toml = r#"
[voice.tts]
provider = "openai"
fallback = ["openai", "google", "piper", "google"]
"#;
    let result = validate_toml_str(toml);
    let paths: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("voice.tts.fallback"))
        .map(|d| d.path.as_str())
        .collect();
    assert_eq!(paths, ["voice.tts.fallback[0]", "voice.tts.fallback[3]"]);
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.category != "unknown-field"),
        "{:?}",
        result.diagnostics
    );
}
