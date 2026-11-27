//! Resources, prompts and roots requests of [`McpClient`].

use std::collections::HashMap;

use {serde::de::DeserializeOwned, serde_json::json, tracing::debug};

use crate::{
    client::McpClient,
    error::{Context, Error, Result},
    protocol::{
        McpPrompt, McpResource, McpResourceTemplate, PromptsGetResult, PromptsListResult,
        ResourceTemplatesListResult, ResourcesListResult, ResourcesReadResult,
    },
    types::{PromptsCapability, ResourcesCapability},
};

/// Upper bound on pages fetched for one list call, so a server that keeps
/// returning cursors cannot loop the client forever.
const MAX_PAGES: usize = 100;

impl McpClient {
    fn resources_capability(&self) -> Option<&ResourcesCapability> {
        self.server_info
            .as_ref()
            .and_then(|info| info.capabilities.resources.as_ref())
    }

    fn prompts_capability(&self) -> Option<&PromptsCapability> {
        self.server_info
            .as_ref()
            .and_then(|info| info.capabilities.prompts.as_ref())
    }

    /// Fetch every page of a paginated list method.
    pub(crate) async fn paginate<P, T>(
        &self,
        method: &str,
        split: impl Fn(P) -> (Vec<T>, Option<String>),
    ) -> Result<Vec<T>>
    where
        P: DeserializeOwned,
    {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = cursor.as_ref().map(|cursor| json!({ "cursor": cursor }));
            let resp = self.transport.request(method, params).await?;
            let page: P = serde_json::from_value(
                resp.result
                    .with_context(|| format!("{method} returned no result"))?,
            )
            .with_context(|| format!("failed to parse {method} result"))?;
            let (mut page_items, next) = split(page);
            items.append(&mut page_items);
            match next {
                Some(next) if !next.is_empty() && Some(&next) != cursor.as_ref() => {
                    cursor = Some(next);
                },
                _ => return Ok(items),
            }
        }
        debug!(server = %self.server_name, method, "stopped paginating after {MAX_PAGES} pages");
        Ok(items)
    }

    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        self.ensure_ready()?;
        if self.resources_capability().is_none() {
            return Ok(Vec::new());
        }
        self.paginate("resources/list", |page: ResourcesListResult| {
            (page.resources, page.next_cursor)
        })
        .await
    }

    pub async fn list_resource_templates(&self) -> Result<Vec<McpResourceTemplate>> {
        self.ensure_ready()?;
        if self.resources_capability().is_none() {
            return Ok(Vec::new());
        }
        self.paginate(
            "resources/templates/list",
            |page: ResourceTemplatesListResult| (page.resource_templates, page.next_cursor),
        )
        .await
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ResourcesReadResult> {
        self.ensure_ready()?;
        let resp = self
            .transport
            .request("resources/read", Some(json!({ "uri": uri })))
            .await?;
        Ok(serde_json::from_value(
            resp.result.context("resources/read returned no result")?,
        )?)
    }

    fn ensure_subscribe(&self) -> Result<()> {
        if self
            .resources_capability()
            .is_some_and(|caps| caps.subscribe)
        {
            return Ok(());
        }
        Err(Error::message(format!(
            "MCP server '{}' does not support resource subscriptions",
            self.server_name
        )))
    }

    pub async fn subscribe_resource(&self, uri: &str) -> Result<()> {
        self.ensure_ready()?;
        self.ensure_subscribe()?;
        self.transport
            .request("resources/subscribe", Some(json!({ "uri": uri })))
            .await?;
        Ok(())
    }

    pub async fn unsubscribe_resource(&self, uri: &str) -> Result<()> {
        self.ensure_ready()?;
        self.ensure_subscribe()?;
        self.transport
            .request("resources/unsubscribe", Some(json!({ "uri": uri })))
            .await?;
        Ok(())
    }

    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        self.ensure_ready()?;
        if self.prompts_capability().is_none() {
            return Ok(Vec::new());
        }
        self.paginate("prompts/list", |page: PromptsListResult| {
            (page.prompts, page.next_cursor)
        })
        .await
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptsGetResult> {
        self.ensure_ready()?;
        let resp = self
            .transport
            .request(
                "prompts/get",
                Some(json!({ "name": name, "arguments": arguments })),
            )
            .await?;
        Ok(serde_json::from_value(
            resp.result.context("prompts/get returned no result")?,
        )?)
    }

    pub async fn notify_roots_changed(&self) -> Result<()> {
        if !self.transport.accepts_server_requests() {
            return Ok(());
        }
        self.transport
            .notify("notifications/roots/list_changed", None)
            .await
    }
}
