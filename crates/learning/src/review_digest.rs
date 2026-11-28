//! Turn a session history into the bounded text the review reads: recent
//! turns verbatim, older turns one line each.

use serde::{Deserialize, Serialize};

const OLD_USER_CHARS: usize = 300;
const OLD_ASSISTANT_CHARS: usize = 200;
const TOOL_RESULT_CHARS: usize = 400;
const VERBATIM_CHARS: usize = 4_000;

/// Speaker of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// One message of the reviewed conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    /// Tools the assistant called in this message, or the tool a result
    /// came from.
    #[serde(default)]
    pub tools: Vec<String>,
}

impl Turn {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
            tools: Vec::new(),
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>, tools: Vec<String>) -> Self {
        Self {
            role: Role::Assistant,
            text: text.into(),
            tools,
        }
    }

    #[must_use]
    pub fn tool(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            text: text.into(),
            tools: vec![name.into()],
        }
    }
}

/// User turns and tool calls in a slice of history.
#[must_use]
pub fn activity(turns: &[Turn]) -> (usize, usize) {
    let users = turns.iter().filter(|turn| turn.role == Role::User).count();
    let tools = turns
        .iter()
        .filter(|turn| turn.role == Role::Assistant)
        .map(|turn| turn.tools.len())
        .sum();
    (users, tools)
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut clipped: String = flat.chars().take(max).collect();
    clipped.push('…');
    clipped
}

/// Keep line breaks, cut long messages.
fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

fn verbatim(turn: &Turn) -> String {
    let text = truncate(&turn.text, VERBATIM_CHARS);
    match turn.role {
        Role::User => format!("USER:\n{text}"),
        Role::Assistant if turn.tools.is_empty() => format!("ASSISTANT:\n{text}"),
        Role::Assistant => format!("ASSISTANT [called: {}]:\n{text}", turn.tools.join(", ")),
        Role::Tool => format!(
            "TOOL RESULT ({}): {}",
            turn.tools.join(", "),
            clip(&turn.text, TOOL_RESULT_CHARS)
        ),
    }
}

fn one_line(turn: &Turn) -> Option<String> {
    match turn.role {
        Role::User if !turn.text.trim().is_empty() => {
            Some(format!("USER: {}", clip(&turn.text, OLD_USER_CHARS)))
        },
        Role::Assistant => {
            let tools =
                (!turn.tools.is_empty()).then(|| format!("[tools: {}] ", turn.tools.join(", ")));
            let text = clip(&turn.text, OLD_ASSISTANT_CHARS);
            (tools.is_some() || !text.is_empty())
                .then(|| format!("ASSISTANT: {}{text}", tools.unwrap_or_default()))
        },
        Role::User | Role::Tool => None,
    }
}

/// Render the conversation within `budget_chars`: the most recent turns
/// verbatim, older turns condensed. The verbatim run never starts on a tool
/// result, which would read as an answer to nothing.
#[must_use]
pub fn digest(turns: &[Turn], budget_chars: usize) -> String {
    let rendered: Vec<String> = turns.iter().map(verbatim).collect();
    let mut used = 0;
    let mut start = turns.len();
    while start > 0 {
        let cost = rendered[start - 1].len() + 2;
        if used + cost > budget_chars && start < turns.len() {
            break;
        }
        used += cost;
        start -= 1;
    }
    while start < turns.len() && turns[start].role == Role::Tool {
        start += 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push_str("[Earlier turns, condensed]\n");
        let older_budget = budget_chars.saturating_sub(used);
        let mut older: Vec<String> = turns[..start].iter().filter_map(one_line).collect();
        let mut total: usize = older.iter().map(|line| line.len() + 1).sum();
        while total > older_budget && !older.is_empty() {
            total -= older.remove(0).len() + 1;
        }
        for line in older {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("\n[Recent turns, verbatim]\n");
    }
    out.push_str(&rendered[start..].join("\n\n"));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn history() -> Vec<Turn> {
        vec![
            Turn::user("set up the release"),
            Turn::assistant("checking", vec!["exec".into()]),
            Turn::tool("exec", "ok"),
            Turn::user("no, never merge; rebase"),
            Turn::assistant("rebased", vec![]),
        ]
    }

    #[test]
    fn short_histories_are_verbatim() {
        let text = digest(&history(), 10_000);
        assert!(!text.contains("condensed"));
        assert!(text.contains("USER:\nno, never merge; rebase"));
        assert!(text.contains("ASSISTANT [called: exec]"));
        assert!(text.contains("TOOL RESULT (exec): ok"));
    }

    #[test]
    fn long_histories_condense_older_turns() {
        let mut turns = Vec::new();
        for index in 0..40 {
            turns.push(Turn::user(format!("question {index} {}", "x".repeat(200))));
            turns.push(Turn::assistant(format!("answer {index}"), vec![
                "exec".into(),
            ]));
            turns.push(Turn::tool("exec", "y".repeat(1_000)));
        }
        let text = digest(&turns, 3_000);
        assert!(text.contains("[Earlier turns, condensed]"));
        assert!(text.contains("question 39"));
        assert!(text.len() < 4_500, "{}", text.len());
        let recent = text.split("[Recent turns, verbatim]\n").nth(1).unwrap();
        assert!(!recent.starts_with("TOOL RESULT"));
    }

    #[test]
    fn counts_activity() {
        assert_eq!(activity(&history()), (2, 1));
        assert_eq!(activity(&[]), (0, 0));
    }

    #[test]
    fn huge_messages_are_cut() {
        let text = digest(&[Turn::user("z".repeat(10_000))], 100_000);
        assert!(text.len() < 4_100);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn clip_marks_truncation() {
        assert_eq!(clip("a  b\nc", 10), "a b c");
        assert_eq!(clip("abcdef", 3), "abc…");
    }
}
