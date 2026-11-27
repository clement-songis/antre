/// Design canvas configuration for `moltis.toml`.
///
/// ```toml
/// [canvas]
/// enabled = true
/// # bind = "127.0.0.1"                      # default: server.bind
/// # port = 18790                            # default: a free port picked at startup
/// # public_url = "https://designs.example.com"
/// # token_ttl_secs = 3600
/// # script_sources = ["https://cdn.example.com"]
/// ```
use serde::{Deserialize, Serialize};

/// Settings for design artifacts and their sandboxed render origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasConfig {
    /// Enable design tools, the `canvas.*` API and the render origin.
    pub enabled: bool,
    /// Address the render origin listens on. `None` uses `server.bind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    /// Port of the render origin. `None` picks a free port at startup, which
    /// the web UI discovers through `canvas.config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Public origin of the render server when it sits behind a reverse
    /// proxy or its own hostname (e.g. `https://designs.example.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Lifetime of signed preview links, in seconds.
    pub token_ttl_secs: u64,
    /// Extra `https://` origins allowed in `script-src`, on top of the
    /// vendored scripts served at `/_vendor/`. Empty keeps designs offline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub script_sources: Vec<String>,
    /// Allow `eval`/`new Function` in designs (needed by in-browser JSX
    /// transpilers). Off by default.
    pub allow_eval: bool,
    /// Largest single file the render origin serves or exports, in bytes.
    pub max_file_bytes: u64,
    /// Run PNG/PDF renders in a browser container instead of on the host.
    pub render_sandbox: bool,
    /// Timeout of one PNG/PDF render, in seconds.
    pub render_timeout_secs: u64,
    /// Seed the bundled starter design systems into `data_dir/skills/`.
    pub seed_design_systems: bool,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: None,
            port: None,
            public_url: None,
            token_ttl_secs: 3600,
            script_sources: Vec::new(),
            allow_eval: false,
            max_file_bytes: 10 * 1024 * 1024,
            render_sandbox: false,
            render_timeout_secs: 30,
            seed_design_systems: true,
        }
    }
}

impl CanvasConfig {
    /// Entries of `script_sources` that are not a bare `https://host[:port]`
    /// origin. They are dropped at runtime so they cannot inject CSP syntax.
    #[must_use]
    pub fn invalid_script_sources(&self) -> Vec<&str> {
        self.script_sources
            .iter()
            .map(String::as_str)
            .filter(|source| !is_https_origin(source))
            .collect()
    }

    /// Whether `public_url` is set to something other than an http(s) origin.
    #[must_use]
    pub fn public_url_is_invalid(&self) -> bool {
        self.public_url
            .as_deref()
            .is_some_and(|url| !is_http_origin(url))
    }
}

/// `https://host[:port]` with no path, query, wildcard or CSP keyword.
#[must_use]
pub fn is_https_origin(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .is_some_and(is_valid_authority)
}

/// `http(s)://host[:port]` with an optional trailing slash.
#[must_use]
pub fn is_http_origin(value: &str) -> bool {
    let value = value.strip_suffix('/').unwrap_or(value);
    value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .is_some_and(is_valid_authority)
}

fn is_valid_authority(authority: &str) -> bool {
    !authority.is_empty()
        && authority.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_designs_offline() {
        let config = CanvasConfig::default();
        assert!(config.enabled);
        assert!(config.script_sources.is_empty());
        assert!(!config.allow_eval);
        assert_eq!(config.port, None);
    }

    #[test]
    fn script_sources_must_be_bare_https_origins() {
        let config = CanvasConfig {
            script_sources: vec![
                "https://cdn.example.com".into(),
                "https://cdn.example.com:8443".into(),
                "http://cdn.example.com".into(),
                "https://*.example.com".into(),
                "https://cdn.example.com/lib.js".into(),
                "https://x; script-src *".into(),
                "'unsafe-eval'".into(),
            ],
            ..CanvasConfig::default()
        };
        assert_eq!(config.invalid_script_sources(), vec![
            "http://cdn.example.com",
            "https://*.example.com",
            "https://cdn.example.com/lib.js",
            "https://x; script-src *",
            "'unsafe-eval'",
        ]);
    }

    #[test]
    fn public_url_accepts_http_and_https_origins_only() {
        let with = |url: &str| CanvasConfig {
            public_url: Some(url.into()),
            ..CanvasConfig::default()
        };
        assert!(!with("https://designs.example.com").public_url_is_invalid());
        assert!(!with("http://127.0.0.1:18790/").public_url_is_invalid());
        assert!(with("designs.example.com").public_url_is_invalid());
        assert!(with("https://designs.example.com/path").public_url_is_invalid());
    }

    #[test]
    fn deserializes_partial_table() {
        let config: CanvasConfig = toml::from_str("port = 18790\nallow_eval = true").unwrap();
        assert_eq!(config.port, Some(18790));
        assert!(config.allow_eval);
        assert_eq!(config.token_ttl_secs, 3600);
    }
}
