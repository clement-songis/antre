use super::*;

fn looks_like_env_var(value: &str) -> bool {
    // ${VAR} or $VAR (POSIX)
    if value.starts_with('$') {
        return true;
    }
    // %VAR% (Windows)
    if value.starts_with('%') && value.ends_with('%') && value.len() > 2 {
        return true;
    }
    false
}

pub(super) fn check_plaintext_api_keys(config: &MoltisConfig, diagnostics: &mut Vec<Diagnostic>) {
    // LLM provider keys
    for (name, entry) in &config.providers.providers {
        if let Some(ref key) = entry.api_key {
            let value = key.expose_secret();
            if !value.is_empty() && !looks_like_env_var(value) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    category: "security",
                    path: format!("providers.{name}.api_key"),
                    message: format!(
                        "API key for provider '{name}' is stored as plain text in the config file. \
                         Use an environment variable (api_key = \"${{{}}}\") or save it via the \
                         web UI so it is stored in the credential store instead.",
                        name.to_uppercase().replace('-', "_") + "_API_KEY"
                    ),
                });
            }
        }
    }

    // Voice TTS keys
    let voice_tts_keys: &[(&str, &Option<secrecy::Secret<String>>)] = &[
        (
            "voice.tts.elevenlabs.api_key",
            &config.voice.tts.elevenlabs.api_key,
        ),
        ("voice.tts.openai.api_key", &config.voice.tts.openai.api_key),
        ("voice.tts.google.api_key", &config.voice.tts.google.api_key),
    ];
    for (path, key) in voice_tts_keys {
        if let Some(k) = key {
            let value = k.expose_secret();
            if !value.is_empty() && !looks_like_env_var(value) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    category: "security",
                    path: (*path).into(),
                    message: format!(
                        "Voice API key at {path} is stored as plain text in the config file. \
                         Save it via the web UI so it is stored in the credential store instead."
                    ),
                });
            }
        }
    }

    // Voice STT keys
    let voice_stt_keys: &[(&str, &Option<secrecy::Secret<String>>)] = &[
        (
            "voice.stt.whisper.api_key",
            &config.voice.stt.whisper.api_key,
        ),
        ("voice.stt.groq.api_key", &config.voice.stt.groq.api_key),
        (
            "voice.stt.deepgram.api_key",
            &config.voice.stt.deepgram.api_key,
        ),
        ("voice.stt.google.api_key", &config.voice.stt.google.api_key),
        (
            "voice.stt.mistral.api_key",
            &config.voice.stt.mistral.api_key,
        ),
        ("voice.stt.gemini.api_key", &config.voice.stt.gemini.api_key),
        (
            "voice.stt.elevenlabs.api_key",
            &config.voice.stt.elevenlabs.api_key,
        ),
    ];
    for (path, key) in voice_stt_keys {
        if let Some(k) = key {
            let value = k.expose_secret();
            if !value.is_empty() && !looks_like_env_var(value) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    category: "security",
                    path: (*path).into(),
                    message: format!(
                        "Voice API key at {path} is stored as plain text in the config file. \
                         Save it via the web UI so it is stored in the credential store instead."
                    ),
                });
            }
        }
    }
}

/// Check that file paths referenced in TLS config exist on disk.
pub(crate) fn check_file_references(
    toml_str: &str,
    _config_path: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Only check if we can parse the config
    let Ok(config) = toml::from_str::<MoltisConfig>(toml_str) else {
        return;
    };

    let file_refs: &[(&str, &Option<String>)] = &[
        ("tls.cert_path", &config.tls.cert_path),
        ("tls.key_path", &config.tls.key_path),
        ("tls.ca_cert_path", &config.tls.ca_cert_path),
    ];

    for (path_name, value) in file_refs {
        if let Some(file_path) = value {
            let p = Path::new(file_path);
            if !p.exists() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    category: "file-ref",
                    path: (*path_name).into(),
                    message: format!("file not found: {file_path}"),
                });
            }
        }
    }

    // Agent preset MCP allow/deny server validation — warn on unknown server names.
    // Merge servers from both moltis.toml [mcp.servers] and the persistent
    // mcp-servers.json registry file so we don't false-positive on servers
    // added via the API.
    let mut known_mcp_servers: std::collections::HashSet<String> = config
        .mcp
        .servers
        .keys()
        .map(|k| k.as_str().to_string())
        .collect();
    let registry_path = crate::data_dir().join("mcp-servers.json");
    if let Ok(data) = std::fs::read_to_string(&registry_path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
        && let Some(servers) = json.get("servers").and_then(|v| v.as_object())
    {
        known_mcp_servers.extend(servers.keys().cloned());
    }
    for (preset_name, preset) in &config.agents.presets {
        let servers = match &preset.mcp {
            crate::schema::PresetMcpPolicy::Allow(s) | crate::schema::PresetMcpPolicy::Deny(s) => s,
            crate::schema::PresetMcpPolicy::All => continue,
        };
        for server in servers {
            if !known_mcp_servers.contains(server.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    category: "agents",
                    path: format!("agents.presets.{preset_name}.mcp"),
                    message: format!(
                        "MCP server '{}' referenced in preset but not configured in [mcp.servers] or mcp-servers.json",
                        server
                    ),
                });
            }
        }
    }
}
