//! The mods runtime: install, consent, enable/disable, and loading enabled
//! mods into the contribution registry.
//!
//! All methods do blocking file and wasmtime work; call them from a blocking
//! thread (`tokio::task::spawn_blocking`).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use {
    moltis_agents::tool_registry::ToolRegistry,
    moltis_common::hooks::HookHandler,
    serde::Serialize,
    time::OffsetDateTime,
    tracing::{info, warn},
};

use crate::{
    adapters::{ModHook, ModTool, SharedHook, SharedTool},
    capability::{CapabilitySet, ConsentHash, Risk},
    error::{Error, Result},
    grant::{ConsentState, Grant, GrantOrigin, InstallMode},
    hook_pins::{self, HookAdmission},
    host::{HostLimits, KvStore, ModComponent, ModEngine},
    id::{ContributionId, LocalId, ModId, Owner},
    manifest::{MANIFEST_FILE, ModManifest},
    metrics_support,
    package::{PackageLimits, stage_from_dir},
    registry::{Contribution, ContributionInfo, ContributionRegistry, Registration},
    state::{InstalledMod, ModSource, ModsState, PendingUpdate, StateFile},
};

/// Runtime settings, built from `[mods]` in the config.
#[derive(Debug, Clone)]
pub struct ModsSettings {
    /// Root directory (`<data_dir>/mods`).
    pub dir: PathBuf,
    pub limits: HostLimits,
    pub package_limits: PackageLimits,
    /// Pin filesystem shell hooks by content (see [`hook_pins`]).
    pub pin_shell_hooks: bool,
}

impl ModsSettings {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            limits: HostLimits::default(),
            package_limits: PackageLimits::default(),
            pin_shell_hooks: true,
        }
    }
}

/// One capability as shown to a person deciding on consent.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityView {
    pub capability: String,
    pub risk: Risk,
    pub description: String,
}

