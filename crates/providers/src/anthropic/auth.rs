//! Anthropic credentials: API keys and `claude setup-token` OAuth tokens.
//!
//! Subscription setup-tokens are only accepted by Anthropic on requests that
//! identify as Claude Code, so the OAuth request shape mirrors OpenClaw's
//! (`packages/ai/src/providers/anthropic-model-contract.ts` and
//! `packages/ai/src/transports/anthropic-payload-policy.ts` in
//! openclaw/openclaw): Bearer authorization, the CLI identity headers, and
//! two identity blocks prepended to the system prompt.

use {
    reqwest::RequestBuilder,
    secrecy::{ExposeSecret, Secret},
    serde_json::Value,
};

/// Version reported as `claude-cli/<version>` and in the billing identity
/// block. Tracks OpenClaw's `ANTHROPIC_CLAUDE_CODE_VERSION` floor.
const CLAUDE_CODE_VERSION: &str = "2.1.280";

/// Beta flags Anthropic expects on subscription-token requests.
const OAUTH_BETA_FLAGS: &str = "claude-code-20250219,oauth-2025-04-20";

/// `claude setup-token` tokens are OAuth credentials, not API keys.
pub fn is_setup_token(key: &str) -> bool {
    key.contains("sk-ant-oat")
}

/// Credentials for an Anthropic provider: one API key, or one subscription
/// setup-token. Multiple Claude accounts live as separate provider instances
/// (the builtin and `custom-*` entries), never as a rotation inside one.
pub enum AnthropicAuth {
    ApiKey(Secret<String>),
    SetupToken(Secret<String>),
}

impl Clone for AnthropicAuth {
    fn clone(&self) -> Self {
        match self {
            Self::ApiKey(key) => Self::ApiKey(key.clone()),
            Self::SetupToken(token) => Self::SetupToken(token.clone()),
        }
    }
}

impl std::fmt::Debug for AnthropicAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AnthropicAuth([REDACTED])")
    }
}

impl AnthropicAuth {
    /// Build from a single configured credential, detecting setup-tokens by
    /// their `sk-ant-oat` marker (as OpenClaw's `isAnthropicOAuthApiKey`).
    pub fn from_single(key: Secret<String>) -> Self {
        if is_setup_token(key.expose_secret()) {
            Self::SetupToken(key)
        } else {
            Self::ApiKey(key)
        }
    }

    /// Build from an explicit `claude setup-token` subscription credential.
    pub fn setup_token(token: Secret<String>) -> Self {
        Self::SetupToken(token)
    }

    /// `true` when requests must carry the Claude Code OAuth identity.
    fn is_oauth(&self) -> bool {
        matches!(self, Self::SetupToken(_))
    }

    /// Apply authentication to a single non-rotating request (catalog GETs).
    pub fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self {
            Self::ApiKey(key) => request.header("x-api-key", key.expose_secret()),
            Self::SetupToken(token) => oauth_headers(request, token),
        }
    }

    /// Wrap the system prompt with the Claude Code identity blocks when using
    /// a setup-token. API-key requests are returned unchanged.
    ///
    /// `system_value` is the value produced by `to_anthropic_messages`: a
    /// plain string without caching, or a block array whose last block
    /// carries `cache_control` with caching. The identity blocks are static,
    /// so they stay ahead of the cached application prompt.
    pub fn wrap_system(&self, system_value: Option<Value>, caching: bool) -> Option<Value> {
        if !self.is_oauth() {
            return system_value;
        }
        let rest = match system_value {
            Some(Value::String(text)) => vec![serde_json::json!({"type": "text", "text": text})],
            Some(Value::Array(blocks)) => blocks,
            _ => Vec::new(),
        };
        let identity_cache_control = if caching
            && !rest
                .iter()
                .any(|block| block.get("cache_control").is_some())
        {
            serde_json::json!({ "type": "ephemeral" })
        } else {
            Value::Null
        };
        let mut identity = serde_json::json!({
            "type": "text",
            "text": "You are Claude Code, Anthropic's official CLI for Claude.",
        });
        if !identity_cache_control.is_null() {
            identity["cache_control"] = identity_cache_control;
        }
        let mut blocks = vec![
            serde_json::json!({
                "type": "text",
                "text": format!(
                    "x-anthropic-billing-header: cc_version={CLAUDE_CODE_VERSION}; cc_entrypoint=sdk-cli;"
                ),
            }),
            identity,
        ];
        blocks.extend(rest);
        Some(Value::Array(blocks))
    }

    /// Send a request with the configured credential. The response (success
    /// or failure) is returned for the caller to handle.
    pub async fn send(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> anyhow::Result<reqwest::Response> {
        Ok(self.apply(build()).send().await?)
    }

    /// POST a JSON body with the Anthropic API headers (see [`Self::send`]).
    pub async fn send_json(
        &self,
        client: &reqwest::Client,
        url: &str,
        body: &Value,
    ) -> anyhow::Result<reqwest::Response> {
        self.send(|| {
            client
                .post(url)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(body)
        })
        .await
    }
}

fn oauth_headers(request: RequestBuilder, token: &Secret<String>) -> RequestBuilder {
    request
        .header("authorization", format!("Bearer {}", token.expose_secret()))
        .header("anthropic-beta", OAUTH_BETA_FLAGS)
        .header("x-app", "cli")
        .header("user-agent", format!("claude-cli/{CLAUDE_CODE_VERSION}"))
}
