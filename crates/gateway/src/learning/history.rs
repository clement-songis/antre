//! Session history as the review reads it.

use std::collections::{HashMap, HashSet};

use {
    moltis_agents::model::{ChatMessage, ContentPart, UserContent},
    moltis_learning::review_digest::Turn,
};

const READ_SKILL_TOOL: &str = "read_skill";

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Multimodal(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text(text) => text.as_str(),
                ContentPart::Image { .. } => "[image]",
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Convert stored messages to review turns, and collect the skills the
/// agent loaded with `read_skill` during the session.
pub(super) fn turns_from_messages(messages: &[ChatMessage]) -> (Vec<Turn>, HashSet<String>) {
    let mut tool_names: HashMap<&str, &str> = HashMap::new();
    let mut loaded = HashSet::new();
    let mut turns = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            ChatMessage::System { .. } => {},
            ChatMessage::User { content, .. } => turns.push(Turn::user(user_text(content))),
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                for call in tool_calls {
                    tool_names.insert(&call.id, &call.name);
                    if call.name == READ_SKILL_TOOL
                        && let Some(name) = call.arguments.get("name").and_then(|v| v.as_str())
                    {
                        loaded.insert(name.to_string());
                    }
                }
                turns.push(Turn::assistant(
                    content.clone().unwrap_or_default(),
                    tool_calls.iter().map(|call| call.name.clone()).collect(),
                ));
            },
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                let name = tool_names
                    .get(tool_call_id.as_str())
                    .copied()
                    .unwrap_or("tool");
                turns.push(Turn::tool(name, content.clone()));
            },
        }
    }
    (turns, loaded)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, moltis_agents::model::ToolCall, moltis_learning::review_digest::Role};

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            argument_diagnostic: None,
            metadata: None,
        }
    }

    #[test]
    fn converts_messages_and_finds_loaded_skills() {
        let messages = vec![
            ChatMessage::system("prompt"),
            ChatMessage::user("cut the release"),
            ChatMessage::Assistant {
                content: None,
                tool_calls: vec![
                    call(
                        "1",
                        "read_skill",
                        serde_json::json!({"name": "release-process"}),
                    ),
                    call("2", "exec", serde_json::json!({"command": "git tag"})),
                ],
                reasoning: None,
            },
            ChatMessage::Tool {
                tool_call_id: "2".into(),
                content: "ok".into(),
            },
            ChatMessage::Tool {
                tool_call_id: "unknown".into(),
                content: "?".into(),
            },
            ChatMessage::User {
                content: UserContent::Multimodal(vec![
                    ContentPart::Text("look".into()),
                    ContentPart::Image {
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                ]),
                name: None,
            },
        ];
        let (turns, loaded) = turns_from_messages(&messages);
        assert_eq!(turns.len(), 5);
        assert_eq!(turns[1].tools, ["read_skill", "exec"]);
        assert_eq!(turns[2].role, Role::Tool);
        assert_eq!(turns[2].tools, ["exec"]);
        assert_eq!(turns[3].tools, ["tool"]);
        assert_eq!(turns[4].text, "look [image]");
        assert_eq!(loaded, HashSet::from(["release-process".to_string()]));
    }
}
