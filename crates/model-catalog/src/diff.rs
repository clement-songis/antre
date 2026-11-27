//! Summary of what changed between two models.dev snapshots, for the body of
//! the scheduled snapshot-refresh pull request.

use std::collections::{BTreeMap, BTreeSet};

use crate::{models_dev::ModelsDevCatalog, types::ModelSpec};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CatalogDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub deprecated: Vec<String>,
    /// `provider/model: old -> new` context window changes.
    pub context_changed: Vec<String>,
}

fn index(catalog: &ModelsDevCatalog) -> BTreeMap<String, &ModelSpec> {
    catalog
        .providers
        .iter()
        .flat_map(|(provider, models)| {
            models
                .iter()
                .map(move |model| (format!("{provider}/{}", model.id), model))
        })
        .collect()
}

impl CatalogDiff {
    #[must_use]
    pub fn between(old: &ModelsDevCatalog, new: &ModelsDevCatalog) -> Self {
        let old_index = index(old);
        let new_index = index(new);
        let old_keys: BTreeSet<&String> = old_index.keys().collect();
        let new_keys: BTreeSet<&String> = new_index.keys().collect();
        let mut diff = Self {
            added: new_keys
                .difference(&old_keys)
                .map(|k| (*k).clone())
                .collect(),
            removed: old_keys
                .difference(&new_keys)
                .map(|k| (*k).clone())
                .collect(),
            ..Self::default()
        };
        for key in new_keys.intersection(&old_keys) {
            let (before, after) = (old_index[*key], new_index[*key]);
            if after.status_or_default().is_deprecated()
                && !before.status_or_default().is_deprecated()
            {
                diff.deprecated.push((*key).clone());
            }
            if before.context_window != after.context_window {
                let show =
                    |value: Option<u32>| value.map_or_else(|| "?".to_string(), |v| v.to_string());
                diff.context_changed.push(format!(
                    "{key}: {} -> {}",
                    show(before.context_window),
                    show(after.context_window)
                ));
            }
        }
        diff
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.deprecated.is_empty()
            && self.context_changed.is_empty()
    }

    /// Markdown with counts and up to `limit` items per section.
    #[must_use]
    pub fn to_markdown(&self, limit: usize) -> String {
        let mut out = String::from("## Model catalog changes\n");
        for (title, items) in [
            ("Added", &self.added),
            ("Removed", &self.removed),
            ("Newly deprecated", &self.deprecated),
            ("Context window changed", &self.context_changed),
        ] {
            out.push_str(&format!("\n### {title} ({})\n\n", items.len()));
            for item in items.iter().take(limit) {
                out.push_str(&format!("- `{item}`\n"));
            }
            if items.len() > limit {
                out.push_str(&format!("- … and {} more\n", items.len() - limit));
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, crate::types::ModelStatus};

    fn catalog(models: Vec<ModelSpec>) -> ModelsDevCatalog {
        let mut catalog = ModelsDevCatalog::default();
        catalog.providers.insert("openai".into(), models);
        catalog
    }

    #[test]
    fn reports_added_removed_deprecated_and_context() {
        let old = catalog(vec![
            ModelSpec::new("a"),
            ModelSpec {
                context_window: Some(100),
                ..ModelSpec::new("b")
            },
            ModelSpec::new("c"),
        ]);
        let new = catalog(vec![
            ModelSpec {
                context_window: Some(200),
                ..ModelSpec::new("b")
            },
            ModelSpec {
                status: Some(ModelStatus::Deprecated),
                ..ModelSpec::new("c")
            },
            ModelSpec::new("d"),
        ]);
        let diff = CatalogDiff::between(&old, &new);
        assert_eq!(diff.added, ["openai/d"]);
        assert_eq!(diff.removed, ["openai/a"]);
        assert_eq!(diff.deprecated, ["openai/c"]);
        assert_eq!(diff.context_changed, ["openai/b: 100 -> 200"]);
        let markdown = diff.to_markdown(10);
        assert!(markdown.contains("### Added (1)"));
        assert!(markdown.contains("- `openai/d`"));
        assert!(!diff.is_empty());
        assert!(CatalogDiff::between(&old, &old).is_empty());
    }

    #[test]
    fn markdown_truncates_long_sections() {
        let old = catalog(vec![]);
        let new = catalog((0..5).map(|i| ModelSpec::new(format!("m{i}"))).collect());
        let markdown = CatalogDiff::between(&old, &new).to_markdown(2);
        assert!(markdown.contains("… and 3 more"));
    }
}
