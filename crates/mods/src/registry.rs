//! Server-side contribution registry.
//!
//! Core features and mods register tools and hook handlers here under
//! namespaced ids. Every registration returns a [`Registration`] guard; dropping
//! it (or calling [`Registration::dispose`]) removes the contribution, so a mod
//! that is disabled or uninstalled leaves nothing behind.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use {
    moltis_agents::tool_registry::AgentTool, moltis_common::hooks::HookHandler, serde::Serialize,
};

use crate::{
    error::{Error, Result},
    id::{ContributionId, Owner},
};

/// What a contribution provides.
#[derive(Clone)]
pub enum Contribution {
    Hook(Arc<dyn HookHandler>),
    Tool {
        tool: Arc<dyn AgentTool>,
        /// SHA-256 of the component behind the tool, `None` for core tools.
        component_hash: Option<[u8; 32]>,
    },
}

impl Contribution {
    #[must_use]
    pub fn kind(&self) -> ContributionKind {
        match self {
            Self::Hook(_) => ContributionKind::Hook,
            Self::Tool { .. } => ContributionKind::Tool,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
    Hook,
    Tool,
}

/// A listing row, safe to serialize for the API.
#[derive(Debug, Clone, Serialize)]
pub struct ContributionInfo {
    pub id: ContributionId,
    pub kind: ContributionKind,
    pub name: String,
}

struct Entry {
    contribution: Contribution,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<ContributionId, Entry>,
    next_generation: u64,
}

/// Shared registry handle; cheap to clone.
#[derive(Clone, Default)]
pub struct ContributionRegistry {
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|error| error.into_inner())
}

impl ContributionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a contribution. Fails if the id is taken: a mod can never
    /// replace another owner's contribution by reusing its id.
    pub fn register(&self, id: ContributionId, contribution: Contribution) -> Result<Registration> {
        let mut inner = lock(&self.inner);
        if inner.entries.contains_key(&id) {
            return Err(Error::DuplicateContribution(id.to_string()));
        }
        inner.next_generation += 1;
        let generation = inner.next_generation;
        inner.entries.insert(id.clone(), Entry {
            contribution,
            generation,
        });
        Ok(Registration {
            registry: Arc::downgrade(&self.inner),
            id,
            generation,
        })
    }

    #[must_use]
    pub fn get(&self, id: &ContributionId) -> Option<Contribution> {
        lock(&self.inner)
            .entries
            .get(id)
            .map(|entry| entry.contribution.clone())
    }

    /// Hook handlers in id order (core first, then mods alphabetically).
    #[must_use]
    pub fn hooks(&self) -> Vec<(ContributionId, Arc<dyn HookHandler>)> {
        lock(&self.inner)
            .entries
            .iter()
            .filter_map(|(id, entry)| match &entry.contribution {
                Contribution::Hook(handler) => Some((id.clone(), Arc::clone(handler))),
                Contribution::Tool { .. } => None,
            })
            .collect()
    }

