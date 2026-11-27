//! Process-wide catalog. Features that used to hard-code model ids, the
//! provider registry and the gateway's refresh task all read the same one.

use std::sync::{OnceLock, RwLock};

use crate::{
    catalog::{ModelCatalog, ResolvedModel},
    macros::log_warn,
    roles::{RoleTable, pick},
    types::ModelKind,
};

static CATALOG: OnceLock<RwLock<ModelCatalog>> = OnceLock::new();
static ROLES: OnceLock<RoleTable> = OnceLock::new();

fn catalog() -> &'static RwLock<ModelCatalog> {
    CATALOG.get_or_init(|| {
        let catalog = ModelCatalog::bundled().unwrap_or_else(|error| {
            log_warn!(%error, "bundled model catalog failed to load; using an empty catalog");
            ModelCatalog::empty()
        });
        RwLock::new(catalog)
    })
}

/// Read the global catalog.
pub fn read<R>(f: impl FnOnce(&ModelCatalog) -> R) -> R {
    let guard = catalog().read().unwrap_or_else(|e| e.into_inner());
    f(&guard)
}

/// Mutate the global catalog.
pub fn write<R>(f: impl FnOnce(&mut ModelCatalog) -> R) -> R {
    let mut guard = catalog().write().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// The curated role table compiled into the binary.
pub fn roles() -> &'static RoleTable {
    ROLES.get_or_init(|| {
        RoleTable::bundled().unwrap_or_else(|error| {
            log_warn!(%error, "bundled role table failed to load");
            RoleTable::default()
        })
    })
}

/// Resolve a role against the global catalog.
#[must_use]
pub fn pick_role(provider: &str, kind: ModelKind) -> Option<ResolvedModel> {
    read(|catalog| pick(catalog, roles(), provider, kind))
}

/// Id of a role's pick, or `fallback` when the catalog has none.
#[must_use]
pub fn role_id_or(provider: &str, kind: ModelKind, fallback: &str) -> String {
    pick_role(provider, kind).map_or_else(|| fallback.to_string(), |model| model.spec.id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn global_catalog_resolves_roles() {
        assert_eq!(
            role_id_or("openai", ModelKind::Image, "x"),
            "gpt-image-2.6-flare"
        );
        assert_eq!(
            role_id_or("custom-none", ModelKind::Tts, "fallback"),
            "fallback"
        );
        assert!(read(|catalog| !catalog.known_ids("anthropic").is_empty()));
    }
}
