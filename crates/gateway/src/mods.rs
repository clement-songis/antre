//! Mod runtime wiring: the process-wide [`ModService`], tool and hook
//! syncing, and the operations behind `mods.*` RPC and `/api/mods`.
//!
//! The service is process-wide because hooks are built before the gateway
//! state exists; it is set once at startup.

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use {
    moltis_agents::tool_registry::ToolRegistry,
    moltis_mods::{
        Error as ModError, InstallMode, ModId,
        host::HostLimits,
        service::{ModService, ModsSettings},
    },
    serde_json::Value,
    tokio::sync::RwLock,
    tracing::{info, warn},
};

use crate::state::GatewayState;

static SERVICE: OnceLock<Arc<ModService>> = OnceLock::new();
static TOOLS: OnceLock<Arc<RwLock<ToolRegistry>>> = OnceLock::new();

/// Build runtime settings from `[mods]`.
#[must_use]
pub fn settings_from_config(
    config: &moltis_config::schema::ModsConfig,
    dir: PathBuf,
) -> ModsSettings {
    let defaults = HostLimits::default();
    ModsSettings {
        limits: HostLimits {
            fuel: config.fuel,
            memory_bytes: usize::try_from(config.memory_mb)
                .unwrap_or(usize::MAX)
                .saturating_mul(1024 * 1024),
            timeout: Duration::from_millis(config.timeout_ms),
            epoch_interval: Duration::from_millis(config.epoch_interval_ms.max(1)),
            max_output_bytes: usize::try_from(config.max_output_kb)
                .unwrap_or(usize::MAX)
                .saturating_mul(1024),
            ..defaults
        },
        pin_shell_hooks: config.pin_shell_hooks,
        ..ModsSettings::new(dir)
    }
}

/// Open the runtime at startup. Returns `None` when `[mods] enabled = false`
/// or the runtime cannot start (logged; the gateway keeps running).
pub async fn init(config: &moltis_config::schema::ModsConfig) -> Option<Arc<ModService>> {
    if !config.enabled {
        info!("mods disabled by config");
        return None;
    }
    if let Some(existing) = SERVICE.get() {
        return Some(Arc::clone(existing));
    }
    let settings = settings_from_config(config, moltis_config::data_dir().join("mods"));
    match tokio::task::spawn_blocking(move || ModService::open(settings)).await {
        Ok(Ok(service)) => {
            let _ = SERVICE.set(Arc::clone(&service));
            info!(installed = service.list().len(), "mods runtime started");
            Some(service)
        },
        Ok(Err(error)) => {
            warn!(%error, "mods runtime failed to start");
            None
        },
        Err(error) => {
            warn!(%error, "mods runtime task failed");
            None
        },
    }
}

#[must_use]
pub fn service() -> Option<Arc<ModService>> {
    SERVICE.get().cloned()
}

/// Remember the shared tool registry and add the loaded mods' tools to it.
pub async fn attach_tool_registry(registry: Arc<RwLock<ToolRegistry>>) {
    let _ = TOOLS.set(registry);
    sync_tools().await;
}

pub async fn sync_tools() {
    let (Some(service), Some(tools)) = (service(), TOOLS.get()) else {
        return;
    };
    service.sync_tools(&mut *tools.write().await);
}

