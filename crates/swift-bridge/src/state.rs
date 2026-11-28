//! Global bridge state: tokio runtime, provider registry, session stores, and HTTP server handle.

use std::{
    net::SocketAddr,
    sync::{Arc, LazyLock, Mutex, RwLock},
};

use {
    antre_providers::ProviderRegistry,
    antre_sessions::{
        metadata::SqliteSessionMetadata, session_events::SessionEventBus, store::SessionStore,
    },
};

use crate::callbacks::emit_log;

// ── Global bridge state ──────────────────────────────────────────────────

pub(crate) struct BridgeState {
    pub runtime: tokio::runtime::Runtime,
    pub registry: RwLock<ProviderRegistry>,
    pub session_store: SessionStore,
    pub session_metadata: SqliteSessionMetadata,
    pub credential_store: Arc<antre_gateway::auth::CredentialStore>,
    pub sandbox_default_image_override: RwLock<Option<String>>,
}

impl BridgeState {
    fn new() -> Self {
        #[cfg(test)]
        init_swift_bridge_test_dirs();

        // Initialize config directory (write defaults, compact, persist port).
        antre_config::initialize_config();

        emit_log(
            "INFO",
            "bridge",
            "Initializing Rust bridge (tokio runtime + registry)",
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("failed to create tokio runtime: {e}"));

        let registry = build_registry();

        // Initialize persistent session storage (JSONL message files).
        let data_dir = antre_config::data_dir();
        let sessions_dir = data_dir.join("sessions");
        if let Err(e) = std::fs::create_dir_all(&sessions_dir) {
            emit_log(
                "ERROR",
                "bridge",
                &format!("Failed to create sessions dir: {e}"),
            );
        }
        let session_store = SessionStore::new(sessions_dir);

        // Open the shared SQLite database (same antre.db used by the gateway).
        // WAL mode + synchronous=NORMAL avoids multi-second write contention.
        let db_path = data_dir.join("antre.db");
        let db_pool = runtime.block_on(async {
            use {
                sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
                std::str::FromStr,
            };
            let opts = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path.display()))
                .expect("invalid antre.db path")
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal);
            let pool = sqlx::SqlitePool::connect_with(opts)
                .await
                .unwrap_or_else(|e| panic!("failed to open antre.db: {e}"));
            // Run migrations so the sessions table exists even if the gateway
            // hasn't been started yet. Order: projects first (FK dependency).
            if let Err(e) = antre_projects::run_migrations(&pool).await {
                emit_log("WARN", "bridge", &format!("projects migration: {e}"));
            }
            if let Err(e) = antre_sessions::run_migrations(&pool).await {
                emit_log("WARN", "bridge", &format!("sessions migration: {e}"));
            }
            if let Err(e) = antre_gateway::run_migrations(&pool).await {
                emit_log("WARN", "bridge", &format!("gateway migration: {e}"));
            }
            pool
        });
        let event_bus = SessionEventBus::new();
        let session_metadata = SqliteSessionMetadata::with_event_bus(db_pool.clone(), event_bus);
        let credential_store = runtime.block_on(async {
            // Keep vault metadata up to date so env var encryption status works
            // even when the full gateway server is not running.
            if let Err(e) = antre_gateway::auth::antre_vault::run_migrations(&db_pool).await {
                emit_log("WARN", "bridge", &format!("vault migration: {e}"));
            }

            let vault = match antre_gateway::auth::antre_vault::Vault::new(db_pool.clone()).await {
                Ok(vault) => Some(Arc::new(vault)),
                Err(e) => {
                    emit_log("WARN", "bridge", &format!("vault init failed: {e}"));
                    None
                },
            };

            match antre_gateway::auth::CredentialStore::with_vault(
                db_pool.clone(),
                &antre_config::discover_and_load().auth,
                vault,
            )
            .await
            {
                Ok(store) => Arc::new(store),
                Err(e) => panic!("failed to init credential store: {e}"),
            }
        });

        emit_log("INFO", "bridge", "Bridge initialized successfully");
        Self {
            runtime,
            registry: RwLock::new(registry),
            session_store,
            session_metadata,
            credential_store,
            sandbox_default_image_override: RwLock::new(None),
        }
    }
}

#[cfg(test)]
fn init_swift_bridge_test_dirs() {
    static TEST_DIRS_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    TEST_DIRS_INIT.get_or_init(|| {
        let base = std::env::temp_dir().join(format!(
            "antre-swift-bridge-tests-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let config_dir = base.join("config");
        let data_dir = base.join("data");

        if let Err(error) = std::fs::create_dir_all(&config_dir) {
            panic!("failed to create swift-bridge test config dir: {error}");
        }
        if let Err(error) = std::fs::create_dir_all(&data_dir) {
            panic!("failed to create swift-bridge test data dir: {error}");
        }

        antre_config::set_config_dir(config_dir);
        antre_config::set_data_dir(data_dir);
    });
}

pub(crate) fn build_registry() -> ProviderRegistry {
    let config = antre_config::discover_and_load();
    let env_overrides = config.env.clone();
    let key_store = antre_provider_setup::KeyStore::new();
    let effective =
        antre_provider_setup::config_with_saved_keys(&config.providers, &key_store, &[]);
    #[cfg(test)]
    {
        ProviderRegistry::from_config_with_static_catalogs(
            &effective,
            &env_overrides,
            std::collections::HashMap::new(),
        )
    }
    #[cfg(not(test))]
    {
        ProviderRegistry::from_env_with_config_and_overrides(
            &effective,
            &env_overrides,
            antre_providers::extract_cw_overrides(&config.models),
        )
    }
}

pub(crate) static BRIDGE: LazyLock<BridgeState> = LazyLock::new(BridgeState::new);

// ── HTTP Server handle ───────────────────────────────────────────────────

/// Handle to a running httpd server, used to shut it down.
pub(crate) struct HttpdHandle {
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
    pub server_task: tokio::task::JoinHandle<()>,
    pub addr: SocketAddr,
    /// Gateway state -- used for abort/peek FFI calls and kept alive while
    /// the server is running.
    pub state: std::sync::Arc<antre_gateway::state::GatewayState>,
}

/// Global server handle -- `None` when stopped, `Some` when running.
pub(crate) static HTTPD: Mutex<Option<HttpdHandle>> = Mutex::new(None);

pub(crate) fn stop_httpd_handle(handle: HttpdHandle, log_target: &str, stop_message: &str) {
    emit_log("INFO", log_target, stop_message);
    let HttpdHandle {
        shutdown_tx,
        server_task,
        state,
        ..
    } = handle;
    let _ = shutdown_tx.send(());
    BRIDGE.runtime.block_on(async {
        if let Err(error) = server_task.await {
            emit_log(
                "WARN",
                log_target,
                &format!("httpd task join failed during shutdown: {error}"),
            );
        }
        state
            .instrumentation
            .shutdown(std::time::Duration::from_secs(5))
            .await;
    });
}
