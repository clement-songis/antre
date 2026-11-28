//! Comment pins on design versions.
//!
//! A pin anchors a note to an element of one version (its `data-mid`, else a
//! CSS selector) or to page coordinates. The web UI turns open pins into one
//! prompt ("Send to agent") and then resolves them.

use {
    serde::{Deserialize, Serialize},
    sqlx::{FromRow, SqlitePool},
    time::OffsetDateTime,
};

use crate::{DesignId, Error, Result, events::CanvasEvent};

pub const MAX_BODY_CHARS: usize = 2000;
pub const MAX_SELECTOR_CHARS: usize = 500;
pub const MAX_ELEMENT_TEXT_CHARS: usize = 500;
pub const MAX_MID_CHARS: usize = 120;
/// Most comments kept on one design version.
pub const MAX_COMMENTS_PER_VERSION: i64 = 500;

/// A stored comment pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub design_id: DesignId,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i64>,
    pub body: String,
    pub resolved: bool,
    pub created_at: i64,
}

/// Fields of a new comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewComment {
    pub mid: Option<String>,
    pub selector: Option<String>,
    pub element_text: Option<String>,
    pub x: Option<i64>,
    pub y: Option<i64>,
    pub body: String,
}

fn clip(value: Option<String>, max: usize) -> Option<String> {
    value
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(max)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|text| !text.is_empty())
}

impl NewComment {
    fn cleaned(self) -> Result<Self> {
        let body: String = self
            .body
            .chars()
            .filter(|ch| *ch == '\n' || !ch.is_control())
            .collect::<String>()
            .trim()
            .to_string();
        if body.is_empty() {
            return Err(Error::Invalid("comment must not be empty".into()));
        }
        if body.chars().count() > MAX_BODY_CHARS {
            return Err(Error::Invalid(format!(
                "comment is longer than {MAX_BODY_CHARS} characters"
            )));
        }
        let comment = Self {
            mid: clip(self.mid, MAX_MID_CHARS),
            selector: clip(self.selector, MAX_SELECTOR_CHARS),
            element_text: clip(self.element_text, MAX_ELEMENT_TEXT_CHARS),
            x: self.x.map(|x| x.clamp(0, 1_000_000)),
            y: self.y.map(|y| y.clamp(0, 1_000_000)),
            body,
        };
        let anchored = comment.mid.is_some()
            || comment.selector.is_some()
            || (comment.x.is_some() && comment.y.is_some());
        if !anchored {
            return Err(Error::Invalid(
                "comment needs an element (mid or selector) or a position".into(),
            ));
        }
        Ok(comment)
    }
}

#[derive(FromRow)]
struct CommentRow {
    id: String,
    design_id: String,
    version: i64,
    mid: Option<String>,
    selector: Option<String>,
    element_text: Option<String>,
    x: Option<i64>,
    y: Option<i64>,
    body: String,
    resolved: i64,
    created_at: i64,
}

impl TryFrom<CommentRow> for Comment {
    type Error = Error;

    fn try_from(row: CommentRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            design_id: row.design_id.parse()?,
            version: u32::try_from(row.version)
                .map_err(|_| Error::message(format!("corrupt version number {}", row.version)))?,
            mid: row.mid,
            selector: row.selector,
            element_text: row.element_text,
            x: row.x,
            y: row.y,
            body: row.body,
            resolved: row.resolved != 0,
            created_at: row.created_at,
        })
    }
}

/// SQLite store of comment pins.
#[derive(Debug, Clone)]
pub struct CommentStore {
    pool: SqlitePool,
}

