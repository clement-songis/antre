//! SQLite persistence for design metadata and the append-only version list.

use {
    serde::Serialize,
    sqlx::{FromRow, SqlitePool},
    time::OffsetDateTime,
};

use crate::{DesignId, Error, Result};

/// Metadata of a design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Design {
    pub id: DesignId,
    pub title: String,
    pub session_key: Option<String>,
    pub project_id: Option<String>,
    pub design_system: Option<String>,
    pub current_version: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One immutable snapshot of a design directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignVersion {
    pub design_id: DesignId,
    pub version: u32,
    #[serde(skip)]
    pub checkpoint_id: String,
    pub content_hash: String,
    pub reason: String,
    pub session_key: Option<String>,
    pub run_id: Option<String>,
    pub created_at: i64,
}

/// Fields of a new design row.
#[derive(Debug, Clone, Default)]
pub struct NewDesign {
    pub title: String,
    pub session_key: Option<String>,
    pub project_id: Option<String>,
    pub design_system: Option<String>,
}

/// Fields of a new version row; the version number is assigned by the store.
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub checkpoint_id: String,
    pub content_hash: String,
    pub reason: String,
    pub session_key: Option<String>,
    pub run_id: Option<String>,
}

/// Filter for [`DesignStore::list`].
#[derive(Debug, Clone, Default)]
pub struct DesignFilter {
    pub session_key: Option<String>,
    pub project_id: Option<String>,
    pub limit: Option<u32>,
}

pub const DEFAULT_LIST_LIMIT: u32 = 50;
pub const MAX_LIST_LIMIT: u32 = 500;
pub const MAX_TITLE_CHARS: usize = 200;