    /// Tools in id order.
    #[must_use]
    pub fn tools(&self) -> Vec<(ContributionId, Arc<dyn AgentTool>, Option<[u8; 32]>)> {
        lock(&self.inner)
            .entries
            .iter()
            .filter_map(|(id, entry)| match &entry.contribution {
                Contribution::Tool {
                    tool,
                    component_hash,
                } => Some((id.clone(), Arc::clone(tool), *component_hash)),
                Contribution::Hook(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn list(&self) -> Vec<ContributionInfo> {
        lock(&self.inner)
            .entries
            .iter()
            .map(|(id, entry)| ContributionInfo {
                id: id.clone(),
                kind: entry.contribution.kind(),
                name: match &entry.contribution {
                    Contribution::Hook(handler) => handler.name().to_string(),
                    Contribution::Tool { tool, .. } => tool.name().to_string(),
                },
            })
            .collect()
    }

    /// Ids currently registered by `owner`.
    #[must_use]
    pub fn ids_for(&self, owner: &Owner) -> Vec<ContributionId> {
        lock(&self.inner)
            .entries
            .keys()
            .filter(|id| id.owner() == owner)
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.inner).entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Disposer for one contribution. Dropping it unregisters the contribution
/// unless the id has since been re-registered by someone else.
#[must_use = "dropping a Registration immediately unregisters the contribution"]
pub struct Registration {
    registry: Weak<Mutex<Inner>>,
    id: ContributionId,
    generation: u64,
}

impl Registration {
    #[must_use]
    pub fn id(&self) -> &ContributionId {
        &self.id
    }

    /// Unregister now. Equivalent to dropping the guard.
    pub fn dispose(self) {
        drop(self);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let Some(inner) = self.registry.upgrade() else {
            return;
        };
        let mut inner = lock(&inner);
        if inner
            .entries
            .get(&self.id)
            .is_some_and(|entry| entry.generation == self.generation)
        {
            inner.entries.remove(&self.id);
        }
    }
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("id", &self.id.to_string())
            .finish()
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        async_trait::async_trait,
        moltis_common::hooks::{HookAction, HookEvent, HookPayload},
    };

    struct NamedHook(&'static str);

    #[async_trait]
    impl HookHandler for NamedHook {
        fn name(&self) -> &str {
            self.0
        }

        fn events(&self) -> &[HookEvent] {
            &[HookEvent::SessionStart]
        }

        async fn handle(
            &self,
            _event: HookEvent,
            _payload: &HookPayload,
        ) -> moltis_common::Result<HookAction> {
            Ok(HookAction::Continue)
        }
    }

    struct NamedTool(&'static str);

    #[async_trait]
    impl AgentTool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    fn hook(name: &'static str) -> Contribution {
        Contribution::Hook(Arc::new(NamedHook(name)))
    }

    #[test]
    fn register_and_dispose() {
        let registry = ContributionRegistry::new();
        let id = ContributionId::core("hook.logger").unwrap();
        let guard = registry.register(id.clone(), hook("logger")).unwrap();
        assert_eq!(registry.len(), 1);
        assert_eq!(guard.id(), &id);
        assert!(registry.get(&id).is_some());
        guard.dispose();
        assert!(registry.is_empty());
        assert!(registry.get(&id).is_none());
    }

    #[test]
    fn drop_unregisters() {
        let registry = ContributionRegistry::new();
        {
            let _guard = registry
                .register(ContributionId::core("a").unwrap(), hook("a"))
                .unwrap();
            assert_eq!(registry.len(), 1);
        }
        assert!(registry.is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let registry = ContributionRegistry::new();
        let id: ContributionId = "acme.x/hook".parse().unwrap();
        let _first = registry.register(id.clone(), hook("first")).unwrap();
        let err = registry.register(id, hook("second")).unwrap_err();
        assert!(matches!(err, Error::DuplicateContribution(_)));
        assert_eq!(registry.hooks()[0].1.name(), "first");
    }

    #[test]
    fn stale_guard_does_not_remove_newer_registration() {
        let registry = ContributionRegistry::new();
        let id = ContributionId::core("a").unwrap();
        let first = registry.register(id.clone(), hook("first")).unwrap();
        // Simulate a replace: unregister through a fresh registry handle path.
        drop(first);
        let second = registry.register(id.clone(), hook("second")).unwrap();
        let stale = Registration {
            registry: Arc::downgrade(&registry.inner),
            id: id.clone(),
            generation: 1,
        };
        drop(stale);
        assert_eq!(registry.hooks()[0].1.name(), "second");
        drop(second);
        assert!(registry.is_empty());
    }

    #[test]
    fn guards_outliving_registry_are_harmless() {
        let registry = ContributionRegistry::new();
        let guard = registry
            .register(ContributionId::core("a").unwrap(), hook("a"))
            .unwrap();
        drop(registry);
        drop(guard);
    }

    #[test]
    fn listing_and_owner_filter() {
        let registry = ContributionRegistry::new();
        let _core = registry
            .register(ContributionId::core("hook.a").unwrap(), hook("a"))
            .unwrap();
        let _tool = registry
            .register("acme.x/tool.echo".parse().unwrap(), Contribution::Tool {
                tool: Arc::new(NamedTool("echo")),
                component_hash: Some([7; 32]),
            })
            .unwrap();
        assert_eq!(registry.hooks().len(), 1);
        let tools = registry.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].2, Some([7; 32]));
        assert_eq!(registry.ids_for(&Owner::Core).len(), 1);
        let owner = Owner::Mod(crate::id::ModId::parse("acme.x").unwrap());
        assert_eq!(registry.ids_for(&owner).len(), 1);
        let listing = serde_json::to_value(registry.list()).unwrap();
        assert_eq!(listing[0]["id"], "core/hook.a");
        assert_eq!(listing[1]["id"], "acme.x/tool.echo");
        assert_eq!(listing[1]["kind"], "tool");
    }
}
