//! Claude Code wire format for shell hooks (`protocol: claude` in HOOK.md).
//!
//! stdin carries the payload with Claude field names (`hook_event_name`,
//! `session_id`, `tool_input`, `tool_response`); exit code 2 blocks with
//! stderr as the reason; on exit 0, stdout may carry a JSON decision.

use {
    serde::{Deserialize, Serialize},
    serde_json::{Map, Value},
};

use {
    crate::hooks::{HookAction, HookEvent, HookPayload},
    antre_common::{Error as HookError, Result as HookResult},
};

/// Which wire format a shell hook speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookProtocol {
    /// Antre format: exit 1 blocks, `{"action":"modify","data":…}` modifies.
    #[default]
    Native,
    /// Claude Code format: exit 2 blocks, Claude decision JSON on stdout.
    Claude,
}

/// Build the Claude-style stdin document for `payload`.
pub fn claude_stdin(event: HookEvent, payload: &HookPayload) -> HookResult<String> {
    let value = serde_json::to_value(payload)
        .map_err(|source| HookError::message(format!("failed to serialize payload: {source}")))?;
    let mut object = match value {
        Value::Object(object) => object,
        other => {
            let mut object = Map::new();
            object.insert("payload".into(), other);
            object
        },
    };
    object.remove("event");
    for (from, to) in [
        ("session_key", "session_id"),
        ("arguments", "tool_input"),
        ("result", "tool_response"),
    ] {
        if let Some(value) = object.remove(from) {
            object.insert(to.into(), value);
        }
    }
    object.insert(
        "hook_event_name".into(),
        Value::String(event.claude_name().into()),
    );
    serde_json::to_string(&Value::Object(object))
        .map_err(|source| HookError::message(format!("failed to serialize payload: {source}")))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOutput {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default, rename = "continue")]
    keep_going: Option<bool>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    hook_specific_output: Option<HookSpecificOutput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookSpecificOutput {
    #[serde(default)]
    permission_decision: Option<String>,
    #[serde(default)]
    permission_decision_reason: Option<String>,
    #[serde(default)]
    updated_input: Option<Value>,
}

/// Map a finished Claude-protocol hook process to an action.
pub fn interpret_claude(
    hook_name: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> HookResult<HookAction> {
    let default_reason = || format!("hook '{hook_name}' blocked the action");
    if exit_code == 2 {
        let reason = stderr.trim();
        return Ok(HookAction::Block(if reason.is_empty() {
            default_reason()
        } else {
            reason.to_string()
        }));
    }
    if exit_code != 0 {
        return Err(HookError::message(format!(
            "hook '{hook_name}' exited with code {exit_code}: {}",
            stderr.trim()
        )));
    }
    let stdout = stdout.trim();
    if stdout.is_empty() {
        return Ok(HookAction::Continue);
    }
    let Ok(output) = serde_json::from_str::<ClaudeOutput>(stdout) else {
        return Ok(HookAction::Continue);
    };
    if output.decision.as_deref() == Some("block") {
        return Ok(HookAction::Block(
            output.reason.unwrap_or_else(default_reason),
        ));
    }
    if output.keep_going == Some(false) {
        return Ok(HookAction::Block(
            output.stop_reason.unwrap_or_else(default_reason),
        ));
    }
    if let Some(specific) = output.hook_specific_output {
        if specific.permission_decision.as_deref() == Some("deny") {
            return Ok(HookAction::Block(
                specific
                    .permission_decision_reason
                    .unwrap_or_else(default_reason),
            ));
        }
        if let Some(updated) = specific.updated_input {
            return Ok(HookAction::ModifyPayload(updated));
        }
    }
    Ok(HookAction::Continue)
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_uses_claude_field_names() {
        let payload = HookPayload::BeforeToolCall {
            session_key: "main".into(),
            tool_name: "exec".into(),
            arguments: serde_json::json!({"command": "ls"}),
            channel: None,
        };
        let text = claude_stdin(HookEvent::BeforeToolCall, &payload).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["hook_event_name"], "PreToolUse");
        assert_eq!(value["session_id"], "main");
        assert_eq!(value["tool_name"], "exec");
        assert_eq!(value["tool_input"]["command"], "ls");
        assert!(value.get("event").is_none());
        assert!(value.get("arguments").is_none());
    }

    #[test]
    fn exit_two_blocks_with_stderr() {
        match interpret_claude("h", 2, "", "no rm -rf\n").unwrap() {
            HookAction::Block(reason) => assert_eq!(reason, "no rm -rf"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            interpret_claude("h", 2, "", "").unwrap(),
            HookAction::Block(_)
        ));
    }

    #[test]
    fn exit_one_is_an_error_not_a_block() {
        assert!(interpret_claude("h", 1, "", "oops").is_err());
    }

    #[test]
    fn json_decisions() {
        let cases = [
            (r#"{"decision":"block","reason":"nope"}"#, Some("nope")),
            (r#"{"continue":false,"stopReason":"halt"}"#, Some("halt")),
            (
                r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"denied"}}"#,
                Some("denied"),
            ),
            (
                r#"{"hookSpecificOutput":{"permissionDecision":"allow"}}"#,
                None,
            ),
            (r#"{"decision":"approve"}"#, None),
            ("not json", None),
            ("", None),
        ];
        for (stdout, blocked) in cases {
            match (interpret_claude("h", 0, stdout, "").unwrap(), blocked) {
                (HookAction::Block(reason), Some(expected)) => assert_eq!(reason, expected),
                (HookAction::Continue, None) => {},
                (other, _) => panic!("{stdout}: {other:?}"),
            }
        }
    }

    #[test]
    fn updated_input_modifies_payload() {
        let stdout = r#"{"hookSpecificOutput":{"updatedInput":{"command":"ls -la"}}}"#;
        match interpret_claude("h", 0, stdout, "").unwrap() {
            HookAction::ModifyPayload(value) => assert_eq!(value["command"], "ls -la"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn protocol_parses_from_frontmatter_values() {
        let native: HookProtocol = serde_json::from_str("\"native\"").unwrap();
        let claude: HookProtocol = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(native, HookProtocol::Native);
        assert_eq!(claude, HookProtocol::Claude);
        assert_eq!(HookProtocol::default(), HookProtocol::Native);
    }
}
