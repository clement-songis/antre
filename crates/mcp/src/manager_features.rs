//! Resources, prompts, roots and MCP Apps operations of [`McpManager`].

use std::{collections::HashMap, sync::Arc};

use {
    futures::future::join_all,
    serde::Serialize,
    tokio::sync::{OwnedRwLockReadGuard, RwLock},
    tracing::{debug, warn},
};

use crate::{
    client_slot::ClientSlot,
    error::{Error, Result},
    manager::McpManager,
    protocol::{
        McpPrompt, McpResource, McpResourceTemplate, PromptCommand, PromptsGetResult,
        ResourcesReadResult, Root, UiResource,
    },
    registry::McpTrustTier,
    traits::McpClientTrait,
    types::{McpManagerError, McpToolDef},
};

/// What a running server offers beyond tools.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSummary {
    pub name: String,
    pub trust: McpTrustTier,
    pub resources: bool,
    pub resource_subscribe: bool,
    pub prompts: bool,
    pub apps: bool,
}

/// Resources of one server, as returned by the fan-out listing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerResources {
    pub server: String,
    pub trust: McpTrustTier,
    pub resources: Vec<McpResource>,
    pub templates: Vec<McpResourceTemplate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl McpManager {
    async fn running_slots(&self) -> Vec<(String, Arc<ClientSlot>)> {
        let inner = self.inner.read().await;
        let mut slots: Vec<_> = inner
            .clients
            .iter()
            .map(|(name, slot)| (name.clone(), Arc::clone(slot)))
            .collect();
        slots.sort_by(|left, right| left.0.cmp(&right.0));
        slots
    }

    async fn client(&self, server: &str) -> Result<OwnedRwLockReadGuard<dyn McpClientTrait>> {
        let slot = self
            .inner
            .read()
            .await
            .clients
            .get(server)
            .cloned()
            .ok_or_else(|| McpManagerError::ServerNotFound {
                server: server.to_string(),
            })?;
        slot.read_client()
            .await
            .ok_or_else(|| Error::message(format!("MCP server '{server}' is not running")))
    }

    async fn trust_of(&self, server: &str) -> McpTrustTier {
        self.inner
            .read()
            .await
            .registry
            .get(server)
            .map(|config| config.trust)
            .unwrap_or_default()
    }

    /// Capabilities summary of every running server.
    pub async fn server_summaries(&self) -> Vec<McpServerSummary> {
        let mut summaries = Vec::new();
        for (name, slot) in self.running_slots().await {
            let Some(client) = slot.read_client().await else {
                continue;
            };
            let caps = client.server_capabilities();
            let resources = caps.and_then(|caps| caps.resources.as_ref());
            let has_ui = self
                .server_tools(&name)
                .await
                .unwrap_or_default()
                .iter()
                .any(|tool| tool.ui().is_some());
            summaries.push(McpServerSummary {
                trust: self.trust_of(&name).await,
                resources: resources.is_some(),
                resource_subscribe: resources.is_some_and(|caps| caps.subscribe),
                prompts: caps.is_some_and(|caps| caps.prompts.is_some()),
                apps: has_ui,
                name,
            });
        }
        summaries
    }

    pub async fn list_resources(&self, server: &str) -> Result<Vec<McpResource>> {
        self.client(server).await?.list_resources().await
    }

    pub async fn list_resource_templates(&self, server: &str) -> Result<Vec<McpResourceTemplate>> {
        self.client(server).await?.list_resource_templates().await
    }

    /// Resources and templates of every running server, fetched concurrently.
    /// A failing server is reported in its entry instead of failing the call.
    pub async fn list_all_resources(&self) -> Vec<ServerResources> {
        let slots = self.running_slots().await;
        let mut trust = HashMap::new();
        for (name, _) in &slots {
            trust.insert(name.clone(), self.trust_of(name).await);
        }
        join_all(slots.into_iter().map(|(name, slot)| {
            let trust = trust.get(&name).copied().unwrap_or_default();
            async move {
                let mut entry = ServerResources {
                    server: name.clone(),
                    trust,
                    resources: Vec::new(),
                    templates: Vec::new(),
                    error: None,
                };
                let Some(client) = slot.read_client().await else {
                    return None;
                };
                if client
                    .server_capabilities()
                    .is_none_or(|caps| caps.resources.is_none())
                {
                    return None;
                }
                let (resources, templates) =
                    tokio::join!(client.list_resources(), client.list_resource_templates());
                match (resources, templates) {
                    (Ok(resources), Ok(templates)) => {
                        entry.resources = resources;
                        entry.templates = templates;
                    },
                    (Err(error), _) | (_, Err(error)) => {
                        warn!(server = %name, %error, "listing MCP resources failed");
                        entry.error = Some(error.to_string());
                    },
                }
                Some(entry)
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }

    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<ResourcesReadResult> {
        self.client(server).await?.read_resource(uri).await
    }

    pub async fn subscribe_resource(&self, server: &str, uri: &str) -> Result<()> {
        self.client(server).await?.subscribe_resource(uri).await
    }

    pub async fn unsubscribe_resource(&self, server: &str, uri: &str) -> Result<()> {
        self.client(server).await?.unsubscribe_resource(uri).await
    }

    pub async fn list_prompts(&self, server: &str) -> Result<Vec<McpPrompt>> {
        self.client(server).await?.list_prompts().await
    }

    /// Prompts of every running server, as slash-command candidates.
    pub async fn prompt_commands(&self) -> Vec<PromptCommand> {
        let slots = self.running_slots().await;
        let lists = join_all(slots.into_iter().map(|(name, slot)| async move {
            let client = slot.read_client().await?;
            match client.list_prompts().await {
                Ok(prompts) => Some(
                    prompts
                        .iter()
                        .map(|prompt| PromptCommand::new(&name, prompt))
                        .collect::<Vec<_>>(),
                ),
                Err(error) => {
                    warn!(server = %name, %error, "listing MCP prompts failed");
                    None
                },
            }
        }))
        .await;
        lists.into_iter().flatten().flatten().collect()
    }

    /// Render a prompt. Required arguments are checked before the request
    /// so the composer gets a clear error.
    pub async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptsGetResult> {
        let client = self.client(server).await?;
        if let Some(prompt) = client
            .list_prompts()
            .await
            .ok()
            .and_then(|prompts| prompts.into_iter().find(|prompt| prompt.name == name))
        {
            let missing = prompt.missing_arguments(&arguments);
            if !missing.is_empty() {
                return Err(Error::message(format!(
                    "prompt '{name}' needs argument(s): {}",
                    missing.join(", ")
                )));
            }
        }
        client.get_prompt(name, arguments).await
    }

    /// Read a `ui://` resource for an MCP App host.
    pub async fn read_ui_resource(&self, server: &str, uri: &str) -> Result<UiResource> {
        if !uri.starts_with(crate::protocol::UI_RESOURCE_SCHEME) {
            return Err(Error::message(format!("'{uri}' is not a ui:// resource")));
        }
        let read = self.read_resource(server, uri).await?;
        UiResource::from_read(server, uri, read)
    }

    /// Replace the roots exposed to servers and notify running servers when
    /// the list changed.
    pub async fn set_roots(&self, roots: Vec<Root>) {
        if !self.host.roots.set(roots) {
            return;
        }
        for (name, slot) in self.running_slots().await {
            let Some(client) = slot.read_client().await else {
                continue;
            };
            if let Err(error) = client.notify_roots_changed().await {
                debug!(server = %name, %error, "failed to send roots/list_changed");
            }
        }
    }

    #[must_use]
    pub fn roots(&self) -> Vec<Root> {
        self.host.roots.get()
    }

    /// Re-fetch a server's tools after `notifications/tools/list_changed`.
    /// Returns the new tool list; callers resync the agent tool registry.
    pub async fn refresh_tools(&self, server: &str) -> Result<Vec<McpToolDef>> {
        let slot = self
            .inner
            .read()
            .await
            .clients
            .get(server)
            .cloned()
            .ok_or_else(|| McpManagerError::ServerNotFound {
                server: server.to_string(),
            })?;
        let client: Arc<RwLock<dyn McpClientTrait>> = slot
            .current()
            .await
            .ok_or_else(|| Error::message(format!("MCP server '{server}' is not running")))?;
        let tools = client.write().await.list_tools().await?.to_vec();
        let mut inner = self.inner.write().await;
        let still_current = inner
            .clients
            .get(server)
            .is_some_and(|current| Arc::ptr_eq(current, &slot));
        if still_current {
            inner.tools.insert(server.to_string(), tools.clone());
        }
        Ok(tools)
    }
}
