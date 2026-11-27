use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use {async_trait::async_trait, serde_json::Value, tracing::warn};

use moltis_projects::{
    ProjectStore,
    complete::complete_path,
    context::load_context_files,
    detect::{auto_detect, default_scan_dirs},
    files::search_files,
};

use crate::services::{ProjectService, ServiceError, ServiceResult};

/// Most results a `projects.files` call returns.
const MAX_FILE_RESULTS: usize = 50;

/// The directory a session works in: its worktree when it has one inside
/// the project, the project directory otherwise.
fn session_root(project_dir: &Path, session_key: Option<&str>) -> PathBuf {
    let Some(key) = session_key.filter(|key| !key.is_empty()) else {
        return project_dir.to_path_buf();
    };
    // The key comes from the client: never let it walk out of the project.
    if key.contains('/') || key.contains('\\') || key.contains("..") {
        return project_dir.to_path_buf();
    }
    let worktree = project_dir.join(".moltis-worktrees").join(key);
    if worktree.is_dir() {
        worktree
    } else {
        project_dir.to_path_buf()
    }
}

pub struct LiveProjectService {
    store: Arc<dyn ProjectStore>,
}

impl LiveProjectService {
    pub fn new(store: Arc<dyn ProjectStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ProjectService for LiveProjectService {
    async fn list(&self) -> ServiceResult {
        let projects = self.store.list().await.map_err(ServiceError::message)?;
        Ok(serde_json::to_value(projects)?)
    }

    async fn get(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id' parameter".to_string())?;
        let project = self.store.get(id).await.map_err(ServiceError::message)?;
        Ok(serde_json::to_value(project)?)
    }

    async fn upsert(&self, params: Value) -> ServiceResult {
        let project: moltis_projects::Project = serde_json::from_value(params)?;
        self.store
            .upsert(project.clone())
            .await
            .map_err(ServiceError::message)?;
        Ok(serde_json::to_value(project)?)
    }

    async fn delete(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id' parameter".to_string())?;
        self.store.delete(id).await.map_err(ServiceError::message)?;
        Ok(serde_json::json!({"deleted": id}))
    }

    async fn detect(&self, params: Value) -> ServiceResult {
        let dirs: Vec<String> = params
            .get("directories")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let existing = self.store.list().await.map_err(ServiceError::message)?;
        let known_ids: Vec<String> = existing.iter().map(|p| p.id.clone()).collect();

        let dir_refs: Vec<PathBuf> = if dirs.is_empty() {
            default_scan_dirs()
        } else {
            dirs.iter().map(PathBuf::from).collect()
        };
        let dir_slices: Vec<&Path> = dir_refs.iter().map(|p| p.as_path()).collect();
        let detected = auto_detect(&dir_slices, &known_ids);

        for p in &detected {
            if let Err(e) = self.store.upsert(p.clone()).await {
                warn!(id = %p.id, error = %e, "failed to persist detected project");
            }
        }

        Ok(serde_json::to_value(detected)?)
    }

    async fn complete_path(&self, params: Value) -> ServiceResult {
        let partial = params.get("partial").and_then(|v| v.as_str()).unwrap_or("");
        let results: Vec<String> = complete_path(partial)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        Ok(serde_json::to_value(results)?)
    }

    async fn files(&self, params: Value) -> ServiceResult {
        let id = params
            .get("projectId")
            .or_else(|| params.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'projectId' parameter".to_string())?;
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map_or(20, |limit| limit as usize)
            .clamp(1, MAX_FILE_RESULTS);
        let project = self
            .store
            .get(id)
            .await
            .map_err(ServiceError::message)?
            .ok_or_else(|| format!("project '{id}' not found"))?;
        let root = session_root(
            &project.directory,
            params.get("sessionKey").and_then(|v| v.as_str()),
        );
        let files = tokio::task::spawn_blocking(move || search_files(&root, &query, limit))
            .await
            .map_err(ServiceError::message)?;
        Ok(serde_json::json!({ "files": files }))
    }

    async fn context(&self, params: Value) -> ServiceResult {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'id' parameter".to_string())?;
        let project = self
            .store
            .get(id)
            .await
            .map_err(ServiceError::message)?
            .ok_or_else(|| format!("project '{id}' not found"))?;

        let context_files =
            load_context_files(&project.directory).map_err(ServiceError::message)?;
        let entries: Vec<Value> = context_files
            .iter()
            .map(|cf| {
                serde_json::json!({
                    "path": cf.path.to_string_lossy(),
                    "content": cf.content,
                    "kind": cf.kind,
                    "warnings": cf.warnings,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "project": project,
            "context_files": entries,
        }))
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod files_tests {
    use super::*;

    #[test]
    fn session_root_uses_an_existing_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().join(".moltis-worktrees").join("session:abc");
        std::fs::create_dir_all(&worktree).unwrap();
        assert_eq!(session_root(dir.path(), Some("session:abc")), worktree);
        assert_eq!(session_root(dir.path(), Some("session:none")), dir.path());
        assert_eq!(session_root(dir.path(), None), dir.path());
    }

    #[test]
    fn session_root_ignores_keys_that_could_escape() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(session_root(dir.path(), Some("../../etc")), dir.path());
        assert_eq!(session_root(dir.path(), Some("a/b")), dir.path());
    }
}
