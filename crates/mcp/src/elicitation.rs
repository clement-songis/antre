//! Elicitation: typed form schemas, answer validation, and the pending
//! request store the gateway resolves from the UI.

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use {
    serde::{Deserialize, Serialize},
    serde_json::{Map, Value},
    tokio::sync::{Mutex, oneshot},
    tracing::{debug, warn},
};

use crate::protocol::{ElicitAction, ElicitResult};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationFieldType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
}

/// `{ "const": "value", "title": "Label" }` option of a titled enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumOption {
    #[serde(rename = "const")]
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Items of a multi-select (`type: "array"`) field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationItems {
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_of: Vec<EnumOption>,
}

impl ElicitationItems {
    fn allowed(&self) -> Vec<&str> {
        self.enum_values
            .iter()
            .map(String::as_str)
            .chain(self.any_of.iter().map(|option| option.value.as_str()))
            .collect()
    }
}

/// One primitive field of an elicitation form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationField {
    #[serde(rename = "type")]
    pub kind: ElicitationFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<EnumOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ElicitationItems>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl ElicitationField {
    fn allowed_values(&self) -> Vec<&str> {
        self.enum_values
            .iter()
            .map(String::as_str)
            .chain(self.one_of.iter().map(|option| option.value.as_str()))
            .collect()
    }

    fn validate(&self, name: &str, value: &Value) -> Result<(), String> {
        match self.kind {
            ElicitationFieldType::String => {
                let text = value
                    .as_str()
                    .ok_or_else(|| format!("'{name}' must be a string"))?;
                let length = text.chars().count();
                if self.min_length.is_some_and(|min| length < min) {
                    return Err(format!("'{name}' is shorter than {:?}", self.min_length));
                }
                if self.max_length.is_some_and(|max| length > max) {
                    return Err(format!("'{name}' is longer than {:?}", self.max_length));
                }
                let allowed = self.allowed_values();
                if !allowed.is_empty() && !allowed.contains(&text) {
                    return Err(format!("'{name}' must be one of {allowed:?}"));
                }
                self.validate_format(name, text)
            },
            ElicitationFieldType::Number | ElicitationFieldType::Integer => {
                let number = value
                    .as_f64()
                    .ok_or_else(|| format!("'{name}' must be a number"))?;
                if self.kind == ElicitationFieldType::Integer && !(value.is_i64() || value.is_u64())
                {
                    return Err(format!("'{name}' must be an integer"));
                }
                if self.minimum.is_some_and(|min| number < min) {
                    return Err(format!("'{name}' is below {:?}", self.minimum));
                }
                if self.maximum.is_some_and(|max| number > max) {
                    return Err(format!("'{name}' is above {:?}", self.maximum));
                }
                Ok(())
            },
            ElicitationFieldType::Boolean => value
                .is_boolean()
                .then_some(())
                .ok_or_else(|| format!("'{name}' must be true or false")),
            ElicitationFieldType::Array => {
                let items = value
                    .as_array()
                    .ok_or_else(|| format!("'{name}' must be a list"))?;
                if self.min_items.is_some_and(|min| items.len() < min) {
                    return Err(format!(
                        "'{name}' needs at least {:?} items",
                        self.min_items
                    ));
                }
                if self.max_items.is_some_and(|max| items.len() > max) {
                    return Err(format!(
                        "'{name}' allows at most {:?} items",
                        self.max_items
                    ));
                }
                let allowed = self
                    .items
                    .as_ref()
                    .map(ElicitationItems::allowed)
                    .unwrap_or_default();
                for item in items {
                    let text = item
                        .as_str()
                        .ok_or_else(|| format!("'{name}' items must be strings"))?;
                    if !allowed.is_empty() && !allowed.contains(&text) {
                        return Err(format!("'{name}' item '{text}' is not one of {allowed:?}"));
                    }
                }
                Ok(())
            },
        }
    }

    fn validate_format(&self, name: &str, text: &str) -> Result<(), String> {
        let valid = match self.format.as_deref() {
            Some("email") => text
                .split_once('@')
                .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.')),
            Some("uri") => url::Url::parse(text).is_ok(),
            Some("date") => is_iso_date(text),
            Some("date-time") => {
                text.len() > 10 && is_iso_date(&text[..10]) && text.as_bytes()[10] == b'T'
            },
            _ => true,
        };
        valid.then_some(()).ok_or_else(|| {
            format!(
                "'{name}' is not a valid {}",
                self.format.as_deref().unwrap_or("value")
            )
        })
    }

    fn describe(&self, name: &str, required: bool) -> String {
        let label = self.title.as_deref().unwrap_or(name);
        let required = if required {
            ", required"
        } else {
            ""
        };
        let mut line = format!("- {label} ({name}, {}{required})", self.type_label());
        let allowed = self.allowed_values();
        if !allowed.is_empty() {
            line.push_str(&format!(": one of {}", allowed.join(", ")));
        }
        if let Some(description) = &self.description {
            line.push_str(&format!(" — {description}"));
        }
        line
    }

    fn type_label(&self) -> &'static str {
        match self.kind {
            ElicitationFieldType::String => "text",
            ElicitationFieldType::Number => "number",
            ElicitationFieldType::Integer => "whole number",
            ElicitationFieldType::Boolean => "yes/no",
            ElicitationFieldType::Array => "list",
        }
    }
}

fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// The restricted JSON Schema of an elicitation form: a flat object of
/// primitive fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ElicitationSchema {
    #[serde(rename = "type", default = "object_type")]
    pub kind: String,
    #[serde(default)]
    pub properties: BTreeMap<String, ElicitationField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

fn object_type() -> String {
    "object".into()
}

impl ElicitationSchema {
    /// Check an accepted answer against the schema.
    pub fn validate(&self, content: &Map<String, Value>) -> Result<(), String> {
        if let Some(unknown) = content
            .keys()
            .find(|key| !self.properties.contains_key(*key))
        {
            return Err(format!("unknown field '{unknown}'"));
        }
        if let Some(missing) = self
            .required
            .iter()
            .find(|name| !content.contains_key(*name))
        {
            return Err(format!("missing required field '{missing}'"));
        }
        content.iter().try_for_each(|(name, value)| {
            self.properties
                .get(name)
                .map_or(Ok(()), |field| field.validate(name, value))
        })
    }

    /// Plain-text description of the form, for channels without a form UI.
    #[must_use]
    pub fn describe(&self) -> String {
        self.properties
            .iter()
            .map(|(name, field)| field.describe(name, self.required.contains(name)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// An elicitation waiting for the user, as shown to clients.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationView {
    pub id: String,
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    pub message: String,
    pub schema: ElicitationSchema,
    pub timeout_secs: u64,
}

impl ElicitationView {
    /// Message sent to a chat channel bound to the session.
    #[must_use]
    pub fn channel_text(&self) -> String {
        let fields = self.schema.describe();
        let fields = if fields.is_empty() {
            String::new()
        } else {
            format!("\n{fields}")
        };
        format!(
            "MCP server '{}' is asking: {}{fields}\nAnswer it in the web UI within {} min; otherwise it is cancelled.",
            self.server,
            self.message,
            self.timeout_secs.div_ceil(60),
        )
    }
}

struct PendingElicitation {
    view: ElicitationView,
    tx: oneshot::Sender<ElicitResult>,
}

/// Pending elicitations keyed by request id.
pub struct PendingElicitations {
    pending: Mutex<HashMap<String, PendingElicitation>>,
    timeout: Duration,
}

impl PendingElicitations {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            timeout: timeout.max(Duration::from_secs(1)),
        }
    }

    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Register a request; the receiver yields the user's answer.
    pub async fn create(
        &self,
        server: &str,
        session_key: Option<&str>,
        message: &str,
        schema: ElicitationSchema,
    ) -> (ElicitationView, oneshot::Receiver<ElicitResult>) {
        let view = ElicitationView {
            id: uuid::Uuid::new_v4().to_string(),
            server: server.to_string(),
            session_key: session_key.map(str::to_string),
            message: message.to_string(),
            schema,
            timeout_secs: self.timeout.as_secs(),
        };
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(view.id.clone(), PendingElicitation {
                view: view.clone(),
                tx,
            });
        debug!(id = %view.id, server, "elicitation pending");
        (view, rx)
    }

    /// Resolve a request. An accepted answer must match the schema; on a
    /// validation error the request stays pending so the user can fix it.
    pub async fn resolve(&self, id: &str, result: ElicitResult) -> Result<ElicitationView, String> {
        let mut pending = self.pending.lock().await;
        let entry = pending
            .get(id)
            .ok_or_else(|| format!("no pending elicitation '{id}'"))?;
        let result = match result.action {
            ElicitAction::Accept => {
                let content = result.content.unwrap_or_default();
                entry.view.schema.validate(&content)?;
                ElicitResult::accept(content)
            },
            ElicitAction::Decline => ElicitResult::decline(),
            ElicitAction::Cancel => ElicitResult::cancel(),
        };
        let entry = pending
            .remove(id)
            .ok_or_else(|| format!("no pending elicitation '{id}'"))?;
        if entry.tx.send(result).is_err() {
            warn!(id, "elicitation answered after the server stopped waiting");
        }
        Ok(entry.view)
    }

    /// Wait for the answer; a timeout or dropped request counts as cancel.
    pub async fn wait(&self, id: &str, rx: oneshot::Receiver<ElicitResult>) -> ElicitResult {
        let result = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => ElicitResult::cancel(),
            Err(_) => {
                debug!(id, "elicitation timed out");
                ElicitResult::cancel()
            },
        };
        self.pending.lock().await.remove(id);
        result
    }

