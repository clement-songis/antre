//! Gateway wiring of the design canvas: service construction, render-origin
//! listener, event bus bridge, share-store backend, hook and agent tools.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use {
    antre_canvas::{
        CanvasEvent, CanvasEventSink, CanvasService, CanvasServiceParts, CanvasSettings,
        CreatedShare, DesignId, ShareBackend, Visibility,
    },
    async_trait::async_trait,
    tokio::sync::OnceCell,
    tracing::{info, warn},
};

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    share_store::{ShareStore, ShareVisibility},
    state::GatewayState,
};

/// Prefix of the share-store `session_key` used for design shares, so they
/// never collide with (or render as) session shares.
pub const SHARE_KEY_PREFIX: &str = "canvas:";

/// Publishes canvas events on the WebSocket bus once the gateway state exists.
struct BroadcastSink {
    state: Arc<OnceCell<Arc<GatewayState>>>,
}

impl CanvasEventSink for BroadcastSink {
    fn emit(&self, event: CanvasEvent) {
        let Some(state) = self.state.get().cloned() else {
            return;
        };
        let payload = match serde_json::to_value(&event) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(%error, "failed to serialize canvas event");
                return;
            },
        };
        tokio::spawn(async move {
            broadcast(&state, event.name(), payload, BroadcastOpts::default()).await;
        });
    }
}

/// Design shares on top of the session share store.
pub struct ShareStoreBackend {
    store: Arc<ShareStore>,
}

impl ShareStoreBackend {
    #[must_use]
    pub fn new(store: Arc<ShareStore>) -> Self {
        Self { store }
    }
}

fn design_of(session_key: &str) -> Option<DesignId> {
    session_key.strip_prefix(SHARE_KEY_PREFIX)?.parse().ok()
}

#[async_trait]
impl ShareBackend for ShareStoreBackend {
    async fn publish(
        &self,
        design_id: &DesignId,
        visibility: Visibility,
        snapshot_json: String,
        version: u32,
    ) -> anyhow::Result<CreatedShare> {
        let visibility = match visibility {
            Visibility::Public => ShareVisibility::Public,
            Visibility::Private => ShareVisibility::Private,
        };
        let created = self
            .store
            .create_or_replace(
                &format!("{SHARE_KEY_PREFIX}{design_id}"),
                visibility,
                snapshot_json,
                version,
            )
            .await?;
        Ok(CreatedShare {
            share_id: created.share.id,
            access_key: created.access_key,
        })
    }

    async fn resolve(
        &self,
        share_id: &str,
        access_key: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let Some(share) = self.store.get_active_by_id(share_id).await? else {
            return Ok(None);
        };
        if design_of(&share.session_key).is_none() {
            return Ok(None);
        }
        let allowed = share.visibility == ShareVisibility::Public
            || access_key.is_some_and(|key| ShareStore::verify_access_key(&share, key));
        Ok(allowed.then_some(share.snapshot_json))
    }

    async fn unpublish(&self, share_id: &str) -> anyhow::Result<Option<DesignId>> {
        let Some(share) = self.store.get_by_id(share_id).await? else {
            return Ok(None);
        };
        let Some(design_id) = design_of(&share.session_key) else {
            return Ok(None);
        };
        self.store.revoke(share_id).await?;
        Ok(Some(design_id))
    }
}

fn bind_ip(bind: &str) -> IpAddr {
    match bind {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        other => other.parse().unwrap_or_else(|_| {
            warn!(bind = other, "invalid canvas bind address, using 127.0.0.1");
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        }),
    }
}

/// Address the render origin listens on.
#[must_use]
pub fn render_address(config: &antre_config::AntreConfig) -> SocketAddr {
    let bind = config.canvas.bind.as_deref().unwrap_or(&config.server.bind);
    SocketAddr::new(bind_ip(bind), config.canvas.port.unwrap_or(0))
}

/// Build the canvas service and start its render origin. `None` when the
/// canvas is disabled or fails to initialize (the rest of the gateway runs).
pub async fn init(
    config: &antre_config::AntreConfig,
    data_dir: &Path,
    db_pool: &sqlx::SqlitePool,
    share_store: &Arc<ShareStore>,
    deferred_state: &Arc<OnceCell<Arc<GatewayState>>>,
    browser_container_prefix: &str,
) -> Option<Arc<CanvasService>> {
    if !config.canvas.enabled {
        info!("canvas disabled by canvas.enabled=false");
        return None;
    }
    if let Err(error) = antre_canvas::run_migrations(db_pool).await {
        warn!(%error, "canvas migrations failed, canvas disabled");
        return None;
    }
    if config.canvas.seed_design_systems
        && let Err(error) = antre_canvas::design_systems::seed_starters(&data_dir.join("skills"))
    {
        warn!(%error, "failed to seed starter design systems");
    }
    let renderer = antre_canvas::BrowserRenderer::new(
        &config.tools.browser,
        format!("{browser_container_prefix}-canvas"),
    );
    let service = match CanvasService::new(CanvasServiceParts {
        data_dir: data_dir.to_path_buf(),
        pool: db_pool.clone(),
        settings: CanvasSettings::from(&config.canvas),
        renderer: Some(Arc::new(renderer)),
        shares: Some(Arc::new(ShareStoreBackend::new(Arc::clone(share_store)))),
    }) {
        Ok(service) => Arc::new(service),
        Err(error) => {
            warn!(%error, "canvas initialization failed, canvas disabled");
            return None;
        },
    };
    service.set_event_sink(Arc::new(BroadcastSink {
        state: Arc::clone(deferred_state),
    }));
    let address = render_address(config);
    if let Err(error) = antre_canvas::server::spawn(Arc::clone(&service), address).await {
        warn!(%address, %error, "canvas render origin failed to start; previews unavailable");
    }
    Some(service)
}

