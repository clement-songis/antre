use std::collections::HashMap;

use super::{Diagnostic, Severity};

use crate::schema::{AgentPreset, ToolChoice};

pub(crate) fn check_agent_presets(
    presets: &HashMap<String, AgentPreset>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (name, preset) in presets {
        if preset.max_iterations == Some(0) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                category: "invalid-value",
                path: format!("agents.presets.{name}.max_iterations"),
                message: "agents.presets.<name>.max_iterations must be at least 1".into(),
            });
        }
        if let Some(ToolChoice::Tool { name: tool_name }) = &preset.tool_controls.tool_choice {
            if tool_name.trim().is_empty() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    category: "invalid-value",
                    path: format!("agents.presets.{name}.tool_controls.tool_choice.name"),
                    message: "forced tool_choice requires a non-empty name".into(),
                });
            }
            if let Some(active_tools) = &preset.tool_controls.active_tools
                && !active_tools.iter().any(|active| active == tool_name)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    category: "invalid-value",
                    path: format!("agents.presets.{name}.tool_controls"),
                    message: format!(
                        "forced tool_choice `{tool_name}` must be included in active_tools"
                    ),
                });
            }
        }
    }
}
