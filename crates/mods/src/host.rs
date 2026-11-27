//! wasmtime host for `moltis:mods/plugin` components.
//!
//! Each call gets a fresh store and instance (no state survives between calls
//! except through the granted KV store), a fuel budget, a memory limit and an
//! epoch deadline driven by one engine-wide ticker thread. Host imports check
//! the granted capabilities on every call.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use {
    moltis_tools::{
        wasm_component::{
            HttpError as ToolHttpError, HttpHostImpl, HttpRequest as ToolHttpRequest,
        },
        wasm_limits::WasmResourceLimiter,
    },
    sha2::{Digest, Sha256},
    tracing::{debug, error, info, trace, warn},
    wasmtime::component::{Component, HasSelf, Linker},
};

use crate::{
    capability::CapabilitySet,
    error::{Error, Result},
    id::ModId,
    metrics_support,
    state::write_atomic,
};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/mod",
        world: "plugin",
    });
}

use bindings::moltis::mods::{host as wit_host, http as wit_http, types as wit_types};

const MAX_LOG_LINE: usize = 2048;
const MAX_LOGS_PER_CALL: usize = 100;
const MAX_KV_KEY: usize = 256;

/// Limits applied to every call into a mod component.
#[derive(Debug, Clone)]
pub struct HostLimits {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub timeout: Duration,
    pub epoch_interval: Duration,
    pub max_output_bytes: usize,
    pub http_timeout: Duration,
    pub max_http_response_bytes: usize,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            fuel: 50_000_000,
            memory_bytes: 32 * 1024 * 1024,
            timeout: Duration::from_secs(10),
            epoch_interval: Duration::from_millis(50),
            max_output_bytes: 1024 * 1024,
            http_timeout: Duration::from_secs(15),
            max_http_response_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Engine-wide epoch ticker. One thread for all mods; each store's deadline
/// is expressed in ticks of this clock.
struct EpochTicker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: wasmtime::Engine, interval: Duration) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("moltis-mods-epoch".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(interval);
                    engine.increment_epoch();
                }
            })
            .map_err(|error| Error::Wasm(format!("cannot start epoch ticker: {error}")))?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Shared wasmtime engine configured for fuel and epoch interruption.
pub struct ModEngine {
    engine: wasmtime::Engine,
    limits: HostLimits,
    _ticker: EpochTicker,
}

impl ModEngine {
    pub fn new(limits: HostLimits) -> Result<Arc<Self>> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = wasmtime::Engine::new(&config)
            .map_err(|error| Error::Wasm(format!("cannot create engine: {error}")))?;
        let interval = limits.epoch_interval.max(Duration::from_millis(1));
        let ticker = EpochTicker::start(engine.clone(), interval)?;
        Ok(Arc::new(Self {
            engine,
            limits,
            _ticker: ticker,
        }))
    }

    #[must_use]
    pub fn limits(&self) -> &HostLimits {
        &self.limits
    }

    fn deadline_ticks(&self) -> u64 {
        let interval = self.limits.epoch_interval.max(Duration::from_millis(1));
        let ticks = self.limits.timeout.as_nanos() / interval.as_nanos();
        u64::try_from(ticks).unwrap_or(u64::MAX).max(1) + 1
    }

    pub fn compile(&self, bytes: &[u8]) -> Result<Component> {
        Component::new(&self.engine, bytes)
            .map_err(|error| Error::Package(format!("invalid component: {error:#}")))
    }

    /// Static check: refuse a component that imports anything outside the mod
    /// world and WASI, or imports HTTP without any `net` capability.
    pub fn check_imports(&self, component: &Component, requested: &CapabilitySet) -> Result<()> {
        let component_type = component.component_type();
        for (name, _) in component_type.imports(&self.engine) {
            classify_import(name, requested)?;
        }
        Ok(())
    }
}

