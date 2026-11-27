//! Roles: features ask for "the image model of OpenAI" instead of naming an
//! id that retires a few months later.

use serde::Deserialize;

use crate::{
    catalog::{ModelCatalog, ResolvedModel},
    types::ModelKind,
};

const RECOMMENDED: &str = include_str!("../data/recommended.toml");

/// A curated, ordered list of picks for one provider and kind.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoleEntry {
    pub provider: String,
    pub kind: ModelKind,
    pub models: Vec<String>,
}

/// The curated picks (`data/recommended.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RoleTable {
    #[serde(default, rename = "role")]
    pub entries: Vec<RoleEntry>,
}

impl RoleTable {
    pub fn parse(text: &str) -> crate::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// The table compiled into the binary.
    pub fn bundled() -> crate::Result<Self> {
        Self::parse(RECOMMENDED)
    }

    /// Curated ids for a provider and kind, best first.
    #[must_use]
    pub fn recommended(&self, provider: &str, kind: ModelKind) -> &[String] {
        self.entries
            .iter()
            .find(|entry| entry.provider == provider && entry.kind == kind)
            .map(|entry| entry.models.as_slice())
            .unwrap_or_default()
    }
}

/// Resolve a role: the first curated pick the catalog knows as that kind and
/// not deprecated, else the newest active model of that kind.
#[must_use]
pub fn pick(
    catalog: &ModelCatalog,
    roles: &RoleTable,
    provider: &str,
    kind: ModelKind,
) -> Option<ResolvedModel> {
    let curated = roles.recommended(provider, kind).iter().find_map(|id| {
        let model = catalog.resolve(provider, id, None);
        (model.kind() == kind && !model.status().is_deprecated()).then_some(model)
    });
    curated.or_else(|| {
        catalog
            .models_of_kind(provider, kind)
            .into_iter()
            .find(|model| !model.status().is_deprecated())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        crate::{
            catalog::Source,
            types::{ModelSpec, ModelStatus},
        },
    };

    #[test]
    fn recommended_ids_exist_and_are_active() {
        let catalog = ModelCatalog::bundled().unwrap();
        let roles = RoleTable::bundled().unwrap();
        assert!(!roles.entries.is_empty());
        for entry in &roles.entries {
            assert!(
                !entry.models.is_empty(),
                "{} {:?}",
                entry.provider,
                entry.kind
            );
            for id in &entry.models {
                let model = catalog.resolve(&entry.provider, id, None);
                assert!(
                    catalog.known_ids(&entry.provider).contains(id),
                    "recommended {}::{id} is not in the bundled catalog",
                    entry.provider
                );
                assert_eq!(
                    model.kind(),
                    entry.kind,
                    "recommended {}::{id} has the wrong kind",
                    entry.provider
                );
                assert!(
                    !model.status().is_deprecated(),
                    "recommended {}::{id} is deprecated in the bundled catalog",
                    entry.provider
                );
            }
        }
    }

    #[test]
    fn pick_prefers_curated_then_newest_active() {
        let catalog = ModelCatalog::bundled().unwrap();
        let roles = RoleTable::bundled().unwrap();
        let image = pick(&catalog, &roles, "openai", ModelKind::Image).unwrap();
        assert_eq!(image.id(), "gpt-image-2.6-flare");
        let tts = pick(&catalog, &roles, "openai", ModelKind::Tts).unwrap();
        assert_eq!(tts.id(), "gpt-4o-mini-tts");
        assert!(pick(&catalog, &roles, "custom-none", ModelKind::Image).is_none());
    }

    #[test]
    fn pick_skips_curated_ids_deprecated_at_runtime() {
        let mut catalog = ModelCatalog::bundled().unwrap();
        let roles = RoleTable::parse(
            r#"
            [[role]]
            provider = "openai"
            kind = "image"
            models = ["gpt-image-2.6-flare", "gpt-image-2.6-sunburst"]
            "#,
        )
        .unwrap();
        catalog.set_provider_rows(Source::Live, "openai", vec![ModelSpec {
            status: Some(ModelStatus::Deprecated),
            ..ModelSpec::new("gpt-image-2.6-flare")
        }]);
        let image = pick(&catalog, &roles, "openai", ModelKind::Image).unwrap();
        assert_eq!(image.id(), "gpt-image-2.6-sunburst");
    }
}