fn view(capabilities: &CapabilitySet) -> Vec<CapabilityView> {
    capabilities
        .iter()
        .map(|capability| CapabilityView {
            capability: capability.to_string(),
            risk: capability.risk(),
            description: capability.describe(),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpdateView {
    pub version: String,
    pub consent_hash: ConsentHash,
    pub capabilities: Vec<CapabilityView>,
    pub missing: Vec<CapabilityView>,
}

/// API view of an installed mod.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    pub id: ModId,
    pub version: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub loaded: bool,
    pub source: ModSource,
    pub content_hash: String,
    pub capabilities: Vec<CapabilityView>,
    pub consent: ConsentState,
    pub granted_by: Option<GrantOrigin>,
    pub pending_update: Option<PendingUpdateView>,
    pub contributions: Vec<String>,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

/// What an install did.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InstallOutcome {
    /// New mod, installed disabled until consent and enable.
    Installed { id: ModId, consent: ConsentState },
    /// Same content already installed.
    Unchanged { id: ModId },
    /// Update applied: it fits within the existing grant.
    Updated { id: ModId, version: String },
    /// Update staged: it widens capabilities and waits for consent while the
    /// current version keeps running.
    UpdatePending { id: ModId, consent: ConsentState },
}

struct LoadedMod {
    _registrations: Vec<Registration>,
}

/// The mods runtime.
pub struct ModService {
    settings: ModsSettings,
    state_file: StateFile,
    state: Mutex<ModsState>,
    engine: Arc<ModEngine>,
    registry: ContributionRegistry,
    loaded: Mutex<BTreeMap<ModId, LoadedMod>>,
    errors: Mutex<BTreeMap<ModId, String>>,
    core: Mutex<Vec<Registration>>,
    synced_tools: Mutex<BTreeSet<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

impl ModService {
    /// Open the runtime and load every enabled, granted mod.
    pub fn open(settings: ModsSettings) -> Result<Arc<Self>> {
        fs::create_dir_all(settings.dir.join("store"))?;
        let state_file = StateFile::new(settings.dir.join("state.json"));
        let state = state_file.load()?;
        let engine = ModEngine::new(settings.limits.clone())?;
        let service = Arc::new(Self {
            settings,
            state_file,
            state: Mutex::new(state),
            engine,
            registry: ContributionRegistry::new(),
            loaded: Mutex::new(BTreeMap::new()),
            errors: Mutex::new(BTreeMap::new()),
            core: Mutex::new(Vec::new()),
            synced_tools: Mutex::new(BTreeSet::new()),
        });
        service.load_all_enabled();
        Ok(service)
    }

    #[must_use]
    pub fn registry(&self) -> &ContributionRegistry {
        &self.registry
    }

    fn store_dir(&self, content_hash: &str) -> PathBuf {
        self.settings.dir.join("store").join(content_hash)
    }

    fn kv_path(&self, id: &ModId) -> PathBuf {
        self.settings.dir.join("kv").join(format!("{id}.json"))
    }

    fn read_manifest(&self, content_hash: &str) -> Result<ModManifest> {
        let text = fs::read_to_string(self.store_dir(content_hash).join(MANIFEST_FILE))?;
        ModManifest::parse(&text)
    }

    fn save(&self, state: &ModsState) -> Result<()> {
        self.state_file.save(state)?;
        metrics_support::installed(state.mods.len());
        Ok(())
    }

    fn load_all_enabled(&self) {
        let enabled: Vec<ModId> = lock(&self.state)
            .mods
            .iter()
            .filter(|(_, installed)| installed.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        for id in enabled {
            if let Err(error) = self.load(&id) {
                warn!(mod_id = %id, %error, "failed to load mod");
                lock(&self.errors).insert(id, error.to_string());
            }
        }
    }

    // ── Listing ─────────────────────────────────────────────────────────

    #[must_use]
    pub fn list(&self) -> Vec<ModInfo> {
        let state = lock(&self.state).clone();
        state
            .mods
            .iter()
            .map(|(id, installed)| self.info(id, installed))
            .collect()
    }

    pub fn get(&self, id: &ModId) -> Result<ModInfo> {
        let state = lock(&self.state);
        let installed = state
            .mods
            .get(id)
            .ok_or_else(|| Error::NotInstalled(id.clone()))?
            .clone();
        drop(state);
        Ok(self.info(id, &installed))
    }

    fn info(&self, id: &ModId, installed: &InstalledMod) -> ModInfo {
        let manifest = self.read_manifest(&installed.content_hash);
        let owner = Owner::Mod(id.clone());
        let contributions: Vec<String> = self
            .registry
            .ids_for(&owner)
            .iter()
            .map(ToString::to_string)
            .collect();
        let pending_update = installed.pending_update.as_ref().and_then(|pending| {
            let manifest = self.read_manifest(&pending.content_hash).ok()?;
            let requested = manifest.capabilities();
            let missing: CapabilitySet = match &installed.grant {
                Some(grant) => requested
                    .missing_from(&grant.capabilities)
                    .into_iter()
                    .collect(),
                None => requested.clone(),
            };
            Some(PendingUpdateView {
                version: pending.version.clone(),
                consent_hash: pending.consent_hash.clone(),
                capabilities: view(&requested),
                missing: view(&missing),
            })
        });
        let error = lock(&self.errors).get(id).cloned();
        match manifest {
            Ok(manifest) => {
                let requested = manifest.capabilities();
                ModInfo {
                    id: id.clone(),
                    version: installed.version.clone(),
                    name: manifest.name.clone(),
                    description: manifest.description.clone(),
                    enabled: installed.enabled,
                    loaded: lock(&self.loaded).contains_key(id),
                    source: installed.source.clone(),
                    content_hash: installed.content_hash.clone(),
                    capabilities: view(&requested),
                    consent: ConsentState::evaluate(&requested, installed.grant.as_ref()),
                    granted_by: installed.grant.as_ref().map(|grant| grant.origin),
                    pending_update,
                    contributions,
                    tools: manifest
                        .tools
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect(),
                    error,
                }
            },
            Err(manifest_error) => ModInfo {
                id: id.clone(),
                version: installed.version.clone(),
                name: id.to_string(),
                description: String::new(),
                enabled: installed.enabled,
                loaded: false,
                source: installed.source.clone(),
                content_hash: installed.content_hash.clone(),
                capabilities: Vec::new(),
                consent: ConsentState::evaluate(&CapabilitySet::new(), None),
                granted_by: None,
                pending_update,
                contributions,
                tools: Vec::new(),
                error: Some(manifest_error.to_string()),
            },
        }
    }

    // ── Install / update / uninstall ────────────────────────────────────

    /// Install or update from a local folder. Never grants anything.
    #[tracing::instrument(skip(self), fields(path = %path.display()))]
    pub fn install_local(&self, path: &Path) -> Result<InstallOutcome> {
        let result = self.install_local_inner(path);
        metrics_support::install_outcome(match &result {
            Ok(InstallOutcome::Installed { .. }) => "installed",
            Ok(InstallOutcome::Unchanged { .. }) => "unchanged",
            Ok(InstallOutcome::Updated { .. }) => "updated",
            Ok(InstallOutcome::UpdatePending { .. }) => "update_pending",
            Err(_) => "rejected",
        });
        result
    }

    fn install_local_inner(&self, path: &Path) -> Result<InstallOutcome> {
        let source_path = fs::canonicalize(path)?;
        if source_path.starts_with(fs::canonicalize(&self.settings.dir)?) {
            return Err(Error::Package(
                "cannot install from inside the mods directory".into(),
            ));
        }
        let staged = stage_from_dir(
            &source_path,
            &self.settings.dir.join("staging"),
            self.settings.package_limits,
        )?;
        let requested = staged.manifest.capabilities();
        if let Some(bytes) = &staged.component_bytes {
            let component = match self.engine.compile(bytes) {
                Ok(component) => component,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staged.dir);
                    return Err(error);
                },
            };
            if let Err(error) = self.engine.check_imports(&component, &requested) {
                let _ = fs::remove_dir_all(&staged.dir);
                return Err(error);
            }
        }

        let target = self.store_dir(&staged.content_hash);
        if target.exists() {
            fs::remove_dir_all(&staged.dir)?;
        } else {
            fs::rename(&staged.dir, &target)?;
        }

        let id = staged.manifest.id.clone();
        let now = OffsetDateTime::now_utc();
        let source = ModSource::LocalPath {
            path: source_path.display().to_string(),
        };
        let mut state = lock(&self.state);
        let outcome = match state.mods.get_mut(&id) {
            None => {
                state.mods.insert(id.clone(), InstalledMod {
                    version: staged.manifest.version.clone(),
                    content_hash: staged.content_hash.clone(),
                    source,
                    enabled: false,
                    grant: None,
                    pending_update: None,
                    installed_at: now,
                    updated_at: now,
                });
                info!(mod_id = %id, "mod installed (disabled until consent)");
                InstallOutcome::Installed {
                    id: id.clone(),
                    consent: ConsentState::evaluate(&requested, None),
                }
            },
            Some(installed) if installed.content_hash == staged.content_hash => {
                installed.pending_update = None;
                InstallOutcome::Unchanged { id: id.clone() }
            },
            Some(installed) => {
                let consent = ConsentState::evaluate(&requested, installed.grant.as_ref());
                installed.source = source;
                if consent.is_granted() {
                    installed.version = staged.manifest.version.clone();
                    installed.content_hash = staged.content_hash.clone();
                    installed.pending_update = None;
                    installed.updated_at = now;
                    InstallOutcome::Updated {
                        id: id.clone(),
                        version: staged.manifest.version.clone(),
                    }
                } else {
                    installed.pending_update = Some(PendingUpdate {
                        version: staged.manifest.version.clone(),
                        content_hash: staged.content_hash.clone(),
                        consent_hash: requested.consent_hash(),
                    });
                    info!(mod_id = %id, "update widens capabilities; waiting for consent");
                    InstallOutcome::UpdatePending {
                        id: id.clone(),
                        consent,
                    }
                }
            },
        };
        let reload = matches!(outcome, InstallOutcome::Updated { .. })
            && state
                .mods
                .get(&id)
                .is_some_and(|installed| installed.enabled);
        self.save(&state)?;
        drop(state);
        if reload {
            self.unload(&id);
            self.load_recording_error(&id);
        }
        Ok(outcome)
    }

    /// Remove a mod, its store copy (when unshared) and its storage.
    pub fn uninstall(&self, id: &ModId) -> Result<()> {
        self.unload(id);
        let mut state = lock(&self.state);
        let removed = state
            .mods
            .remove(id)
            .ok_or_else(|| Error::NotInstalled(id.clone()))?;
        let mut hashes = vec![removed.content_hash];
        if let Some(pending) = removed.pending_update {
            hashes.push(pending.content_hash);
        }
        let still_used: BTreeSet<&str> = state
            .mods
            .values()
            .flat_map(|installed| {
                std::iter::once(installed.content_hash.as_str()).chain(
                    installed
                        .pending_update
                        .as_ref()
                        .map(|pending| pending.content_hash.as_str()),
                )
            })
            .collect();
        for hash in hashes
            .iter()
            .filter(|hash| !still_used.contains(hash.as_str()))
        {
            let _ = fs::remove_dir_all(self.store_dir(hash));
        }
        self.save(&state)?;
        drop(state);
        let _ = fs::remove_file(self.kv_path(id));
        lock(&self.errors).remove(id);
        info!(mod_id = %id, "mod uninstalled");
        Ok(())
    }

    // ── Consent ─────────────────────────────────────────────────────────

    /// Approve the capability set identified by `consent_hash`: either the
    /// installed version's, or a pending update's (which is then applied).
    /// Non-interactive callers can never grant.
    pub fn grant(&self, id: &ModId, consent_hash: &str, mode: InstallMode) -> Result<ModInfo> {
        if mode == InstallMode::NonInteractive {
            return Err(Error::Denied(
                "non-interactive callers cannot grant capabilities".into(),
            ));
        }
        let mut state = lock(&self.state);
        let installed = state
            .mods
            .get(id)
            .ok_or_else(|| Error::NotInstalled(id.clone()))?
            .clone();
        let (content_hash, version, applying_update) = match &installed.pending_update {
            Some(pending) if pending.consent_hash.as_str() == consent_hash => {
                (pending.content_hash.clone(), pending.version.clone(), true)
            },
            _ => (
                installed.content_hash.clone(),
                installed.version.clone(),
                false,
            ),
        };
        let requested = self.read_manifest(&content_hash)?.capabilities();
        let expected = requested.consent_hash();
        if expected.as_str() != consent_hash {
            return Err(Error::ConsentMismatch {
                mod_id: id.clone(),
                expected: expected.to_string(),
                actual: consent_hash.to_string(),
            });
        }
        let Some(entry) = state.mods.get_mut(id) else {
            return Err(Error::NotInstalled(id.clone()));
        };
        entry.grant = Some(Grant::new(requested, GrantOrigin::Interactive));
        if applying_update {
            entry.content_hash = content_hash;
            entry.version = version;
            entry.pending_update = None;
            entry.updated_at = OffsetDateTime::now_utc();
        }
        let reload = applying_update && entry.enabled;
        self.save(&state)?;
        drop(state);
        if reload {
            self.unload(id);
            self.load_recording_error(id);
        }
        info!(mod_id = %id, applying_update, "mod capabilities granted");
        self.get(id)
    }

    /// Withdraw consent: the mod is disabled and unloaded immediately.
    pub fn revoke(&self, id: &ModId) -> Result<ModInfo> {
        self.unload(id);
        let mut state = lock(&self.state);
        let entry = state
            .mods
            .get_mut(id)
            .ok_or_else(|| Error::NotInstalled(id.clone()))?;
        entry.grant = None;
        entry.enabled = false;
        self.save(&state)?;
        drop(state);
        self.get(id)
    }

    // ── Enable / disable ────────────────────────────────────────────────

    pub fn enable(&self, id: &ModId) -> Result<ModInfo> {
        {
            let mut state = lock(&self.state);
            let entry = state
                .mods
                .get(id)
                .ok_or_else(|| Error::NotInstalled(id.clone()))?;
            let requested = self.read_manifest(&entry.content_hash)?.capabilities();
            if let ConsentState::Pending { consent_hash, .. } =
                ConsentState::evaluate(&requested, entry.grant.as_ref())
            {
                return Err(Error::ConsentRequired {
                    mod_id: id.clone(),
                    consent_hash: consent_hash.to_string(),
                });
            }
            if let Some(entry) = state.mods.get_mut(id) {
                entry.enabled = true;
            }
            self.save(&state)?;
        }
        self.unload(id);
        if let Err(error) = self.load(id) {
            lock(&self.errors).insert(id.clone(), error.to_string());
            let mut state = lock(&self.state);
            if let Some(entry) = state.mods.get_mut(id) {
                entry.enabled = false;
            }
            self.save(&state)?;
            return Err(error);
        }
        self.get(id)
    }

    pub fn disable(&self, id: &ModId) -> Result<ModInfo> {
        self.unload(id);
        let mut state = lock(&self.state);
        let entry = state
            .mods
            .get_mut(id)
            .ok_or_else(|| Error::NotInstalled(id.clone()))?;
        entry.enabled = false;
        self.save(&state)?;
        drop(state);
        self.get(id)
    }

    // ── Loading ─────────────────────────────────────────────────────────

    fn load_recording_error(&self, id: &ModId) {
        match self.load(id) {
            Ok(()) => {
                lock(&self.errors).remove(id);
            },
            Err(error) => {
                warn!(mod_id = %id, %error, "failed to reload mod");
                lock(&self.errors).insert(id.clone(), error.to_string());
            },
        }
    }

    fn load(&self, id: &ModId) -> Result<()> {
        let installed = lock(&self.state)
            .mods
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotInstalled(id.clone()))?;
        let manifest = self.read_manifest(&installed.content_hash)?;
        if &manifest.id != id {
            return Err(Error::Package("stored manifest id does not match".into()));
        }
        let requested = manifest.capabilities();
        if !ConsentState::evaluate(&requested, installed.grant.as_ref()).is_granted() {
            return Err(Error::ConsentRequired {
                mod_id: id.clone(),
                consent_hash: requested.consent_hash().to_string(),
            });
        }

        let mut registrations = Vec::new();
        if let Some(relative) = &manifest.component {
            let bytes = fs::read(self.store_dir(&installed.content_hash).join(relative))?;
            let kv = Arc::new(KvStore::open(self.kv_path(id))?);
            // Effective capabilities are what the manifest asks for, which the
            // grant was just checked to cover; a wider old grant adds nothing.
            let component = Arc::new(ModComponent::new(
                Arc::clone(&self.engine),
                id.clone(),
                &bytes,
                &requested,
                requested.clone(),
                kv,
            )?);
            let owner = Owner::Mod(id.clone());
            for tool in &manifest.tools {
                let local = LocalId::parse(&format!("tool.{}", tool.name))?;
                let tool_impl = Arc::new(ModTool::new(tool.clone(), Arc::clone(&component)));
                registrations.push(self.registry.register(
                    ContributionId::new(owner.clone(), local),
                    Contribution::Tool {
                        tool: tool_impl,
                        component_hash: Some(component.hash()),
                    },
                )?);
            }
            for hook in &manifest.hooks {
                let local = LocalId::parse(&format!("hook.{}", hook.id))?;
                let handler = Arc::new(ModHook::new(
                    format!("{id}/{}", hook.id),
                    hook.events.clone(),
                    hook.mode,
                    hook.priority,
                    Arc::clone(&component),
                ));
                registrations.push(self.registry.register(
                    ContributionId::new(owner.clone(), local),
                    Contribution::Hook(handler),
                )?);
            }
        }
        lock(&self.loaded).insert(id.clone(), LoadedMod {
            _registrations: registrations,
        });
        lock(&self.errors).remove(id);
        info!(mod_id = %id, version = %installed.version, "mod loaded");
        Ok(())
    }

    fn unload(&self, id: &ModId) {
        if lock(&self.loaded).remove(id).is_some() {
            info!(mod_id = %id, "mod unloaded");
        }
    }

    // ── Core contributions and consumers ────────────────────────────────

    /// Replace the core hook contributions (`core/<local>`). Called each time
    /// the hook registry is rebuilt so config changes take effect.
    pub fn replace_core_hooks(&self, hooks: Vec<(&str, Arc<dyn HookHandler>)>) -> Result<()> {
        let mut core = lock(&self.core);
        core.clear();
        for (local, handler) in hooks {
            core.push(
                self.registry
                    .register(ContributionId::core(local)?, Contribution::Hook(handler))?,
            );
        }
        Ok(())
    }

    /// All hook handlers (core and mods), ready for a `HookRegistry`.
    #[must_use]
    pub fn hook_handlers(&self) -> Vec<Arc<dyn HookHandler>> {
        self.registry
            .hooks()
            .into_iter()
            .map(|(_, handler)| Arc::new(SharedHook(handler)) as Arc<dyn HookHandler>)
            .collect()
    }

    #[must_use]
    pub fn contributions(&self) -> Vec<ContributionInfo> {
        self.registry.list()
    }

    /// Make the tool registry match the loaded mods' tools.
    pub fn sync_tools(&self, tools: &mut ToolRegistry) {
        let mut synced = lock(&self.synced_tools);
        for mod_id in synced.iter() {
            tools.unregister_mod(mod_id);
        }
        synced.clear();
        for (id, tool, component_hash) in self.registry.tools() {
            let Owner::Mod(mod_id) = id.owner() else {
                continue;
            };
            let mod_id = mod_id.to_string();
            tools.register_mod(
                Box::new(SharedTool(tool)),
                mod_id.clone(),
                component_hash.unwrap_or_default(),
            );
            synced.insert(mod_id);
        }
    }

    // ── Shell hook pins (migration of `crates/plugins` hooks) ──────────

    /// Decide which filesystem shell hooks may run. The first call pins every
    /// existing hook as migrated; later calls require approval for new or
    /// changed hooks.
    pub fn admit_shell_hooks(
        &self,
        hooks: &[(String, PathBuf)],
    ) -> BTreeMap<String, HookAdmission> {
        if !self.settings.pin_shell_hooks {
            return hooks
                .iter()
                .map(|(name, _)| (name.clone(), HookAdmission::Allowed))
                .collect();
        }
        let mut state = lock(&self.state);
        let migrating = !state.hooks_migrated;
        let before = state.hook_pins.clone();
        let admissions = hooks
            .iter()
            .map(|(name, dir)| {
                (
                    name.clone(),
                    hook_pins::admit(&mut state, name, dir, migrating),
                )
            })
            .collect();
        state.hooks_migrated = true;
        if (migrating || state.hook_pins != before)
            && let Err(error) = self.save(&state)
        {
            warn!(%error, "failed to persist shell hook pins");
        }
        admissions
    }

    /// Approve a shell hook's current content after a person reviewed it.
    pub fn approve_shell_hook(&self, name: &str, dir: &Path, mode: InstallMode) -> Result<()> {
        if mode == InstallMode::NonInteractive {
            return Err(Error::Denied(
                "non-interactive callers cannot approve shell hooks".into(),
            ));
        }
        let mut state = lock(&self.state);
        hook_pins::approve(&mut state, name, dir)?;
        self.save(&state)
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