/// Error from a mods operation, with an API-friendly classification.
#[derive(Debug)]
pub struct ModsApiError {
    pub kind: ModsErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModsErrorKind {
    BadRequest,
    NotFound,
    Forbidden,
    Conflict,
    Unavailable,
    Internal,
}

impl ModsApiError {
    fn new(kind: ModsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Protocol error code for RPC responses.
    #[must_use]
    pub fn code(&self) -> &'static str {
        use moltis_protocol::error_codes as codes;
        match self.kind {
            ModsErrorKind::BadRequest => codes::INVALID_REQUEST,
            ModsErrorKind::NotFound => codes::NOT_FOUND,
            ModsErrorKind::Forbidden => codes::FORBIDDEN,
            ModsErrorKind::Conflict => codes::CONFLICT,
            ModsErrorKind::Unavailable => codes::UNAVAILABLE,
            ModsErrorKind::Internal => codes::INTERNAL,
        }
    }
}

impl From<ModError> for ModsApiError {
    fn from(error: ModError) -> Self {
        let kind = match &error {
            ModError::InvalidId { .. }
            | ModError::Manifest(_)
            | ModError::ManifestToml(_)
            | ModError::Package(_) => ModsErrorKind::BadRequest,
            ModError::NotInstalled(_) => ModsErrorKind::NotFound,
            ModError::Denied(_) => ModsErrorKind::Forbidden,
            ModError::ConsentRequired { .. }
            | ModError::ConsentMismatch { .. }
            | ModError::DuplicateContribution(_) => ModsErrorKind::Conflict,
            ModError::Wasm(_) | ModError::Io(_) | ModError::Json(_) => ModsErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}

impl std::fmt::Display for ModsApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

type ApiResult = Result<Value, ModsApiError>;

fn require_service() -> Result<Arc<ModService>, ModsApiError> {
    service().ok_or_else(|| {
        ModsApiError::new(
            ModsErrorKind::Unavailable,
            "mods are disabled ([mods] enabled = false) or failed to start",
        )
    })
}

pub fn parse_id(raw: &str) -> Result<ModId, ModsApiError> {
    // Host-owned ids (`local.*`) are valid targets for management calls.
    serde_json::from_value(Value::String(raw.to_string()))
        .map_err(|error| ModsApiError::new(ModsErrorKind::BadRequest, error.to_string()))
}

async fn blocking<T, F>(work: F) -> Result<T, ModsApiError>
where
    T: Send + 'static,
    F: FnOnce(Arc<ModService>) -> Result<T, ModError> + Send + 'static,
{
    let service = require_service()?;
    tokio::task::spawn_blocking(move || work(service))
        .await
        .map_err(|error| ModsApiError::new(ModsErrorKind::Internal, error.to_string()))?
        .map_err(ModsApiError::from)
}

fn to_json<T: serde::Serialize>(value: T) -> ApiResult {
    serde_json::to_value(value)
        .map_err(|error| ModsApiError::new(ModsErrorKind::Internal, error.to_string()))
}

/// Re-sync tools and rebuild the hook registry after a change.
async fn after_change(state: &Arc<GatewayState>) {
    sync_tools().await;
    crate::methods::reload_hooks(state).await;
}

pub async fn list() -> ApiResult {
    let mods = blocking(|service| Ok(service.list())).await?;
    let contributions = blocking(|service| Ok(service.contributions())).await?;
    to_json(serde_json::json!({ "mods": mods, "contributions": contributions }))
}

pub async fn get(id: &str) -> ApiResult {
    let id = parse_id(id)?;
    to_json(blocking(move |service| service.get(&id)).await?)
}

/// Install or update from a local folder on the gateway host.
pub async fn install(state: &Arc<GatewayState>, path: &str) -> ApiResult {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(ModsApiError::new(
            ModsErrorKind::BadRequest,
            "path must be absolute",
        ));
    }
    let outcome = blocking(move |service| service.install_local(&path)).await?;
    after_change(state).await;
    to_json(outcome)
}

/// Approve a capability set. RPC and REST callers are operators acting in
/// the UI or CLI, so they count as interactive.
pub async fn grant(
    state: &Arc<GatewayState>,
    id: &str,
    consent_hash: &str,
    enable: bool,
) -> ApiResult {
    let id = parse_id(id)?;
    let hash = consent_hash.to_string();
    let info = blocking(move |service| {
        let info = service.grant(&id, &hash, InstallMode::Interactive)?;
        if enable {
            service.enable(&id)
        } else {
            Ok(info)
        }
    })
    .await?;
    after_change(state).await;
    to_json(info)
}

pub async fn revoke(state: &Arc<GatewayState>, id: &str) -> ApiResult {
    let id = parse_id(id)?;
    let info = blocking(move |service| service.revoke(&id)).await?;
    after_change(state).await;
    to_json(info)
}

pub async fn enable(state: &Arc<GatewayState>, id: &str) -> ApiResult {
    let id = parse_id(id)?;
    let info = blocking(move |service| service.enable(&id)).await?;
    after_change(state).await;
    to_json(info)
}

pub async fn disable(state: &Arc<GatewayState>, id: &str) -> ApiResult {
    let id = parse_id(id)?;
    let info = blocking(move |service| service.disable(&id)).await?;
    after_change(state).await;
    to_json(info)
}

pub async fn uninstall(state: &Arc<GatewayState>, id: &str) -> ApiResult {
    let id = parse_id(id)?;
    blocking(move |service| service.uninstall(&id)).await?;
    after_change(state).await;
    to_json(serde_json::json!({ "ok": true }))
}

/// Approve a filesystem shell hook's current content after an operator
/// enabled or edited it.
pub async fn approve_shell_hook(name: &str, dir: PathBuf) -> Result<(), ModsApiError> {
    let name = name.to_string();
    blocking(move |service| service.approve_shell_hook(&name, &dir, InstallMode::Interactive)).await
}

/// Shell hooks held back by content pinning, as `name → reason`. Empty when
/// the runtime is off.
pub async fn shell_hook_approvals(
    hooks: Vec<(String, PathBuf)>,
) -> std::collections::HashMap<String, String> {
    use moltis_mods::hook_pins::{ApprovalReason, HookAdmission};
    let Some(service) = service() else {
        return Default::default();
    };
    let admissions =
        match tokio::task::spawn_blocking(move || service.admit_shell_hooks(&hooks)).await {
            Ok(admissions) => admissions,
            Err(error) => {
                warn!(%error, "shell hook admission task failed");
                return Default::default();
            },
        };
    admissions
        .into_iter()
        .filter_map(|(name, admission)| match admission {
            HookAdmission::Allowed => None,
            HookAdmission::NeedsApproval { reason } => {
                let reason = match reason {
                    ApprovalReason::New => "new",
                    ApprovalReason::Changed => "changed",
                    ApprovalReason::Unreadable => "unreadable",
                };
                warn!(hook = %name, reason, "shell hook held back until approved");
                Some((name, reason.to_string()))
            },
        })
        .collect()
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_config() {
        let config = moltis_config::schema::ModsConfig {
            fuel: 7,
            memory_mb: 3,
            timeout_ms: 250,
            epoch_interval_ms: 0,
            max_output_kb: 2,
            pin_shell_hooks: false,
            ..Default::default()
        };
        let settings = settings_from_config(&config, PathBuf::from("/x"));
        assert_eq!(settings.limits.fuel, 7);
        assert_eq!(settings.limits.memory_bytes, 3 * 1024 * 1024);
        assert_eq!(settings.limits.timeout, Duration::from_millis(250));
        assert_eq!(settings.limits.epoch_interval, Duration::from_millis(1));
        assert_eq!(settings.limits.max_output_bytes, 2048);
        assert!(!settings.pin_shell_hooks);
        assert_eq!(settings.dir, PathBuf::from("/x"));
    }

    #[test]
    fn errors_map_to_protocol_codes() {
        let id = ModId::parse("acme.x").unwrap();
        let cases = [
            (ModError::NotInstalled(id.clone()), "NOT_FOUND"),
            (ModError::Denied("x".into()), "FORBIDDEN"),
            (ModError::Package("x".into()), "INVALID_REQUEST"),
            (
                ModError::ConsentRequired {
                    mod_id: id,
                    consent_hash: "h".into(),
                },
                "CONFLICT",
            ),
            (ModError::Wasm("x".into()), "INTERNAL"),
        ];
        for (error, code) in cases {
            assert_eq!(ModsApiError::from(error).code(), code);
        }
    }

    #[test]
    fn ids_parse_including_host_owned() {
        assert!(parse_id("acme.demo").is_ok());
        assert!(parse_id("local.hook-guard").is_ok());
        assert!(parse_id("../../etc").is_err());
    }

    #[test]
    fn mutating_methods_are_admin_only() {
        use crate::methods::authorize_method;
        let scopes = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let read = scopes(&["operator.read"]);
        let write = scopes(&["operator.read", "operator.write"]);
        let admin = scopes(&["operator.admin"]);
        assert!(authorize_method("mods.list", "operator", &read).is_none());
        assert!(authorize_method("mods.get", "operator", &read).is_none());
        for method in [
            "mods.install",
            "mods.grant",
            "mods.enable",
            "mods.disable",
            "mods.revoke",
            "mods.uninstall",
        ] {
            assert!(
                authorize_method(method, "operator", &write).is_some(),
                "{method}"
            );
            assert!(
                authorize_method(method, "operator", &admin).is_none(),
                "{method}"
            );
            assert!(
                authorize_method(method, "node", &admin).is_some(),
                "{method}"
            );
        }
    }

    #[tokio::test]
    async fn operations_report_unavailable_without_runtime() {
        if service().is_some() {
            return;
        }
        let err = list().await.unwrap_err();
        assert_eq!(err.kind, ModsErrorKind::Unavailable);
    }
}
