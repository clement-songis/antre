//! Last-resort display names: turn a wire id into something readable.

use serde::Serialize;

/// A display name with an optional secondary version label (date or
/// snapshot suffix) that pickers show next to the name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrettyName {
    pub name: String,
    pub version: Option<String>,
}

/// Words with a fixed casing.
fn known_casing(word: &str) -> Option<&'static str> {
    Some(match word {
        "gpt" => "GPT",
        "glm" => "GLM",
        "oss" => "OSS",
        "tts" => "TTS",
        "stt" => "STT",
        "vl" => "VL",
        "ai" => "AI",
        "qwen" => "Qwen",
        "deepseek" => "DeepSeek",
        "minimax" => "MiniMax",
        "llama" => "Llama",
        "gemini" => "Gemini",
        "gemma" => "Gemma",
        "claude" => "Claude",
        "grok" => "Grok",
        "kimi" => "Kimi",
        "mistral" => "Mistral",
        "codestral" => "Codestral",
        "nemotron" => "Nemotron",
        _ => return None,
    })
}

fn is_digits(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit())
}

/// Trailing tokens that are a date or snapshot tag rather than part of the
/// name: `20250514`, `0925`, `2603`, `2026-01-23` and `latest`.
fn split_version(tokens: &[&str]) -> (usize, Option<String>) {
    let n = tokens.len();
    if n >= 4 {
        let (y, m, d) = (tokens[n - 3], tokens[n - 2], tokens[n - 1]);
        if y.len() == 4
            && is_digits(y)
            && m.len() == 2
            && is_digits(m)
            && d.len() == 2
            && is_digits(d)
        {
            return (n - 3, Some(format!("{y}-{m}-{d}")));
        }
    }
    if n >= 2 {
        let last = tokens[n - 1];
        if (is_digits(last) && (last.len() == 4 || last.len() == 6 || last.len() == 8))
            || last == "latest"
        {
            return (n - 1, Some(last.to_string()));
        }
    }
    (n, None)
}

fn pretty_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    if let Some(fixed) = known_casing(&lower) {
        return fixed.to_string();
    }
    // o1, o3, o4-mini keep OpenAI's lowercase reasoning-model names.
    if lower.len() <= 3 && lower.starts_with('o') && is_digits(&lower[1..]) {
        return lower;
    }
    // Parameter and context sizes: 120b -> 120B, 256k -> 256K, 4o stays.
    if let Some(number) = lower.strip_suffix(['b', 'k', 'm'])
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return word.to_ascii_uppercase();
    }
    if word.chars().any(char::is_uppercase) {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn is_version_number(token: &str) -> bool {
    !token.is_empty()
        && token.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && token.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Prettify a wire id, e.g. `glm-4.6-air-0925` -> "GLM 4.6 Air" with version
/// "0925", `claude-opus-5-5` -> "Claude Opus 5.5", `gpt-6.2-sol` ->
/// "GPT-6.2 Sol". Router prefixes (`vendor/`, `accounts/x/models/`) are dropped.
#[must_use]
pub fn prettify_id(id: &str) -> PrettyName {
    let base = id.rsplit('/').next().unwrap_or(id);
    let tokens: Vec<&str> = base
        .split(['-', '_', ' '])
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return PrettyName {
            name: id.to_string(),
            version: None,
        };
    }
    let (name_len, version) = split_version(&tokens);
    let name_tokens = &tokens[..name_len];

    let mut words: Vec<String> = Vec::new();
    let mut index = 0;
    while index < name_tokens.len() {
        let token = name_tokens[index];
        // Anthropic-style split versions: `5-5` -> `5.5`.
        if is_digits(token) && token.len() <= 2 {
            let mut joined = token.to_string();
            while index + 1 < name_tokens.len()
                && is_digits(name_tokens[index + 1])
                && name_tokens[index + 1].len() <= 2
            {
                index += 1;
                joined.push('.');
                joined.push_str(name_tokens[index]);
            }
            match words.last_mut() {
                Some(previous) if previous == "GPT" => {
                    previous.push('-');
                    previous.push_str(&joined);
                },
                _ => words.push(joined),
            }
            index += 1;
            continue;
        }
        let word = pretty_word(token);
        match words.last_mut() {
            Some(previous) if previous == "GPT" && (is_version_number(token) || token == "4o") => {
                previous.push('-');
                previous.push_str(&word);
            },
            _ => words.push(word),
        }
        index += 1;
    }
    PrettyName {
        name: words.join(" "),
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettifies_real_ids() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("glm-4.6-air-0925", "GLM 4.6 Air", Some("0925")),
            (
                "claude-sonnet-4-20250514",
                "Claude Sonnet 4",
                Some("20250514"),
            ),
            ("claude-opus-5-5", "Claude Opus 5.5", None),
            (
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                Some("20251001"),
            ),
            ("gpt-6.2-sol", "GPT-6.2 Sol", None),
            ("gpt-4o-mini-tts", "GPT-4o Mini TTS", None),
            ("gpt-5", "GPT-5", None),
            ("openai/gpt-oss-120b", "GPT OSS 120B", None),
            ("qwen3-max-2026-01-23", "Qwen3 Max", Some("2026-01-23")),
            ("deepseek-v4-pro", "DeepSeek V4 Pro", None),
            ("o3-mini", "o3 Mini", None),
            ("kimi-k3", "Kimi K3", None),
            ("MiniMax-M3.1-Flash", "MiniMax M3.1 Flash", None),
            ("gemini-3.8-flash-tts", "Gemini 3.8 Flash TTS", None),
            ("accounts/fireworks/models/kimi-k3", "Kimi K3", None),
            ("llama-3.3-70b-versatile", "Llama 3.3 70B Versatile", None),
            ("llama-3-1-8b", "Llama 3.1 8B", None),
            ("mistral-small-2603", "Mistral Small", Some("2603")),
            ("mistral-large-latest", "Mistral Large", Some("latest")),
            ("k3-256k", "K3 256K", None),
            ("qwen_qwen3-coder", "Qwen Qwen3 Coder", None),
        ];
        for (id, name, version) in cases {
            let pretty = prettify_id(id);
            assert_eq!(pretty.name, *name, "{id}");
            assert_eq!(pretty.version.as_deref(), *version, "{id}");
        }
    }

    #[test]
    fn degenerate_ids() {
        assert_eq!(prettify_id("").name, "");
        assert_eq!(prettify_id("---").name, "---");
        let only_date = prettify_id("2603");
        assert_eq!(only_date.name, "2603");
        assert_eq!(only_date.version, None);
    }
}