fn classify_import(name: &str, requested: &CapabilitySet) -> Result<()> {
    let interface = name.split('@').next().unwrap_or(name);
    match interface {
        "moltis:mods/types" | "moltis:mods/host" => Ok(()),
        "moltis:mods/http" if requested.net_hosts().is_empty() => Err(Error::Denied(
            "component imports moltis:mods/http but the manifest declares no `net` hosts".into(),
        )),
        "moltis:mods/http" => Ok(()),
        _ if interface.starts_with("wasi:sockets/") || interface.starts_with("wasi:http/") => {
            Err(Error::Denied(format!(
                "component imports `{name}`; network access goes through moltis:mods/http"
            )))
        },
        _ if interface.starts_with("wasi:") => Ok(()),
        _ => Err(Error::Denied(format!(
            "component imports `{name}`, which is not part of the mod API"
        ))),
    }
}

/// Per-mod key/value store persisted as JSON, bounded by the storage quota.
pub struct KvStore {
    path: PathBuf,
    data: Mutex<BTreeMap<String, String>>,
}

impl KvStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.data.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn used_bytes(data: &BTreeMap<String, String>) -> u64 {
        data.iter()
            .map(|(key, value)| (key.len() + value.len()) as u64)
            .sum()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.lock().get(key).cloned()
    }

    pub fn set(&self, key: &str, value: &str, quota: u64) -> std::result::Result<(), String> {
        if key.is_empty() || key.len() > MAX_KV_KEY {
            return Err(format!("keys must be 1-{MAX_KV_KEY} bytes"));
        }
        let mut data = self.lock();
        let previous = data.insert(key.to_string(), value.to_string());
        if Self::used_bytes(&data) > quota {
            match previous {
                Some(old) => data.insert(key.to_string(), old),
                None => data.remove(key),
            };
            return Err(format!("storage quota of {quota} bytes exceeded"));
        }
        self.persist(&data)
    }

    pub fn delete(&self, key: &str) -> std::result::Result<(), String> {
        let mut data = self.lock();
        if data.remove(key).is_some() {
            self.persist(&data)?;
        }
        Ok(())
    }

    fn persist(&self, data: &BTreeMap<String, String>) -> std::result::Result<(), String> {
        let bytes = serde_json::to_vec(data).map_err(|error| error.to_string())?;
        write_atomic(&self.path, &bytes).map_err(|error| error.to_string())
    }
}

/// Host-side state the component's imports run against.
struct HostState {
    mod_id: ModId,
    granted: CapabilitySet,
    kv: Arc<KvStore>,
    http: Option<HttpHostImpl>,
    logs: usize,
}

impl HostState {
    fn deny(&self, capability: &'static str) -> String {
        metrics_support::denial(&self.mod_id.to_string(), capability);
        warn!(mod_id = %self.mod_id, capability, "mod capability denied");
        format!("capability denied: {capability}")
    }

    fn storage_quota(&self) -> std::result::Result<u64, String> {
        self.granted
            .storage_quota()
            .ok_or_else(|| self.deny("storage"))
    }
}

impl wit_types::Host for HostState {}

impl wit_host::Host for HostState {
    fn log(&mut self, level: wit_types::LogLevel, message: String) {
        self.logs += 1;
        if self.logs > MAX_LOGS_PER_CALL {
            return;
        }
        let message: String = message.chars().take(MAX_LOG_LINE).collect();
        let mod_id = &self.mod_id;
        match level {
            wit_types::LogLevel::Trace => trace!(%mod_id, "{message}"),
            wit_types::LogLevel::Debug => debug!(%mod_id, "{message}"),
            wit_types::LogLevel::Info => info!(%mod_id, "{message}"),
            wit_types::LogLevel::Warn => warn!(%mod_id, "{message}"),
            wit_types::LogLevel::Error => error!(%mod_id, "{message}"),
        }
    }

    fn kv_get(&mut self, key: String) -> std::result::Result<Option<String>, String> {
        self.storage_quota()?;
        Ok(self.kv.get(&key))
    }

    fn kv_set(&mut self, key: String, value: String) -> std::result::Result<(), String> {
        let quota = self.storage_quota()?;
        self.kv.set(&key, &value, quota)
    }

    fn kv_delete(&mut self, key: String) -> std::result::Result<(), String> {
        self.storage_quota()?;
        self.kv.delete(&key)
    }
}

