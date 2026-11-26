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
