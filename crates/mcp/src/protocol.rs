//! MCP protocol types beyond tools: resources, prompts, roots, sampling,
//! elicitation, server notifications and the MCP Apps (`ui://`) metadata.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::{ContentBlock, McpToolDef};

// ── Resources ───────────────────────────────────────────────────────────────

/// A resource listed by `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// A parameterized resource listed by `resources/templates/list`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceTemplate {
    pub uri_template: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesListResult {
    #[serde(default)]
    pub resources: Vec<McpResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplatesListResult {
    #[serde(default)]
    pub resource_templates: Vec<McpResourceTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One item of a `resources/read` result: either `text` or base64 `blob`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ResourcesReadResult {
    #[serde(default)]
    pub contents: Vec<ResourceContents>,
}

// ── Prompts ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// A prompt template listed by `prompts/list`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpPrompt {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
}

impl McpPrompt {
    /// Names of required arguments missing from `arguments`.
    #[must_use]
    pub fn missing_arguments(&self, arguments: &HashMap<String, String>) -> Vec<&str> {
        self.arguments
            .iter()
            .filter(|arg| arg.required && !arguments.contains_key(&arg.name))
            .map(|arg| arg.name.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PromptsListResult {
    #[serde(default)]
    pub prompts: Vec<McpPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptMessage {
    pub role: Role,
    pub content: ContentBlock,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PromptsGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub messages: Vec<PromptMessage>,
}

/// A prompt offered as a slash-command candidate to the chat composer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptCommand {
    /// Command name without the leading slash: `<server>:<prompt>`.
    pub command: String,
    pub server: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub arguments: Vec<PromptArgument>,
}

impl PromptCommand {
    #[must_use]
    pub fn new(server: &str, prompt: &McpPrompt) -> Self {
        Self {
            command: format!("{server}:{}", prompt.name),
            server: server.to_string(),
            prompt: prompt.name.clone(),
            title: prompt.title.clone(),
            description: prompt.description.clone(),
            arguments: prompt.arguments.clone(),
        }
    }
}

// ── Roots ───────────────────────────────────────────────────────────────────

/// A filesystem root the client exposes to servers (`file://` URI).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Root {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Root {
    /// Build a root from an absolute path. Returns `None` for relative paths.
    #[must_use]
    pub fn from_path(path: &std::path::Path, name: Option<String>) -> Option<Self> {
        let uri = url::Url::from_directory_path(path).ok()?;
        Some(Self {
            uri: uri.to_string(),
            name,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ListRootsResult {
    pub roots: Vec<Root>,
}

// ── Sampling ────────────────────────────────────────────────────────────────

/// One content block or several (newer revisions allow arrays).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        match self {
            Self::One(item) => std::slice::from_ref(item).iter(),
            Self::Many(items) => items.iter(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SamplingMessage {
    pub role: Role,
    pub content: OneOrMany<ContentBlock>,
}

impl SamplingMessage {
    /// Concatenated text blocks of the message.
    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferences {
    #[serde(default)]
    pub hints: Vec<ModelHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intelligence_priority: Option<f64>,
}

/// Parameters of `sampling/createMessage`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateMessageParams {
    pub messages: Vec<SamplingMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_preferences: Option<ModelPreferences>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Result of `sampling/createMessage`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateMessageResult {
    pub role: Role,
    pub content: ContentBlock,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

impl CreateMessageResult {
    #[must_use]
    pub fn text(model: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: ContentBlock::Text { text: text.into() },
            model: model.into(),
            stop_reason: Some("endTurn".into()),
        }
    }
}

// ── Elicitation ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationMode {
    #[default]
    Form,
    Url,
}

/// Parameters of `elicitation/create`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ElicitRequestParams {
    #[serde(default)]
    pub mode: ElicitationMode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_schema: Option<crate::elicitation::ElicitationSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ElicitAction {
    Accept,
    Decline,
    Cancel,
}

/// Result of `elicitation/create`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ElicitResult {
    pub action: ElicitAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ElicitResult {
    #[must_use]
    pub fn accept(content: serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            action: ElicitAction::Accept,
            content: Some(content),
        }
    }

    #[must_use]
    pub fn decline() -> Self {
        Self {
            action: ElicitAction::Decline,
            content: None,
        }
    }

    #[must_use]
    pub fn cancel() -> Self {
        Self {
            action: ElicitAction::Cancel,
            content: None,
        }
    }
}

// ── Server notifications ────────────────────────────────────────────────────

/// A notification a server sent to the client.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerNotification {
    ToolsListChanged,
    ResourcesListChanged,
    ResourceUpdated {
        uri: String,
    },
    PromptsListChanged,
    /// `notifications/message` log entry.
    Log {
        level: String,
        logger: Option<String>,
        data: serde_json::Value,
    },
    Progress {
        progress_token: serde_json::Value,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    },
    Other {
        method: String,
        params: Option<serde_json::Value>,
    },
}

impl ServerNotification {
    #[must_use]
    pub fn parse(method: &str, params: Option<serde_json::Value>) -> Self {
        let field = |name: &str| params.as_ref().and_then(|p| p.get(name)).cloned();
        let text = |name: &str| field(name).and_then(|v| v.as_str().map(str::to_string));
        match method {
            "notifications/tools/list_changed" => Self::ToolsListChanged,
            "notifications/resources/list_changed" => Self::ResourcesListChanged,
            "notifications/prompts/list_changed" => Self::PromptsListChanged,
            "notifications/resources/updated" => match text("uri") {
                Some(uri) => Self::ResourceUpdated { uri },
                None => Self::Other {
                    method: method.to_string(),
                    params,
                },
            },
            "notifications/message" => Self::Log {
                level: text("level").unwrap_or_else(|| "info".into()),
                logger: text("logger"),
                data: field("data").unwrap_or(serde_json::Value::Null),
            },
            "notifications/progress" => Self::Progress {
                progress_token: field("progressToken").unwrap_or(serde_json::Value::Null),
                progress: field("progress")
                    .and_then(|v| v.as_f64())
                    .unwrap_or_default(),
                total: field("total").and_then(|v| v.as_f64()),
                message: text("message"),
            },
            _ => Self::Other {
                method: method.to_string(),
                params,
            },
        }
    }
}

// ── MCP Apps (`io.modelcontextprotocol/ui`) ─────────────────────────────────

/// Extension id advertised in `capabilities.extensions`.
pub const MCP_APPS_EXTENSION: &str = "io.modelcontextprotocol/ui";
/// MIME type of an MCP App HTML resource.
pub const MCP_APP_MIME_TYPE: &str = "text/html;profile=mcp-app";
/// URI scheme of MCP App resources.
pub const UI_RESOURCE_SCHEME: &str = "ui://";

/// Who may call a tool that declares a UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolVisibility {
    Model,
    App,
}

/// `_meta.ui` of a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolUiMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visibility: Vec<ToolVisibility>,
}

impl McpToolDef {
    /// MCP Apps metadata, from `_meta.ui` or the older flat
    /// `_meta["ui/resourceUri"]` key.
    #[must_use]
    pub fn ui(&self) -> Option<ToolUiMeta> {
        let meta = self.meta.as_ref()?;
        let mut ui = meta
            .get("ui")
            .and_then(|value| serde_json::from_value::<ToolUiMeta>(value.clone()).ok())
            .unwrap_or_default();
        if ui.resource_uri.is_none() {
            ui.resource_uri = meta
                .get("ui/resourceUri")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        let uri_ok = ui
            .resource_uri
            .as_deref()
            .is_none_or(|uri| uri.starts_with(UI_RESOURCE_SCHEME));
        if !uri_ok {
            ui.resource_uri = None;
        }
        (ui.resource_uri.is_some() || !ui.visibility.is_empty()).then_some(ui)
    }

    /// Whether the model may call this tool. Tools marked app-only by MCP
    /// Apps are reachable from their UI, never from the agent.
    #[must_use]
    pub fn is_model_visible(&self) -> bool {
        self.ui().is_none_or(|ui| {
            ui.visibility.is_empty() || ui.visibility.contains(&ToolVisibility::Model)
        })
    }
}

/// A `ui://` resource read for an MCP App host.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiResource {
    pub server: String,
    pub uri: String,
    pub mime_type: String,
    pub html: String,
    /// `_meta.ui` of the resource (CSP, permissions, domain), passed through
    /// to the future iframe host unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_meta: Option<serde_json::Value>,
}

impl UiResource {
    /// Validate a `resources/read` result as an MCP App document.
    pub fn from_read(server: &str, uri: &str, read: ResourcesReadResult) -> crate::Result<Self> {
        if !uri.starts_with(UI_RESOURCE_SCHEME) {
            return Err(crate::Error::message(format!(
                "'{uri}' is not a ui:// resource"
            )));
        }
        let contents = read
            .contents
            .into_iter()
            .find(|item| item.uri == uri)
            .ok_or_else(|| {
                crate::Error::message(format!("server returned no content for '{uri}'"))
            })?;
        let mime_type = contents.mime_type.clone().unwrap_or_default();
        if !mime_type.starts_with("text/html") {
            return Err(crate::Error::message(format!(
                "ui resource '{uri}' has MIME type '{mime_type}', expected {MCP_APP_MIME_TYPE}"
            )));
        }
        let html = match (contents.text, contents.blob) {
            (Some(text), _) => text,
            (None, Some(blob)) => decode_base64_utf8(&blob)?,
            (None, None) => {
                return Err(crate::Error::message(format!(
                    "ui resource '{uri}' has no text"
                )));
            },
        };
        Ok(Self {
            server: server.to_string(),
            uri: uri.to_string(),
            mime_type,
            html,
            ui_meta: contents.meta.and_then(|meta| meta.get("ui").cloned()),
        })
    }
}

fn decode_base64_utf8(blob: &str) -> crate::Result<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [u8::MAX; 256];
    for (index, byte) in (0u8..).zip(ALPHABET.iter()) {
        lookup[usize::from(*byte)] = index;
    }
    let mut out = Vec::with_capacity(blob.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in blob
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
    {
        let value = lookup[usize::from(byte)];
        if value == u8::MAX {
            return Err(crate::Error::message(
                "ui resource blob is not valid base64",
            ));
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).map_err(|_| crate::Error::message("ui resource blob is not UTF-8"))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    #[test]
    fn resource_list_parses_cursor_and_meta() {
        let parsed: ResourcesListResult = serde_json::from_value(json!({
            "resources": [{"uri": "file:///a", "name": "a", "mimeType": "text/plain", "size": 3, "_meta": {"k": 1}}],
            "nextCursor": "c2"
        }))
        .unwrap();
        assert_eq!(parsed.next_cursor.as_deref(), Some("c2"));
        assert_eq!(parsed.resources[0].mime_type.as_deref(), Some("text/plain"));
        assert_eq!(parsed.resources[0].meta, Some(json!({"k": 1})));
    }

    #[test]
    fn prompt_missing_arguments_lists_required_only() {
        let prompt: McpPrompt = serde_json::from_value(json!({
            "name": "review",
            "arguments": [{"name": "code", "required": true}, {"name": "style"}]
        }))
        .unwrap();
        assert_eq!(prompt.missing_arguments(&HashMap::new()), vec!["code"]);
        let args = HashMap::from([("code".to_string(), "x".to_string())]);
        assert!(prompt.missing_arguments(&args).is_empty());
        assert_eq!(PromptCommand::new("git", &prompt).command, "git:review");
    }

    #[test]
    fn sampling_message_accepts_single_or_array_content() {
        let one: SamplingMessage = serde_json::from_value(
            json!({"role": "user", "content": {"type": "text", "text": "hi"}}),
        )
        .unwrap();
        let many: SamplingMessage = serde_json::from_value(json!({
            "role": "user",
            "content": [{"type": "text", "text": "a"}, {"type": "image", "data": "x", "mimeType": "image/png"}, {"type": "text", "text": "b"}]
        }))
        .unwrap();
        assert_eq!(one.text(), "hi");
        assert_eq!(many.text(), "a\nb");
    }

    #[test]
    fn create_message_params_round_trip() {
        let params: CreateMessageParams = serde_json::from_value(json!({
            "messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}],
            "systemPrompt": "be brief",
            "maxTokens": 100,
            "modelPreferences": {"hints": [{"name": "claude"}], "speedPriority": 0.5}
        }))
        .unwrap();
        assert_eq!(params.max_tokens, 100);
        let value = serde_json::to_value(CreateMessageResult::text("m", "ok")).unwrap();
        assert_eq!(value["content"]["text"], "ok");
        assert_eq!(value["stopReason"], "endTurn");
    }

    #[test]
    fn root_from_path_builds_file_uri() {
        let root = Root::from_path(std::path::Path::new("/tmp/work"), Some("work".into())).unwrap();
        assert_eq!(root.uri, "file:///tmp/work/");
        assert!(Root::from_path(std::path::Path::new("relative"), None).is_none());
    }

    #[test]
    fn notifications_parse_known_methods() {
        assert_eq!(
            ServerNotification::parse(
                "notifications/resources/updated",
                Some(json!({"uri": "x://1"}))
            ),
            ServerNotification::ResourceUpdated {
                uri: "x://1".into()
            }
        );
        assert_eq!(
            ServerNotification::parse("notifications/tools/list_changed", None),
            ServerNotification::ToolsListChanged
        );
        assert!(matches!(
            ServerNotification::parse("notifications/resources/updated", None),
            ServerNotification::Other { .. }
        ));
        assert!(matches!(
            ServerNotification::parse(
                "notifications/progress",
                Some(json!({"progressToken": 1, "progress": 0.5, "total": 1.0}))
            ),
            ServerNotification::Progress { total: Some(_), .. }
        ));
        assert!(matches!(
            ServerNotification::parse(
                "notifications/message",
                Some(json!({"level": "error", "data": "boom"}))
            ),
            ServerNotification::Log { .. }
        ));
    }

    #[test]
    fn tool_ui_meta_supports_nested_and_flat_keys() {
        let nested = McpToolDef {
            name: "chart".into(),
            meta: Some(json!({"ui": {"resourceUri": "ui://charts/main", "visibility": ["app"]}})),
            ..Default::default()
        };
        let ui = nested.ui().unwrap();
        assert_eq!(ui.resource_uri.as_deref(), Some("ui://charts/main"));
        assert!(!nested.is_model_visible());

        let flat = McpToolDef {
            name: "chart".into(),
            meta: Some(json!({"ui/resourceUri": "ui://charts/flat"})),
            ..Default::default()
        };
        assert_eq!(
            flat.ui().unwrap().resource_uri.as_deref(),
            Some("ui://charts/flat")
        );
        assert!(flat.is_model_visible());

        let bad_scheme = McpToolDef {
            name: "x".into(),
            meta: Some(json!({"ui": {"resourceUri": "https://evil"}})),
            ..Default::default()
        };
        assert!(bad_scheme.ui().is_none());
        assert!(McpToolDef::default().is_model_visible());
    }

    #[test]
    fn ui_resource_validates_scheme_mime_and_decodes_blob() {
        let read = |mime: &str, text: Option<&str>, blob: Option<&str>| ResourcesReadResult {
            contents: vec![ResourceContents {
                uri: "ui://app".into(),
                mime_type: Some(mime.into()),
                text: text.map(str::to_string),
                blob: blob.map(str::to_string),
                meta: Some(json!({"ui": {"csp": {"connectDomains": []}}})),
            }],
        };
        let ok = UiResource::from_read("s", "ui://app", read(MCP_APP_MIME_TYPE, Some("<p>"), None))
            .unwrap();
        assert_eq!(ok.html, "<p>");
        assert!(ok.ui_meta.is_some());
        // "<b>hi</b>" in base64.
        let blob = UiResource::from_read(
            "s",
            "ui://app",
            read(MCP_APP_MIME_TYPE, None, Some("PGI+aGk8L2I+")),
        )
        .unwrap();
        assert_eq!(blob.html, "<b>hi</b>");
        assert!(
            UiResource::from_read("s", "ui://app", read("application/json", Some("{}"), None))
                .is_err()
        );
        assert!(
            UiResource::from_read("s", "https://x", read(MCP_APP_MIME_TYPE, Some(""), None))
                .is_err()
        );
        assert!(
            UiResource::from_read("s", "ui://other", read(MCP_APP_MIME_TYPE, Some(""), None))
                .is_err()
        );
        assert!(
            UiResource::from_read("s", "ui://app", read(MCP_APP_MIME_TYPE, None, Some("!!")))
                .is_err()
        );
        assert!(
            UiResource::from_read("s", "ui://app", read(MCP_APP_MIME_TYPE, None, None)).is_err()
        );
    }

    #[test]
    fn elicit_result_constructors_serialize_actions() {
        assert_eq!(
            serde_json::to_value(ElicitResult::decline()).unwrap(),
            json!({"action": "decline"})
        );
        assert_eq!(
            serde_json::to_value(ElicitResult::cancel()).unwrap(),
            json!({"action": "cancel"})
        );
        let mut content = serde_json::Map::new();
        content.insert("name".into(), json!("Ada"));
        assert_eq!(
            serde_json::to_value(ElicitResult::accept(content)).unwrap(),
            json!({"action": "accept", "content": {"name": "Ada"}})
        );
    }
}