/// Built-in hooks contributed by the canvas.
#[must_use]
pub fn hooks(
    canvas: Option<&Arc<CanvasService>>,
) -> Vec<Arc<dyn antre_common::hooks::HookHandler>> {
    canvas
        .map(|service| {
            vec![
                Arc::new(antre_canvas::hook::DesignVersionHook::new(Arc::clone(
                    service,
                ))) as Arc<dyn antre_common::hooks::HookHandler>,
            ]
        })
        .unwrap_or_default()
}

/// Register `design_create`, `design_open` and `design_screenshot`.
pub fn register_tools(
    registry: &mut antre_agents::tool_registry::ToolRegistry,
    canvas: Option<&Arc<CanvasService>>,
) {
    let Some(service) = canvas else {
        return;
    };
    for tool in antre_canvas::tools::design_tools(service) {
        registry.register(tool);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    async fn share_store() -> Arc<ShareStore> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::run_migrations(&pool).await.unwrap();
        Arc::new(ShareStore::new(pool))
    }

    #[tokio::test]
    async fn share_backend_round_trip() {
        let store = share_store().await;
        let backend = ShareStoreBackend::new(Arc::clone(&store));
        let id = DesignId::generate();
        let private = backend
            .publish(&id, Visibility::Private, "{\"a\":1}".into(), 3)
            .await
            .unwrap();
        let key = private.access_key.clone().unwrap();
        assert!(
            backend
                .resolve(&private.share_id, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            backend
                .resolve(&private.share_id, Some("bad"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            backend
                .resolve(&private.share_id, Some(&key))
                .await
                .unwrap()
                .as_deref(),
            Some("{\"a\":1}")
        );

        let public = backend
            .publish(&id, Visibility::Public, "{}".into(), 4)
            .await
            .unwrap();
        assert!(public.access_key.is_none());
        assert!(
            backend
                .resolve(&private.share_id, Some(&key))
                .await
                .unwrap()
                .is_none(),
            "republishing revokes the previous link"
        );
        assert!(
            backend
                .resolve(&public.share_id, None)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(backend.unpublish(&public.share_id).await.unwrap(), Some(id));
        assert!(
            backend
                .resolve(&public.share_id, None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn session_shares_are_invisible_to_the_canvas() {
        let store = share_store().await;
        let session_share = store
            .create_or_replace("main", ShareVisibility::Public, "{}".into(), 1)
            .await
            .unwrap();
        let backend = ShareStoreBackend::new(store);
        assert!(
            backend
                .resolve(&session_share.share.id, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            backend
                .unpublish(&session_share.share.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn init_starts_render_origin_and_seeds_design_systems() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::run_migrations(&pool).await.unwrap();
        let shares = Arc::new(ShareStore::new(pool.clone()));
        let state = Arc::new(OnceCell::new());
        let mut config = antre_config::AntreConfig::default();
        config.server.bind = "127.0.0.1".into();

        config.canvas.enabled = false;
        assert!(
            init(&config, tmp.path(), &pool, &shares, &state, "test")
                .await
                .is_none()
        );

        config.canvas.enabled = true;
        let service = init(&config, tmp.path(), &pool, &shares, &state, "test")
            .await
            .unwrap();
        let port = service.render_port().unwrap();
        assert_ne!(port, 0);
        assert!(
            tmp.path()
                .join("skills/design-system-minimal/tokens.css")
                .exists()
        );
        let info = service.info(Some("localhost:1234")).await;
        assert_eq!(info.origin, Some(format!("http://localhost:{port}")));
        assert_eq!(hooks(Some(&service)).len(), 1);
        assert!(hooks(None).is_empty());

        let mut registry = antre_agents::tool_registry::ToolRegistry::new();
        register_tools(&mut registry, Some(&service));
        for name in ["design_create", "design_open", "design_screenshot"] {
            assert!(registry.get(name).is_some(), "{name} registered");
        }
    }

    #[test]
    fn render_address_defaults_to_server_bind_and_free_port() {
        let mut config = antre_config::AntreConfig::default();
        config.server.bind = "127.0.0.1".into();
        assert_eq!(render_address(&config), "127.0.0.1:0".parse().unwrap());
        config.canvas.bind = Some("localhost".into());
        config.canvas.port = Some(18790);
        assert_eq!(render_address(&config), "127.0.0.1:18790".parse().unwrap());
        config.canvas.bind = Some("::".into());
        assert_eq!(render_address(&config), "[::]:18790".parse().unwrap());
        config.canvas.bind = Some("not an ip".into());
        assert_eq!(
            render_address(&config).ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn design_of_requires_prefix_and_valid_id() {
        let id = DesignId::generate();
        assert_eq!(design_of(&format!("canvas:{id}")), Some(id));
        assert_eq!(design_of("main"), None);
        assert_eq!(design_of("canvas:../x"), None);
    }
}
