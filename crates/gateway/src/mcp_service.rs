//! Live MCP service implementation backed by `McpManager`.

use std::{collections::HashMap, sync::Arc};

use {
    async_trait::async_trait,
    serde_json::Value,
    tokio::sync::RwLock,
    tracing::{info, warn},
};

use antre_agents::tool_registry::ToolRegistry;

use crate::services::{McpService, ServiceError, ServiceResult};

// Re-export pure parsing functions that now live in antre-mcp.
pub(crate) use antre_mcp::{merge_env_overrides, parse_server_config};

// Re-export sync_mcp_tools from the dedicated bridge crate.
pub(crate) use antre_mcp_agent_bridge::sync_mcp_tools;

// ── Config parsing helper ───────────────────────────────────────────────────

// Extract an `McpServerConfig` from JSON params.
// ── LiveMcpService ──────────────────────────────────────────────────────────

/// Live MCP service delegating to `McpManager`.
pub struct LiveMcpService {
    manager: Arc<antre_mcp::McpManager>,
    /// Shared tool registry for syncing MCP tools into the agent loop.
    /// Set after construction via `set_tool_registry`.
    tool_registry: RwLock<Option<Arc<RwLock<ToolRegistry>>>>,
    config_env_overrides: HashMap<String, String>,
    credential_store: RwLock<Option<Arc<crate::auth::CredentialStore>>>,
    elicitations: Arc<antre_mcp::PendingElicitations>,
}

impl LiveMcpService {
    pub fn new(
        manager: Arc<antre_mcp::McpManager>,
        config_env_overrides: HashMap<String, String>,
        credential_store: Option<Arc<crate::auth::CredentialStore>>,
    ) -> Self {
        Self {
            manager,
            tool_registry: RwLock::new(None),
            config_env_overrides,
            credential_store: RwLock::new(credential_store),
            elicitations: Arc::new(antre_mcp::PendingElicitations::new(
                std::time::Duration::from_secs(300),
            )),
        }
    }

    /// Use a different elicitation timeout (`mcp.elicitation_timeout_secs`).
    #[must_use]
    pub fn with_elicitation_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.elicitations = Arc::new(antre_mcp::PendingElicitations::new(timeout));
        self
    }

    /// Pending elicitations, shared with the MCP host.
    pub fn elicitations(&self) -> Arc<antre_mcp::PendingElicitations> {
        Arc::clone(&self.elicitations)
    }

    /// Store a reference to the shared tool registry so MCP mutations
    /// can automatically sync tools.
    pub async fn set_tool_registry(&self, registry: Arc<RwLock<ToolRegistry>>) {
        *self.tool_registry.write().await = Some(registry);
    }

    /// Sync MCP tools into the shared tool registry (if set).
    pub async fn sync_tools_if_ready(&self) {
        let maybe_reg = self.tool_registry.read().await.clone();
        if let Some(reg) = maybe_reg {
            sync_mcp_tools(&self.manager, &reg).await;
        }
    }

    /// Access the underlying manager.
    pub fn manager(&self) -> &Arc<antre_mcp::McpManager> {
        &self.manager
    }

    pub async fn set_credential_store(&self, credential_store: Arc<crate::auth::CredentialStore>) {
        *self.credential_store.write().await = Some(credential_store);
    }

    async fn refresh_manager_env_overrides(&self) {
        let credential_store = self.credential_store.read().await.clone();
        let env_overrides = if let Some(store) = credential_store {
            match store.get_all_env_values().await {
                Ok(db_env_vars) => merge_env_overrides(&self.config_env_overrides, db_env_vars),
                Err(error) => {
                    warn!(%error, "failed to refresh MCP env overrides from credential store");
                    self.config_env_overrides.clone()
                },
            }
        } else {
            self.config_env_overrides.clone()
        };

        self.manager.set_env_overrides(env_overrides).await;
    }
}

#[async_trait]
impl McpService for LiveMcpService {
    async fn list(&self) -> ServiceResult {
        let statuses = self.manager.status_all().await;
        Ok(serde_json::to_value(&statuses)?)
    }

    async fn add(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let redirect_uri = params
            .get("redirectUri")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned);
        let config =
            parse_server_config(&params, None).map_err(|e| ServiceError::message(e.to_string()))?;
        self.refresh_manager_env_overrides().await;

        // If a server with this name already exists, append a numeric suffix.
        let final_name = {
            let reg = self.manager.registry_snapshot().await;
            let mut candidate = name.to_string();
            let mut n = 2u32;
            while reg.servers.contains_key(&candidate) {
                candidate = format!("{name}-{n}");
                n += 1;
            }
            candidate
        };