#[derive(FromRow)]
struct DesignRow {
    id: String,
    title: String,
    session_key: Option<String>,
    project_id: Option<String>,
    design_system: Option<String>,
    current_version: i64,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<DesignRow> for Design {
    type Error = Error;

    fn try_from(row: DesignRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            title: row.title,
            session_key: row.session_key,
            project_id: row.project_id,
            design_system: row.design_system,
            current_version: version_from_db(row.current_version)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct VersionRow {
    design_id: String,
    version: i64,
    checkpoint_id: String,
    content_hash: String,
    reason: String,
    session_key: Option<String>,
    run_id: Option<String>,
    created_at: i64,
}

impl TryFrom<VersionRow> for DesignVersion {
    type Error = Error;

    fn try_from(row: VersionRow) -> Result<Self> {
        Ok(Self {
            design_id: row.design_id.parse()?,
            version: version_from_db(row.version)?,
            checkpoint_id: row.checkpoint_id,
            content_hash: row.content_hash,
            reason: row.reason,
            session_key: row.session_key,
            run_id: row.run_id,
            created_at: row.created_at,
        })
    }
}

fn version_from_db(value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::message(format!("corrupt version number {value}")))
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// Normalize a user- or model-supplied title.
pub fn clean_title(title: &str) -> Result<String> {
    let title: String = title
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    if title.is_empty() {
        return Err(Error::Invalid("title must not be empty".into()));
    }
    Ok(title)
}

/// Design metadata store backed by the shared `moltis.db` pool.
#[derive(Debug, Clone)]
pub struct DesignStore {
    pool: SqlitePool,
}

impl DesignStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, id: &DesignId, design: NewDesign) -> Result<Design> {
        let now = now();
        sqlx::query(
            "INSERT INTO canvas_designs (id, title, session_key, project_id, design_system, \
             current_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(id.as_str())
        .bind(clean_title(&design.title)?)
        .bind(&design.session_key)
        .bind(&design.project_id)
        .bind(&design.design_system)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get(id).await
    }

    pub async fn get(&self, id: &DesignId) -> Result<Design> {
        sqlx::query_as::<_, DesignRow>("SELECT * FROM canvas_designs WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(id.to_string()))?
            .try_into()
    }

    pub async fn exists(&self, id: &DesignId) -> Result<bool> {
        let found: Option<i64> = sqlx::query_scalar("SELECT 1 FROM canvas_designs WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await?;
        Ok(found.is_some())
    }

    pub async fn list(&self, filter: &DesignFilter) -> Result<Vec<Design>> {
        let limit = filter
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let rows = sqlx::query_as::<_, DesignRow>(
            "SELECT * FROM canvas_designs \
             WHERE (?1 IS NULL OR session_key = ?1) AND (?2 IS NULL OR project_id = ?2) \
             ORDER BY updated_at DESC, id ASC LIMIT ?3",
        )
        .bind(&filter.session_key)
        .bind(&filter.project_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Design::try_from).collect()
    }

    pub async fn rename(&self, id: &DesignId, title: &str) -> Result<Design> {
        let result =
            sqlx::query("UPDATE canvas_designs SET title = ?, updated_at = ? WHERE id = ?")
                .bind(clean_title(title)?)
                .bind(now())
                .bind(id.as_str())
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        self.get(id).await
    }

    /// Append a version and make it current, atomically.
    pub async fn append_version(
        &self,
        id: &DesignId,
        version: NewVersion,
    ) -> Result<DesignVersion> {
        let now = now();
        let mut tx = self.pool.begin().await?;
        let current: Option<i64> =
            sqlx::query_scalar("SELECT current_version FROM canvas_designs WHERE id = ?")
                .bind(id.as_str())
                .fetch_optional(&mut *tx)
                .await?;
        let current = current.ok_or_else(|| Error::NotFound(id.to_string()))?;
        let latest: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(version) FROM canvas_design_versions WHERE design_id = ?",
        )
        .bind(id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        let next = latest.unwrap_or(0).max(current) + 1;
        sqlx::query(
            "INSERT INTO canvas_design_versions (design_id, version, checkpoint_id, content_hash, \
             reason, session_key, run_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.as_str())
        .bind(next)
        .bind(&version.checkpoint_id)
        .bind(&version.content_hash)
        .bind(&version.reason)
        .bind(&version.session_key)
        .bind(&version.run_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE canvas_designs SET current_version = ?, updated_at = ? WHERE id = ?")
            .bind(next)
            .bind(now)
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.version(id, version_from_db(next)?).await
    }

    pub async fn version(&self, id: &DesignId, version: u32) -> Result<DesignVersion> {
        sqlx::query_as::<_, VersionRow>(
            "SELECT * FROM canvas_design_versions WHERE design_id = ? AND version = ?",
        )
        .bind(id.as_str())
        .bind(i64::from(version))
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::VersionNotFound {
            design_id: id.to_string(),
            version,
        })?
        .try_into()
    }

    pub async fn latest_version(&self, id: &DesignId) -> Result<Option<DesignVersion>> {
        sqlx::query_as::<_, VersionRow>(
            "SELECT * FROM canvas_design_versions WHERE design_id = ? \
             ORDER BY version DESC LIMIT 1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?
        .map(DesignVersion::try_from)
        .transpose()
    }

    /// Versions of a design, newest first.
    pub async fn versions(&self, id: &DesignId, limit: u32) -> Result<Vec<DesignVersion>> {
        let rows = sqlx::query_as::<_, VersionRow>(
            "SELECT * FROM canvas_design_versions WHERE design_id = ? \
             ORDER BY version DESC LIMIT ?",
        )
        .bind(id.as_str())
        .bind(i64::from(limit.clamp(1, MAX_LIST_LIMIT)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(DesignVersion::try_from).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;

    pub(crate) async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::run_migrations(&pool).await.unwrap();
        pool
    }

    fn version(hash: &str) -> NewVersion {
        NewVersion {
            checkpoint_id: "0".repeat(32),
            content_hash: hash.into(),
            reason: "test".into(),
            session_key: Some("main".into()),
            run_id: None,
        }
    }

    #[tokio::test]
    async fn insert_get_and_list() {
        let store = DesignStore::new(test_pool().await);
        let first = DesignId::generate();
        let second = DesignId::generate();
        store
            .insert(&first, NewDesign {
                title: "  Landing\npage ".into(),
                session_key: Some("main".into()),
                ..NewDesign::default()
            })
            .await
            .unwrap();
        store
            .insert(&second, NewDesign {
                title: "Deck".into(),
                session_key: Some("other".into()),
                project_id: Some("p1".into()),
                ..NewDesign::default()
            })
            .await
            .unwrap();

        let design = store.get(&first).await.unwrap();
        assert_eq!(design.title, "Landingpage");
        assert_eq!(design.current_version, 0);
        assert!(store.exists(&first).await.unwrap());

        let all = store.list(&DesignFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
        let main = store
            .list(&DesignFilter {
                session_key: Some("main".into()),
                ..DesignFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].id, first);
        let project = store
            .list(&DesignFilter {
                project_id: Some("p1".into()),
                ..DesignFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(project[0].id, second);
    }

    #[tokio::test]
    async fn missing_design_is_not_found() {
        let store = DesignStore::new(test_pool().await);
        let id = DesignId::generate();
        assert!(store.get(&id).await.unwrap_err().is_not_found());
        assert!(!store.exists(&id).await.unwrap());
        assert!(
            store
                .append_version(&id, version("h"))
                .await
                .unwrap_err()
                .is_not_found()
        );
        assert!(store.rename(&id, "x").await.unwrap_err().is_not_found());
    }

    #[tokio::test]
    async fn versions_are_sequential_and_listed_newest_first() {
        let store = DesignStore::new(test_pool().await);
        let id = DesignId::generate();
        store
            .insert(&id, NewDesign {
                title: "x".into(),
                ..NewDesign::default()
            })
            .await
            .unwrap();
        let v1 = store.append_version(&id, version("a")).await.unwrap();
        let v2 = store.append_version(&id, version("b")).await.unwrap();
        assert_eq!((v1.version, v2.version), (1, 2));
        assert_eq!(store.get(&id).await.unwrap().current_version, 2);
        let listed = store.versions(&id, 10).await.unwrap();
        assert_eq!(listed.iter().map(|v| v.version).collect::<Vec<_>>(), vec![
            2, 1
        ]);
        assert_eq!(
            store
                .latest_version(&id)
                .await
                .unwrap()
                .unwrap()
                .content_hash,
            "b"
        );
        assert!(matches!(
            store.version(&id, 9).await.unwrap_err(),
            Error::VersionNotFound { version: 9, .. }
        ));
    }

    #[tokio::test]
    async fn versions_are_append_only_in_the_database() {
        let pool = test_pool().await;
        let store = DesignStore::new(pool.clone());
        let id = DesignId::generate();
        store
            .insert(&id, NewDesign {
                title: "x".into(),
                ..NewDesign::default()
            })
            .await
            .unwrap();
        store.append_version(&id, version("a")).await.unwrap();
        let err = sqlx::query("UPDATE canvas_design_versions SET content_hash = 'z'")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
    }

    #[tokio::test]
    async fn titles_are_cleaned_and_required() {
        assert!(clean_title(" \n\t").is_err());
        assert_eq!(
            clean_title(&"a".repeat(500)).unwrap().len(),
            MAX_TITLE_CHARS
        );
        let store = DesignStore::new(test_pool().await);
        let id = DesignId::generate();
        store
            .insert(&id, NewDesign {
                title: "old".into(),
                ..NewDesign::default()
            })
            .await
            .unwrap();
        assert_eq!(store.rename(&id, "new").await.unwrap().title, "new");
    }
}