impl wit_http::Host for HostState {
    fn send(
        &mut self,
        request: wit_http::Request,
    ) -> std::result::Result<wit_http::Response, wit_http::Error> {
        let host = url_host(&request.url).map_err(wit_http::Error::InvalidUrl)?;
        let hosts = self.granted.net_hosts();
        if !hosts.iter().any(|allowed| allowed.matches(&host)) {
            return Err(wit_http::Error::Denied(format!(
                "{}: host `{host}` is not granted",
                self.deny("net")
            )));
        }
        let Some(http) = &self.http else {
            return Err(wit_http::Error::Denied(self.deny("net")));
        };
        let response = http
            .handle_request(ToolHttpRequest {
                method: request.method,
                url: request.url,
                headers: request.headers,
                body: request.body,
                timeout_ms: request.timeout_ms,
                max_response_bytes: request.max_response_bytes,
            })
            .map_err(map_http_error)?;
        Ok(wit_http::Response {
            status: response.status,
            headers: response.headers,
            body: response.body,
            content_type: response.content_type,
        })
    }
}

fn url_host(url: &str) -> std::result::Result<String, String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| "only http and https URLs are allowed".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("credentials in URLs are not allowed".into());
    }
    let host = if authority.starts_with('[') {
        return Err("IP literals are not allowed".into());
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err("URL has no host".into());
    }
    Ok(host.to_ascii_lowercase())
}

fn map_http_error(error: ToolHttpError) -> wit_http::Error {
    match error {
        ToolHttpError::InvalidUrl(message) => wit_http::Error::InvalidUrl(message),
        ToolHttpError::BlockedUrl(message) => wit_http::Error::BlockedUrl(message),
        ToolHttpError::Timeout(message) => wit_http::Error::Timeout(message),
        ToolHttpError::Network(message) => wit_http::Error::Network(message),
        ToolHttpError::Status(status) => wit_http::Error::Status(status),
        ToolHttpError::TooLarge(size) => wit_http::Error::TooLarge(size),
        ToolHttpError::Other(message) => wit_http::Error::Other(message),
    }
}

struct StoreState {
    limiter: WasmResourceLimiter,
    table: wasmtime_wasi::ResourceTable,
    wasi: wasmtime_wasi::WasiCtx,
    host: HostState,
}

impl wasmtime_wasi::WasiView for StoreState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// A compiled, import-checked mod component bound to its grant.
pub struct ModComponent {
    engine: Arc<ModEngine>,
    component: Component,
    linker: Linker<StoreState>,
    mod_id: ModId,
    granted: CapabilitySet,
    kv: Arc<KvStore>,
    http: Option<HttpHostImpl>,
    hash: [u8; 32],
}

