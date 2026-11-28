//! Adapters exposing mod components through the existing `AgentTool` and
//! `HookHandler` traits.

use std::sync::Arc;

use {
    antre_agents::tool_registry::AgentTool,
    antre_common::hooks::{HookAction, HookEvent, HookHandler, HookPayload},
    async_trait::async_trait,
    serde_json::Value,
};

#[cfg(feature = "wasm")]
use {
    crate::{
        capability::{Capability, HookMode},
        host::ModComponent,
        manifest::ToolDecl,
    },
    tracing::{debug, warn},
};

/// Forwards to a shared tool so one contribution can be handed to the
/// `ToolRegistry` (which takes ownership of a `Box`) and kept in the
/// contribution registry at the same time.
pub struct SharedTool(pub Arc<dyn AgentTool>);

#[async_trait]
impl AgentTool for SharedTool {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters_schema(&self) -> Value {
        self.0.parameters_schema()
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        self.0.warmup().await
    }

    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        self.0.execute(params).await
    }
}

/// Forwards to a shared hook handler.
pub struct SharedHook(pub Arc<dyn HookHandler>);

#[async_trait]
impl HookHandler for SharedHook {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn events(&self) -> &[HookEvent] {
        self.0.events()
    }

    fn priority(&self) -> i32 {
        self.0.priority()
    }

    async fn handle(
        &self,
        event: HookEvent,
        payload: &HookPayload,
    ) -> antre_common::Result<HookAction> {
        self.0.handle(event, payload).await
    }

    fn handle_sync(
        &self,
        event: HookEvent,
        payload: &HookPayload,
    ) -> antre_common::Result<HookAction> {
        self.0.handle_sync(event, payload)
    }
}

/// A tool implemented by a mod component.
#[cfg(feature = "wasm")]
pub struct ModTool {
    decl: ToolDecl,
    component: Arc<ModComponent>,
}

#[cfg(feature = "wasm")]
impl ModTool {
    #[must_use]
    pub fn new(decl: ToolDecl, component: Arc<ModComponent>) -> Self {
        Self { decl, component }
    }
}

#[cfg(feature = "wasm")]
#[async_trait]
impl AgentTool for ModTool {
    fn name(&self) -> &str {
        &self.decl.name
    }

    fn description(&self) -> &str {
        &self.decl.description
    }

    fn parameters_schema(&self) -> Value {
        self.decl.parameters.clone()
    }

    #[tracing::instrument(skip(self, params), fields(mod_id = %self.component.mod_id(), tool = %self.decl.name))]
    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        let component = Arc::clone(&self.component);
        let name = self.decl.name.clone();
        let params_json = serde_json::to_string(&params)?;
        let outcome =
            tokio::task::spawn_blocking(move || component.call_tool(&name, &params_json)).await??;
        match outcome {
            Ok(text) => Ok(serde_json::from_str(&text).unwrap_or(Value::String(text))),
            Err(message) => anyhow::bail!("{message}"),
        }
    }
}

/// A hook handler implemented by a mod component.
#[cfg(feature = "wasm")]
pub struct ModHook {
    name: String,
    events: Vec<HookEvent>,
    mode: HookMode,
    priority: i32,
    component: Arc<ModComponent>,
}

#[cfg(feature = "wasm")]
impl ModHook {
    #[must_use]
    pub fn new(
        name: String,
        events: Vec<HookEvent>,
        mode: HookMode,
        priority: i32,
        component: Arc<ModComponent>,
    ) -> Self {
        Self {
            name,
            events,
            mode,
            priority,
            component,
        }
    }

    /// Block/modify answers only count when the hook declared intercept mode,
    /// the event can be intercepted, and the grant covers interception.
    fn may_intercept(&self, event: HookEvent) -> bool {
        self.mode == HookMode::Intercept
            && !event.is_read_only()
            && self.component.granted().allows(&Capability::Hook {
                event,
                mode: HookMode::Intercept,
            })
    }

    fn interpret(
        &self,
        event: HookEvent,
        answer: Result<Option<String>, String>,
    ) -> antre_common::Result<HookAction> {
        let intercept = self.may_intercept(event);
        match answer {
            Ok(None) => Ok(HookAction::Continue),
            Ok(Some(json)) if intercept => serde_json::from_str(&json)
                .map(HookAction::ModifyPayload)
                .map_err(|error| {
                    antre_common::Error::message(format!(
                        "mod hook `{}` returned invalid JSON: {error}",
                        self.name
                    ))
                }),
            Err(reason) if intercept => Ok(HookAction::Block(reason)),
            Ok(Some(_)) | Err(_) => {
                debug!(hook = %self.name, %event, "observer hook answer ignored");
                Ok(HookAction::Continue)
            },
        }
    }

    fn payload_json(payload: &HookPayload) -> antre_common::Result<String> {
        serde_json::to_string(payload)
            .map_err(|error| antre_common::Error::message(error.to_string()))
    }
}

#[cfg(feature = "wasm")]
#[async_trait]
impl HookHandler for ModHook {
    fn name(&self) -> &str {
        &self.name
    }

    fn events(&self) -> &[HookEvent] {
        &self.events
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[tracing::instrument(skip(self, payload), fields(hook = %self.name))]
    async fn handle(
        &self,
        event: HookEvent,
        payload: &HookPayload,
    ) -> antre_common::Result<HookAction> {
        let payload_json = Self::payload_json(payload)?;
        let component = Arc::clone(&self.component);
        let event_name = event.to_string();
        let answer =
            tokio::task::spawn_blocking(move || component.handle_hook(&event_name, &payload_json))
                .await
                .map_err(|error| antre_common::Error::message(error.to_string()))?
                .map_err(|error| {
                    warn!(hook = %self.name, %error, "mod hook failed");
                    antre_common::Error::message(error.to_string())
                })?;
        self.interpret(event, answer)
    }

    /// The sync path (`ToolResultPersist`) calls the component directly; it is
    /// already synchronous, so no runtime is blocked on.
    fn handle_sync(
        &self,
        event: HookEvent,
        payload: &HookPayload,
    ) -> antre_common::Result<HookAction> {
        let payload_json = Self::payload_json(payload)?;
        let answer = self
            .component
            .handle_hook(&event.to_string(), &payload_json)
            .map_err(|error| antre_common::Error::message(error.to_string()))?;
        self.interpret(event, answer)
    }
}