        info!(server = %final_name, "adding MCP server via API");
        match self
            .manager
            .add_server(final_name.clone(), config, true)
            .await
        {
            Ok(_) => {
                self.sync_tools_if_ready().await;
                Ok(serde_json::json!({ "ok": true, "name": final_name }))
            },
            Err(e) => {
                if matches!(
                    e,
                    antre_mcp::Error::Manager(antre_mcp::McpManagerError::OAuthRequired { .. })
                ) {
                    if let Some(uri) = redirect_uri {
                        let auth_url = self
                            .manager
                            .oauth_start_server(&final_name, &uri)
                            .await
                            .map_err(ServiceError::message)?;
                        Ok(serde_json::json!({
                            "ok": true,
                            "name": final_name,
                            "oauthPending": true,
                            "authUrl": auth_url
                        }))
                    } else {
                        Ok(serde_json::json!({
                            "ok": true,
                            "name": final_name,
                            "oauthPending": true
                        }))
                    }
                } else {
                    Err(ServiceError::message(e))
                }
            },
        }
    }

    async fn remove(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        let removed = self
            .manager
            .remove_server(name)
            .await
            .map_err(ServiceError::message)?;

        self.sync_tools_if_ready().await;

        Ok(serde_json::json!({ "removed": removed }))
    }

    async fn enable(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let redirect_uri = params
            .get("redirectUri")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned);
        self.refresh_manager_env_overrides().await;

        match self.manager.enable_server(name).await {
            Ok(_) => {
                self.sync_tools_if_ready().await;
                Ok(serde_json::json!({ "enabled": true }))
            },
            Err(e) => {
                if matches!(
                    e,
                    antre_mcp::Error::Manager(antre_mcp::McpManagerError::OAuthRequired { .. })
                ) {
                    if let Some(uri) = redirect_uri {
                        let auth_url = self
                            .manager
                            .oauth_start_server(name, &uri)
                            .await
                            .map_err(ServiceError::message)?;
                        Ok(serde_json::json!({
                            "enabled": false,
                            "oauthPending": true,
                            "authUrl": auth_url
                        }))
                    } else {
                        Ok(serde_json::json!({
                            "enabled": false,
                            "oauthPending": true
                        }))
                    }
                } else {
                    Err(ServiceError::message(e))
                }
            },
        }
    }

    async fn disable(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        let ok = self
            .manager
            .disable_server(name)
            .await
            .map_err(ServiceError::message)?;

        self.sync_tools_if_ready().await;

        Ok(serde_json::json!({ "disabled": ok }))
    }

    async fn status(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        match self.manager.status(name).await {
            Some(s) => Ok(serde_json::to_value(&s)?),
            None => Err(format!("MCP server '{name}' not found").into()),
        }
    }

    async fn tools(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        match self.manager.server_tools(name).await {
            Some(tools) => Ok(serde_json::to_value(&tools)?),
            None => Err(format!("MCP server '{name}' not found or not running").into()),
        }
    }

    async fn restart(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        self.refresh_manager_env_overrides().await;

        self.manager
            .restart_server(name)
            .await
            .map_err(ServiceError::message)?;

        self.sync_tools_if_ready().await;

        Ok(serde_json::json!({ "ok": true }))
    }

    async fn update(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let existing = self
            .manager
            .registry_snapshot()
            .await
            .servers
            .get(name)
            .cloned()
            .ok_or_else(|| format!("MCP server '{name}' not found"))?;
        let config = parse_server_config(&params, Some(&existing))
            .map_err(|e| ServiceError::message(e.to_string()))?;
        self.refresh_manager_env_overrides().await;

        self.manager
            .update_server(name, config)
            .await
            .map_err(ServiceError::message)?;

        self.sync_tools_if_ready().await;

        Ok(serde_json::json!({ "ok": true }))
    }

    async fn reauth(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let redirect_uri = params
            .get("redirectUri")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "missing 'redirectUri' parameter".to_string())?;
        self.refresh_manager_env_overrides().await;

        let auth_url = self
            .manager
            .reauth_server(name, redirect_uri)
            .await
            .map_err(ServiceError::message)?;

        Ok(serde_json::json!({
            "ok": true,
            "oauthPending": true,
            "authUrl": auth_url
        }))
    }

    async fn oauth_start(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let redirect_uri = params
            .get("redirectUri")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "missing 'redirectUri' parameter".to_string())?;
        self.refresh_manager_env_overrides().await;

        let auth_url = self
            .manager
            .oauth_start_server(name, redirect_uri)
            .await
            .map_err(ServiceError::message)?;

        Ok(serde_json::json!({
            "ok": true,
            "oauthPending": true,
            "authUrl": auth_url
        }))
    }

    async fn oauth_complete(&self, params: Value) -> ServiceResult {
        let state = params
            .get("state")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'state' parameter".to_string())?;
        let code = params
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'code' parameter".to_string())?;
        self.refresh_manager_env_overrides().await;

        let server_name = self
            .manager
            .oauth_complete_callback(state, code)
            .await
            .map_err(ServiceError::message)?;

        self.sync_tools_if_ready().await;

        Ok(serde_json::json!({
            "ok": true,
            "name": server_name
        }))
    }

    async fn update_request_timeout(&self, request_timeout_secs: u64) -> ServiceResult {
        self.manager.set_request_timeout_secs(request_timeout_secs);
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn features(&self, _params: Value) -> ServiceResult {
        Ok(serde_json::json!({
            "servers": self.manager.server_summaries().await,
            "roots": self.manager.roots(),
        }))
    }

    async fn resources_list(&self, params: Value) -> ServiceResult {
        let Some(server) = params.get("server").and_then(Value::as_str) else {
            return Ok(serde_json::json!({ "servers": self.manager.list_all_resources().await }));
        };
        let (resources, templates) = tokio::join!(
            self.manager.list_resources(server),
            self.manager.list_resource_templates(server)
        );
        Ok(serde_json::json!({
            "server": server,
            "resources": resources.map_err(service_error)?,
            "templates": templates.map_err(service_error)?,
        }))
    }

    async fn resource_read(&self, params: Value) -> ServiceResult {
        let (server, uri) = server_and_uri(&params)?;
        let read = self
            .manager
            .read_resource(server, uri)
            .await
            .map_err(service_error)?;
        Ok(serde_json::to_value(read)?)
    }

    async fn resource_subscribe(&self, params: Value) -> ServiceResult {
        let (server, uri) = server_and_uri(&params)?;
        self.manager
            .subscribe_resource(server, uri)
            .await
            .map_err(service_error)?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn resource_unsubscribe(&self, params: Value) -> ServiceResult {
        let (server, uri) = server_and_uri(&params)?;
        self.manager
            .unsubscribe_resource(server, uri)
            .await
            .map_err(service_error)?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn prompts_list(&self, _params: Value) -> ServiceResult {
        Ok(serde_json::json!({ "commands": self.manager.prompt_commands().await }))
    }

    async fn prompt_get(&self, params: Value) -> ServiceResult {
        let server = required_str(&params, "server")?;
        let name = required_str(&params, "name")?;
        let arguments: HashMap<String, String> = match params.get("arguments") {
            Some(value) if !value.is_null() => serde_json::from_value(value.clone())
                .map_err(|_| "'arguments' must map names to strings".to_string())?,
            _ => HashMap::new(),
        };
        let prompt = self
            .manager
            .get_prompt(server, name, arguments)
            .await
            .map_err(service_error)?;
        Ok(serde_json::to_value(prompt)?)
    }

    async fn elicitation_list(&self, params: Value) -> ServiceResult {
        let session_key = params.get("sessionKey").and_then(Value::as_str);
        Ok(serde_json::json!({ "requests": self.elicitations.list(session_key).await }))
    }

    async fn elicitation_resolve(&self, params: Value) -> ServiceResult {
        let id = required_str(&params, "requestId")?;
        let result: antre_mcp::protocol::ElicitResult = serde_json::from_value(serde_json::json!({
            "action": params.get("action").cloned().unwrap_or(Value::Null),
            "content": params.get("content").cloned().filter(|v| !v.is_null()),
        }))
        .map_err(|_| "'action' must be accept, decline or cancel".to_string())?;
        self.elicitations
            .resolve(id, result)
            .await
            .map_err(ServiceError::from)?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn ui_resource_read(&self, params: Value) -> ServiceResult {
        let (server, uri) = server_and_uri(&params)?;
        let resource = self
            .manager
            .read_ui_resource(server, uri)
            .await
            .map_err(service_error)?;
        Ok(serde_json::to_value(resource)?)
    }
}

fn service_error(error: antre_mcp::Error) -> ServiceError {
    ServiceError::from(error.to_string())
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ServiceError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServiceError::from(format!("missing '{key}' parameter")))
}

fn server_and_uri(params: &Value) -> Result<(&str, &str), ServiceError> {
    Ok((
        required_str(params, "server")?,
        required_str(params, "uri")?,
    ))
}