impl ModComponent {
    /// Compile `bytes`, check imports against `requested` (the manifest's
    /// capabilities) and bind host imports to `granted`.
    pub fn new(
        engine: Arc<ModEngine>,
        mod_id: ModId,
        bytes: &[u8],
        requested: &CapabilitySet,
        granted: CapabilitySet,
        kv: Arc<KvStore>,
    ) -> Result<Self> {
        let component = engine.compile(bytes)?;
        engine.check_imports(&component, requested)?;

        let mut linker = Linker::new(&engine.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| Error::Wasm(format!("cannot link WASI: {error}")))?;
        bindings::Plugin::add_to_linker::<StoreState, HasSelf<HostState>>(
            &mut linker,
            |state: &mut StoreState| &mut state.host,
        )
        .map_err(|error| Error::Wasm(format!("cannot link mod host: {error}")))?;

        let hosts = granted.net_hosts();
        let http = if hosts.is_empty() {
            None
        } else {
            let allowlist = hosts.iter().map(|host| host.suffix().to_string()).collect();
            Some(
                HttpHostImpl::new(
                    engine.limits.http_timeout,
                    engine.limits.max_http_response_bytes,
                    Vec::new(),
                    Some(allowlist),
                    Default::default(),
                )
                .map_err(|error| Error::Wasm(format!("cannot create HTTP host: {error}")))?,
            )
        };

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&Sha256::digest(bytes));
        Ok(Self {
            engine,
            component,
            linker,
            mod_id,
            granted,
            kv,
            http,
            hash,
        })
    }

    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    #[must_use]
    pub fn mod_id(&self) -> &ModId {
        &self.mod_id
    }

    #[must_use]
    pub fn granted(&self) -> &CapabilitySet {
        &self.granted
    }

    fn instantiate(&self) -> Result<(wasmtime::Store<StoreState>, bindings::Plugin)> {
        let limits = &self.engine.limits;
        let state = StoreState {
            limiter: WasmResourceLimiter::new(limits.memory_bytes),
            table: wasmtime_wasi::ResourceTable::new(),
            wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
            host: HostState {
                mod_id: self.mod_id.clone(),
                granted: self.granted.clone(),
                kv: Arc::clone(&self.kv),
                http: self.http.clone(),
                logs: 0,
            },
        };
        let mut store = wasmtime::Store::new(&self.engine.engine, state);
        store.limiter(|state| &mut state.limiter);
        store
            .set_fuel(limits.fuel)
            .map_err(|error| Error::Wasm(error.to_string()))?;
        store.set_epoch_deadline(self.engine.deadline_ticks());
        let plugin = bindings::Plugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|error| describe_failure("instantiate", &error))?;
        Ok((store, plugin))
    }

    fn check_output(&self, text: &str) -> Result<()> {
        if text.len() > self.engine.limits.max_output_bytes {
            return Err(Error::Wasm(format!(
                "output of {} bytes exceeds the {} byte limit",
                text.len(),
                self.engine.limits.max_output_bytes
            )));
        }
        Ok(())
    }

    /// Call `tools.call`. Blocking: run it on a blocking thread.
    pub fn call_tool(
        &self,
        name: &str,
        params_json: &str,
    ) -> Result<std::result::Result<String, String>> {
        let started = Instant::now();
        let result = self.call_tool_inner(name, params_json);
        metrics_support::call(
            &self.mod_id.to_string(),
            "tool",
            started.elapsed().as_secs_f64(),
            !matches!(result, Ok(Ok(_))),
        );
        result
    }

    fn call_tool_inner(
        &self,
        name: &str,
        params_json: &str,
    ) -> Result<std::result::Result<String, String>> {
        let (mut store, plugin) = self.instantiate()?;
        let result = plugin
            .moltis_mods_tools()
            .call_call(&mut store, name, params_json)
            .map_err(|error| describe_failure("tools.call", &error))?;
        match &result {
            Ok(text) | Err(text) => self.check_output(text)?,
        }
        Ok(result)
    }

    /// Call `hooks.handle`. Blocking: run it on a blocking thread.
    pub fn handle_hook(
        &self,
        event: &str,
        payload_json: &str,
    ) -> Result<std::result::Result<Option<String>, String>> {
        let started = Instant::now();
        let result = self.handle_hook_inner(event, payload_json);
        metrics_support::call(
            &self.mod_id.to_string(),
            "hook",
            started.elapsed().as_secs_f64(),
            result.is_err(),
        );
        result
    }

    fn handle_hook_inner(
        &self,
        event: &str,
        payload_json: &str,
    ) -> Result<std::result::Result<Option<String>, String>> {
        let (mut store, plugin) = self.instantiate()?;
        let result = plugin
            .moltis_mods_hooks()
            .call_handle(&mut store, event, payload_json)
            .map_err(|error| describe_failure("hooks.handle", &error))?;
        match &result {
            Ok(Some(text)) | Err(text) => self.check_output(text)?,
            Ok(None) => {},
        }
        Ok(result)
    }
}

fn describe_failure(stage: &str, error: &wasmtime::Error) -> Error {
    let cause = match error.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => "fuel exhausted".to_string(),
        Some(wasmtime::Trap::Interrupt) => "time limit exceeded".to_string(),
        Some(trap) => format!("trap: {trap}"),
        None => format!("{error:#}"),
    };
    Error::Wasm(format!("{stage} failed: {cause}"))
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
