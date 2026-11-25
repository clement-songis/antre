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
fn voice_stt_streaming_fields_are_known() {
    let toml = r#"
[voice.stt]
streaming = true
max_stream_seconds = 120

[voice.stt.whisper]
realtime_model = "gpt-live-transcribe"
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