impl CommentStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        id: &DesignId,
        version: u32,
        comment: NewComment,
    ) -> Result<Comment> {
        let comment = comment.cleaned()?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM canvas_design_comments WHERE design_id = ? AND version = ?",
        )
        .bind(id.as_str())
        .bind(i64::from(version))
        .fetch_one(&self.pool)
        .await?;
        if count >= MAX_COMMENTS_PER_VERSION {
            return Err(Error::Invalid(format!(
                "a version holds at most {MAX_COMMENTS_PER_VERSION} comments"
            )));
        }
        let comment_id = DesignId::generate().to_string();
        sqlx::query(
            "INSERT INTO canvas_design_comments (id, design_id, version, mid, selector, \
             element_text, x, y, body, resolved, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&comment_id)
        .bind(id.as_str())
        .bind(i64::from(version))
        .bind(&comment.mid)
        .bind(&comment.selector)
        .bind(&comment.element_text)
        .bind(comment.x)
        .bind(comment.y)
        .bind(&comment.body)
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .execute(&self.pool)
        .await?;
        self.get(id, &comment_id).await
    }

    pub async fn get(&self, id: &DesignId, comment_id: &str) -> Result<Comment> {
        sqlx::query_as::<_, CommentRow>(
            "SELECT * FROM canvas_design_comments WHERE design_id = ? AND id = ?",
        )
        .bind(id.as_str())
        .bind(comment_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("comment {comment_id}")))?
        .try_into()
    }

    /// Comments of a design, oldest first; one version or all of them.
    pub async fn list(
        &self,
        id: &DesignId,
        version: Option<u32>,
        include_resolved: bool,
    ) -> Result<Vec<Comment>> {
        let rows = sqlx::query_as::<_, CommentRow>(
            "SELECT * FROM canvas_design_comments \
             WHERE design_id = ?1 AND (?2 IS NULL OR version = ?2) AND (?3 OR resolved = 0) \
             ORDER BY created_at ASC, rowid ASC",
        )
        .bind(id.as_str())
        .bind(version.map(i64::from))
        .bind(include_resolved)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Comment::try_from).collect()
    }

    /// Mark comments resolved; returns how many changed.
    pub async fn resolve(&self, id: &DesignId, comment_ids: &[String]) -> Result<u64> {
        let mut changed = 0;
        let mut tx = self.pool.begin().await?;
        for comment_id in comment_ids.iter().take(MAX_COMMENTS_PER_VERSION as usize) {
            changed += sqlx::query(
                "UPDATE canvas_design_comments SET resolved = 1 \
                 WHERE design_id = ? AND id = ? AND resolved = 0",
            )
            .bind(id.as_str())
            .bind(comment_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn delete(&self, id: &DesignId, comment_id: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM canvas_design_comments WHERE design_id = ? AND id = ?")
                .bind(id.as_str())
                .bind(comment_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl crate::CanvasService {
    /// Comments of a design (one version, or all when `version` is `None`).
    pub async fn comments(
        &self,
        id: &DesignId,
        version: Option<u32>,
        include_resolved: bool,
    ) -> Result<Vec<Comment>> {
        self.store().get(id).await?;
        self.comment_store.list(id, version, include_resolved).await
    }

    /// Pin a comment on `version` (the current version when `None`).
    pub async fn add_comment(
        &self,
        id: &DesignId,
        version: Option<u32>,
        comment: NewComment,
    ) -> Result<Comment> {
        let design = self.store().get(id).await?;
        let version = match version {
            Some(number) => self.store().version(id, number).await?.version,
            None if design.current_version > 0 => design.current_version,
            None => return Err(Error::Invalid("design has no versions yet".into())),
        };
        let added = self.comment_store.insert(id, version, comment).await?;
        self.emit_comments_changed(id, version);
        Ok(added)
    }

    /// Resolve comments, typically after sending them to the agent.
    pub async fn resolve_comments(&self, id: &DesignId, comment_ids: &[String]) -> Result<u64> {
        self.store().get(id).await?;
        let changed = self.comment_store.resolve(id, comment_ids).await?;
        if changed > 0 {
            self.emit(CanvasEvent::CommentsChanged {
                design_id: id.clone(),
                version: None,
            });
        }
        Ok(changed)
    }

    pub async fn delete_comment(&self, id: &DesignId, comment_id: &str) -> Result<bool> {
        let comment = match self.comment_store.get(id, comment_id).await {
            Ok(comment) => comment,
            Err(error) if error.is_not_found() => return Ok(false),
            Err(error) => return Err(error),
        };
        let deleted = self.comment_store.delete(id, comment_id).await?;
        if deleted {
            self.emit_comments_changed(id, comment.version);
        }
        Ok(deleted)
    }

    fn emit_comments_changed(&self, id: &DesignId, version: u32) {
        self.emit(CanvasEvent::CommentsChanged {
            design_id: id.clone(),
            version: Some(version),
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn comment(body: &str) -> NewComment {
        NewComment {
            mid: Some("hero".into()),
            body: body.into(),
            ..NewComment::default()
        }
    }

    #[test]
    fn cleaning_requires_body_and_anchor() {
        assert!(comment("  ").cleaned().is_err());
        assert!(
            NewComment {
                body: "x".into(),
                ..NewComment::default()
            }
            .cleaned()
            .is_err()
        );
        assert!(
            NewComment {
                body: "x".into(),
                x: Some(1),
                y: Some(2),
                ..NewComment::default()
            }
            .cleaned()
            .is_ok()
        );
        assert!(comment(&"x".repeat(MAX_BODY_CHARS + 1)).cleaned().is_err());
        let cleaned = NewComment {
            mid: Some(format!(" {}\u{7}", "m".repeat(200))),
            selector: Some("   ".into()),
            x: Some(-5),
            body: "line 1\nline 2\u{0}".into(),
            ..NewComment::default()
        }
        .cleaned()
        .unwrap();
        assert_eq!(cleaned.mid.unwrap().len(), MAX_MID_CHARS - 1);
        assert_eq!(cleaned.selector, None);
        assert_eq!(cleaned.x, Some(0));
        assert_eq!(cleaned.body, "line 1\nline 2");
    }

    #[tokio::test]
    async fn store_round_trip() {
        let pool = crate::store::tests::test_pool().await;
        let designs = crate::DesignStore::new(pool.clone());
        let id = DesignId::generate();
        designs
            .insert(&id, crate::store::NewDesign {
                title: "t".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let store = CommentStore::new(pool);
        let first = store.insert(&id, 1, comment("bigger")).await.unwrap();
        let second = store.insert(&id, 2, comment("bluer")).await.unwrap();
        assert_eq!(store.list(&id, None, false).await.unwrap().len(), 2);
        assert_eq!(store.list(&id, Some(1), false).await.unwrap(), vec![
            first.clone()
        ]);
        assert_eq!(
            store
                .resolve(&id, &[first.id.clone(), "missing".into()])
                .await
                .unwrap(),
            1
        );
        assert_eq!(store.list(&id, None, false).await.unwrap(), vec![
            second.clone()
        ]);
        assert_eq!(store.list(&id, None, true).await.unwrap().len(), 2);
        assert!(store.delete(&id, &second.id).await.unwrap());
        assert!(!store.delete(&id, &second.id).await.unwrap());
        let other = DesignId::generate();
        assert!(
            store
                .get(&other, &first.id)
                .await
                .unwrap_err()
                .is_not_found()
        );
    }
}
