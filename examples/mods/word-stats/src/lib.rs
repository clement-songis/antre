//! Demo mod for the antre mod SDK (`wit/mod/antre-mod.wit`).
//!
//! Build and package it with `just mod-demo`, then install the folder printed
//! by the recipe with `mods.install` or `POST /api/mods/install`.

// Pure functions are used by the guest impl; allow dead_code on host builds.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use serde_json::{Value, json};

#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "../../../wit/mod",
    world: "plugin",
});

const TOTAL_KEY: &str = "total_calls";
const SESSIONS_KEY: &str = "sessions_seen";

/// Word, line and character counts for `text`.
fn stats(text: &str) -> Value {
    json!({
        "words": text.split_whitespace().count(),
        "lines": if text.is_empty() { 0 } else { text.lines().count() },
        "chars": text.chars().count(),
    })
}

fn parse_text(params_json: &str) -> Result<String, String> {
    let params: Value = serde_json::from_str(params_json)
        .map_err(|error| format!("invalid parameters: {error}"))?;
    params
        .get("text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "missing string parameter `text`".to_string())
}

fn bump(current: Option<String>) -> u64 {
    current
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_add(1)
}

#[cfg(target_arch = "wasm32")]
struct WordStats;

#[cfg(target_arch = "wasm32")]
impl exports::antre::mods::tools::Guest for WordStats {
    fn call(name: String, params_json: String) -> Result<String, String> {
        use antre::mods::{host, types::LogLevel};

        if name != "word_stats" {
            return Err(format!("unknown tool `{name}`"));
        }
        let text = parse_text(&params_json)?;
        let mut result = stats(&text);
        // Storage is a granted capability; without it the mod still works.
        match host::kv_get(TOTAL_KEY) {
            Ok(current) => {
                let total = bump(current);
                if host::kv_set(TOTAL_KEY, &total.to_string()).is_ok() {
                    result["total_calls"] = json!(total);
                }
            },
            Err(error) => host::log(LogLevel::Warn, &format!("storage unavailable: {error}")),
        }
        serde_json::to_string(&result).map_err(|error| error.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl exports::antre::mods::hooks::Guest for WordStats {
    fn handle(event: String, _payload_json: String) -> Result<Option<String>, String> {
        use antre::mods::host;

        if event == "SessionStart"
            && let Ok(current) = host::kv_get(SESSIONS_KEY)
        {
            let _ = host::kv_set(SESSIONS_KEY, &bump(current).to_string());
        }
        Ok(None)
    }
}

#[cfg(target_arch = "wasm32")]
export!(WordStats);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_lines_chars() {
        let value = stats("hello world\nsecond line");
        assert_eq!(value["words"], 4);
        assert_eq!(value["lines"], 2);
        assert_eq!(value["chars"], 23);
        assert_eq!(stats("")["lines"], 0);
    }

    #[test]
    fn parses_parameters() {
        assert_eq!(parse_text(r#"{"text":"hi"}"#), Ok("hi".to_string()));
        assert!(parse_text("{}").is_err());
        assert!(parse_text("nope").is_err());
    }

    #[test]
    fn bump_counts_from_zero() {
        assert_eq!(bump(None), 1);
        assert_eq!(bump(Some("41".into())), 42);
        assert_eq!(bump(Some("garbage".into())), 1);
    }
}