    /// Pending requests, optionally limited to one session, oldest id order.
    pub async fn list(&self, session_key: Option<&str>) -> Vec<ElicitationView> {
        let mut views: Vec<_> = self
            .pending
            .lock()
            .await
            .values()
            .filter(|entry| {
                session_key.is_none_or(|key| entry.view.session_key.as_deref() == Some(key))
            })
            .map(|entry| entry.view.clone())
            .collect();
        views.sort_by(|left, right| left.id.cmp(&right.id));
        views
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn schema() -> ElicitationSchema {
        serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "title": "Name", "minLength": 2, "maxLength": 10},
                "email": {"type": "string", "format": "email"},
                "site": {"type": "string", "format": "uri"},
                "born": {"type": "string", "format": "date"},
                "at": {"type": "string", "format": "date-time"},
                "age": {"type": "integer", "minimum": 0, "maximum": 150},
                "score": {"type": "number", "maximum": 1.0},
                "ok": {"type": "boolean"},
                "color": {"type": "string", "enum": ["red", "blue"], "enumNames": ["Red", "Blue"]},
                "size": {"type": "string", "oneOf": [{"const": "s", "title": "Small"}]},
                "tags": {"type": "array", "items": {"enum": ["a", "b"]}, "minItems": 1, "maxItems": 2}
            },
            "required": ["name"]
        }))
        .unwrap()
    }

    fn content(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn valid_answers_pass() {
        let answer = content(json!({
            "name": "Ada", "email": "a@b.io", "site": "https://x.io", "born": "1815-12-10",
            "at": "2026-01-01T10:00:00Z", "age": 36, "score": 0.5, "ok": true, "color": "red",
            "size": "s", "tags": ["a"]
        }));
        schema().validate(&answer).unwrap();
    }

    #[test]
    fn invalid_answers_are_rejected() {
        let cases = [
            json!({}),
            json!({"name": "A"}),
            json!({"name": "Abcdefghijk"}),
            json!({"name": 3}),
            json!({"name": "Ada", "extra": 1}),
            json!({"name": "Ada", "email": "nope"}),
            json!({"name": "Ada", "site": "not a url"}),
            json!({"name": "Ada", "born": "10/12/1815"}),
            json!({"name": "Ada", "at": "2026-01-01 10:00"}),
            json!({"name": "Ada", "age": 1.5}),
            json!({"name": "Ada", "age": -1}),
            json!({"name": "Ada", "age": 200}),
            json!({"name": "Ada", "age": "3"}),
            json!({"name": "Ada", "score": 2}),
            json!({"name": "Ada", "ok": "yes"}),
            json!({"name": "Ada", "color": "green"}),
            json!({"name": "Ada", "size": "xl"}),
            json!({"name": "Ada", "tags": []}),
            json!({"name": "Ada", "tags": ["a", "b", "a"]}),
            json!({"name": "Ada", "tags": ["c"]}),
            json!({"name": "Ada", "tags": [1]}),
            json!({"name": "Ada", "tags": "a"}),
        ];
        for case in cases {
            assert!(schema().validate(&content(case.clone())).is_err(), "{case}");
        }
    }

    #[test]
    fn describe_lists_fields_for_channels() {
        let text = schema().describe();
        assert!(text.contains("- Name (name, text, required)"), "{text}");
        assert!(text.contains("one of red, blue"));
        let view = ElicitationView {
            id: "1".into(),
            server: "jira".into(),
            session_key: None,
            message: "Which project?".into(),
            schema: schema(),
            timeout_secs: 300,
        };
        let message = view.channel_text();
        assert!(message.contains("MCP server 'jira' is asking: Which project?"));
        assert!(message.contains("within 5 min"));
    }

    #[tokio::test]
    async fn pending_store_resolves_and_validates() {
        let store = PendingElicitations::new(Duration::from_secs(5));
        let (view, rx) = store.create("srv", Some("s1"), "who?", schema()).await;
        assert_eq!(store.list(Some("s1")).await.len(), 1);
        assert!(store.list(Some("other")).await.is_empty());

        let bad = store
            .resolve(
                &view.id,
                ElicitResult::accept(content(json!({"name": "A"}))),
            )
            .await;
        assert!(bad.is_err());
        assert_eq!(
            store.list(None).await.len(),
            1,
            "invalid answer keeps it pending"
        );

        store
            .resolve(
                &view.id,
                ElicitResult::accept(content(json!({"name": "Ada"}))),
            )
            .await
            .unwrap();
        let result = store.wait(&view.id, rx).await;
        assert_eq!(result.action, ElicitAction::Accept);
        assert!(store.list(None).await.is_empty());
        assert!(
            store
                .resolve(&view.id, ElicitResult::decline())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn decline_drops_content_and_timeout_cancels() {
        let store = PendingElicitations::new(Duration::from_millis(1));
        assert_eq!(store.timeout(), Duration::from_secs(1));
        let store = PendingElicitations {
            pending: Mutex::new(HashMap::new()),
            timeout: Duration::from_millis(20),
        };
        let (view, rx) = store
            .create("srv", None, "q", ElicitationSchema::default())
            .await;
        let declined = ElicitResult {
            action: ElicitAction::Decline,
            content: Some(Map::new()),
        };
        store.resolve(&view.id, declined).await.unwrap();
        assert_eq!(store.wait(&view.id, rx).await, ElicitResult::decline());

        let (view, rx) = store
            .create("srv", None, "q", ElicitationSchema::default())
            .await;
        assert_eq!(store.wait(&view.id, rx).await, ElicitResult::cancel());
        assert!(store.list(None).await.is_empty());
    }
}
