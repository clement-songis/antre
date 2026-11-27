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
                let client = slot.read_client().await?;
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

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use {super::*, async_trait::async_trait, serde_json::json};

    use crate::{
        client::McpClientState,
        protocol::ResourceContents,
        registry::{McpRegistry, McpServerConfig},
        types::{PromptsCapability, ResourcesCapability, ServerCapabilities, ToolsCallResult},
    };

    struct FakeClient {
        name: &'static str,
        capabilities: ServerCapabilities,
        tools: Vec<McpToolDef>,
        roots_notified: Arc<AtomicUsize>,
        fail_resources: bool,
    }

    impl FakeClient {
        fn new(name: &'static str, roots_notified: Arc<AtomicUsize>) -> Self {
            Self {
                name,
                capabilities: ServerCapabilities {
                    resources: Some(ResourcesCapability {
                        subscribe: true,
                        list_changed: false,
                    }),
                    prompts: Some(PromptsCapability::default()),
                    ..ServerCapabilities::default()
                },
                tools: vec![McpToolDef {
                    name: "chart".into(),
                    meta: Some(json!({"ui": {"resourceUri": "ui://chart"}})),
                    ..Default::default()
                }],
                roots_notified,
                fail_resources: false,
            }
        }
    }

    #[async_trait]
    impl McpClientTrait for FakeClient {
        fn server_name(&self) -> &str {
            self.name
        }

        fn state(&self) -> McpClientState {
            McpClientState::Ready
        }

        fn tools(&self) -> &[McpToolDef] {
            &self.tools
        }

        async fn list_tools(&mut self) -> Result<&[McpToolDef]> {
            self.tools.push(McpToolDef {
                name: format!("added_{}", self.tools.len()),
                ..Default::default()
            });
            Ok(&self.tools)
        }

        async fn call_tool(&self, _: &str, _: serde_json::Value) -> Result<ToolsCallResult> {
            Err(Error::message("unused"))
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn shutdown(&mut self) {}

        fn server_capabilities(&self) -> Option<&ServerCapabilities> {
            Some(&self.capabilities)
        }

        async fn list_resources(&self) -> Result<Vec<McpResource>> {
            if self.fail_resources {
                return Err(Error::message("boom"));
            }
            Ok(vec![McpResource {
                uri: format!("file:///{}", self.name),
                ..Default::default()
            }])
        }

        async fn list_resource_templates(&self) -> Result<Vec<McpResourceTemplate>> {
            Ok(Vec::new())
        }

        async fn read_resource(&self, uri: &str) -> Result<ResourcesReadResult> {
            Ok(ResourcesReadResult {
                contents: vec![ResourceContents {
                    uri: uri.into(),
                    mime_type: Some(crate::protocol::MCP_APP_MIME_TYPE.into()),
                    text: Some("<p>chart</p>".into()),
                    ..Default::default()
                }],
            })
        }

        async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
            Ok(vec![McpPrompt {
                name: "review".into(),
                arguments: vec![crate::protocol::PromptArgument {
                    name: "code".into(),
                    required: true,
                    ..Default::default()
                }],
                ..Default::default()
            }])
        }

        async fn get_prompt(
            &self,
            name: &str,
            _: HashMap<String, String>,
        ) -> Result<PromptsGetResult> {
            Ok(PromptsGetResult {
                description: Some(name.into()),
                messages: Vec::new(),
            })
        }

        async fn notify_roots_changed(&self) -> Result<()> {
            self.roots_notified.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    async fn manager_with(clients: Vec<FakeClient>) -> McpManager {
        let mut registry = McpRegistry::new();
        for client in &clients {
            registry
                .servers
                .insert(client.name.into(), McpServerConfig {
                    command: "x".into(),
                    trust: if client.name == "b" {
                        McpTrustTier::Untrusted
                    } else {
                        McpTrustTier::Trusted
                    },
                    ..Default::default()
                });
        }
        let manager = McpManager::new(registry);
        {
            let mut inner = manager.inner.write().await;
            for client in clients {
                let name = client.name.to_string();
                inner.tools.insert(name.clone(), client.tools.clone());
                let shared: Arc<RwLock<dyn McpClientTrait>> = Arc::new(RwLock::new(client));
                inner
                    .clients
                    .insert(name, Arc::new(ClientSlot::with_client(shared)));
            }
        }
        manager
    }

    #[tokio::test]
    async fn summaries_resources_and_prompt_commands_span_servers() {
        let notified = Arc::new(AtomicUsize::new(0));
        let mut failing = FakeClient::new("b", Arc::clone(&notified));
        failing.fail_resources = true;
        let manager =
            manager_with(vec![FakeClient::new("a", Arc::clone(&notified)), failing]).await;

        let summaries = manager.server_summaries().await;
        assert_eq!(summaries.len(), 2);
        assert!(summaries[0].resources && summaries[0].resource_subscribe && summaries[0].prompts);
        assert!(summaries[0].apps);
        assert_eq!(summaries[1].trust, McpTrustTier::Untrusted);

        let all = manager.list_all_resources().await;
        assert_eq!(all.len(), 2);
        let a = all.iter().find(|entry| entry.server == "a").unwrap();
        assert_eq!(a.resources[0].uri, "file:///a");
        let b = all.iter().find(|entry| entry.server == "b").unwrap();
        assert_eq!(b.error.as_deref(), Some("boom"));

        let commands = manager.prompt_commands().await;
        let names: Vec<&str> = commands.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(names, vec!["a:review", "b:review"]);
    }

    #[tokio::test]
    async fn get_prompt_checks_required_arguments() {
        let manager = manager_with(vec![FakeClient::new("a", Arc::default())]).await;
        let missing = manager.get_prompt("a", "review", HashMap::new()).await;
        assert!(missing.unwrap_err().to_string().contains("code"));
        let ok = manager
            .get_prompt("a", "review", HashMap::from([("code".into(), "x".into())]))
            .await
            .unwrap();
        assert_eq!(ok.description.as_deref(), Some("review"));
        assert!(manager.list_prompts("missing").await.is_err());
    }

    #[tokio::test]
    async fn roots_notify_only_on_change() {
        let notified = Arc::new(AtomicUsize::new(0));
        let manager = manager_with(vec![FakeClient::new("a", Arc::clone(&notified))]).await;
        let root = Root::from_path(std::path::Path::new("/w"), None).unwrap();
        manager.set_roots(vec![root.clone()]).await;
        manager.set_roots(vec![root.clone()]).await;
        assert_eq!(notified.load(Ordering::SeqCst), 1);
        assert_eq!(manager.roots(), vec![root]);
    }

    #[tokio::test]
    async fn ui_resource_refresh_and_errors() {
        let manager = manager_with(vec![FakeClient::new("a", Arc::default())]).await;
        let ui = manager.read_ui_resource("a", "ui://chart").await.unwrap();
        assert_eq!(ui.html, "<p>chart</p>");
        assert!(manager.read_ui_resource("a", "file:///x").await.is_err());

        let tools = manager.refresh_tools("a").await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(manager.server_tools("a").await.unwrap().len(), 2);
        assert!(manager.refresh_tools("nope").await.is_err());

        assert!(
            manager.subscribe_resource("a", "file:///a").await.is_err(),
            "fake lacks subscribe"
        );
        assert_eq!(manager.list_resources("a").await.unwrap().len(), 1);
        assert!(
            manager
                .list_resource_templates("a")
                .await
                .unwrap()
                .is_empty()
        );
    }
}
