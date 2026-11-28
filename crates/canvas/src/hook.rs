//! Hook that versions designs once per agent turn.
//!
//! `BeforeToolCall` notes which designs `Write`/`Edit`/`MultiEdit` touch (and
//! refuses writes into the designs root that are not inside an existing
//! design); `AgentEnd` snapshots every touched design that changed.

use std::{path::Path, sync::Arc};

use {
    antre_common::hooks::{HookAction, HookEvent, HookHandler, HookPayload},
    async_trait::async_trait,
    serde_json::Value,
    tracing::debug,
};

use crate::{CanvasService, paths::DesignsLocation};

const MUTATING_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit"];

/// Versions designs at the end of each agent turn.
pub struct DesignVersionHook {
    service: Arc<CanvasService>,
}

impl DesignVersionHook {
    #[must_use]
    pub fn new(service: Arc<CanvasService>) -> Self {
        Self { service }
    }

    fn file_path(arguments: &Value) -> Option<&str> {
        arguments
            .get("file_path")
            .or_else(|| arguments.get("path"))
            .and_then(Value::as_str)
    }

    async fn before_tool_call(
        &self,
        session_key: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> HookAction {
        if !MUTATING_TOOLS.contains(&tool_name) {
            return HookAction::Continue;
        }
        let Some(file_path) = Self::file_path(arguments) else {
            return HookAction::Continue;
        };
        let id = match self.service.locate(Path::new(file_path)) {
            DesignsLocation::Outside => return HookAction::Continue,
            DesignsLocation::Stray => {
                return HookAction::Block(format!(
                    "{file_path} is inside the designs folder but not inside a design. \
                     Create one with design_create and write under the directory it returns."
                ));
            },
            DesignsLocation::Design(id) => id,
        };
        match self.service.store().exists(&id).await {
            Ok(true) => {},
            Ok(false) => {
                return HookAction::Block(format!(
                    "design {id} does not exist. Create one with design_create first."
                ));
            },
            Err(error) => {
                debug!(%error, "could not check design before tool call");
                return HookAction::Continue;
            },
        }
        let run_id = arguments.get("_run_id").and_then(Value::as_str);
        if let Some(previous) = self.service.touch(session_key, run_id, &id) {
            self.service.commit_turn(previous).await;
        }
        HookAction::Continue
    }
}

#[async_trait]
impl HookHandler for DesignVersionHook {
    fn name(&self) -> &str {
        "canvas_design_versions"
    }

    fn events(&self) -> &[HookEvent] {
        &[HookEvent::BeforeToolCall, HookEvent::AgentEnd]
    }

    fn priority(&self) -> i32 {
        // Before the auto-checkpoint hook (100) so a blocked write never gets
        // a checkpoint.
        110
    }

    async fn handle(
        &self,
        _event: HookEvent,
        payload: &HookPayload,
    ) -> antre_common::Result<HookAction> {
        match payload {
            HookPayload::BeforeToolCall {
                session_key,
                tool_name,
                arguments,
                ..
            } => Ok(self
                .before_tool_call(session_key, tool_name, arguments)
                .await),
            HookPayload::AgentEnd { session_key, .. } => {
                if let Some(turn) = self.service.end_turn(session_key) {
                    self.service.commit_turn(turn).await;
                }
                Ok(HookAction::Continue)
            },
            _ => Ok(HookAction::Continue),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use {
        super::*,
        crate::service::{CreateDesign, tests::service_with},
    };

    #[tokio::test]
    async fn versions_touched_designs_at_agent_end() {
        let (_tmp, service, _sink) = service_with(None).await;
        let created = service
            .create(CreateDesign {
                title: "t".into(),
                ..CreateDesign::default()
            })
            .await
            .unwrap();
        let hook = DesignVersionHook::new(Arc::clone(&service));
        let index = created.dir.join("index.html");
        let before = HookPayload::BeforeToolCall {
            session_key: "s".into(),
            tool_name: "Write".into(),
            arguments: json!({"file_path": index, "_run_id": "r1"}),
            channel: None,
        };
        assert!(matches!(
            hook.handle(HookEvent::BeforeToolCall, &before)
                .await
                .unwrap(),
            HookAction::Continue
        ));
        std::fs::write(&index, "<p>changed</p>").unwrap();
        let end = HookPayload::AgentEnd {
            session_key: "s".into(),
            text: String::new(),
            iterations: 1,
            tool_calls: 1,
        };
        hook.handle(HookEvent::AgentEnd, &end).await.unwrap();
        let versions = service.versions(&created.design.id, 10).await.unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].reason, "agent turn");
        assert_eq!(versions[0].run_id.as_deref(), Some("r1"));

        // A turn that touched nothing records nothing.
        hook.handle(HookEvent::AgentEnd, &end).await.unwrap();
        assert_eq!(
            service
                .versions(&created.design.id, 10)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn new_run_flushes_previous_turn() {
        let (_tmp, service, _sink) = service_with(None).await;
        let created = service
            .create(CreateDesign {
                title: "t".into(),
                ..CreateDesign::default()
            })
            .await
            .unwrap();
        let hook = DesignVersionHook::new(Arc::clone(&service));
        let index = created.dir.join("index.html");
        let call = |run: &str| HookPayload::BeforeToolCall {
            session_key: "s".into(),
            tool_name: "Edit".into(),
            arguments: json!({"file_path": index, "_run_id": run}),
            channel: None,
        };
        hook.handle(HookEvent::BeforeToolCall, &call("r1"))
            .await
            .unwrap();
        std::fs::write(&index, "<p>r1</p>").unwrap();
        hook.handle(HookEvent::BeforeToolCall, &call("r2"))
            .await
            .unwrap();
        let versions = service.versions(&created.design.id, 10).await.unwrap();
        assert_eq!(versions[0].run_id.as_deref(), Some("r1"));
    }

    #[tokio::test]
    async fn blocks_writes_outside_existing_designs() {
        let (_tmp, service, _sink) = service_with(None).await;
        let hook = DesignVersionHook::new(Arc::clone(&service));
        let root = service.designs_root().to_path_buf();
        let missing = crate::DesignId::generate();
        for path in [
            root.join("stray.html"),
            root.join("not-a-design/index.html"),
            root.join(missing.as_str()).join("index.html"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let payload = HookPayload::BeforeToolCall {
                session_key: "s".into(),
                tool_name: "Write".into(),
                arguments: json!({"file_path": path}),
                channel: None,
            };
            assert!(
                matches!(
                    hook.handle(HookEvent::BeforeToolCall, &payload)
                        .await
                        .unwrap(),
                    HookAction::Block(_)
                ),
                "{} should be blocked",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn ignores_other_tools_and_paths() {
        let (tmp, service, _sink) = service_with(None).await;
        let hook = DesignVersionHook::new(Arc::clone(&service));
        for (tool, path) in [
            ("Read", service.designs_root().join("stray.html")),
            ("Write", tmp.path().join("notes.txt")),
        ] {
            let payload = HookPayload::BeforeToolCall {
                session_key: "s".into(),
                tool_name: tool.into(),
                arguments: json!({"file_path": path}),
                channel: None,
            };
            assert!(matches!(
                hook.handle(HookEvent::BeforeToolCall, &payload)
                    .await
                    .unwrap(),
                HookAction::Continue
            ));
        }
        assert!(service.end_turn("s").is_none());
    }
}
