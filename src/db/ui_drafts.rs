//! Persistent, owner-bound UI drafts. Every write uses optimistic concurrency.
use anyhow::{bail, Result};
use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct UiDraft {
    pub id: i64,
    pub owner_id: i64,
    pub kind: String,
    pub schema_version: i64,
    pub revision: i64,
    pub payload: String,
    pub step: String,
    pub return_context: String,
    pub status: String,
    pub updated_at: i64,
    pub expires_at: i64,
    pub committed_target_id: Option<i64>,
}

pub async fn create(
    owner: u64,
    kind: &str,
    payload: &str,
    ttl_hours: u64,
    pool: &SqlitePool,
) -> Result<UiDraft> {
    let now = Utc::now().timestamp();
    let id = sqlx::query(
        "INSERT INTO ui_drafts(owner_id,kind,payload,updated_at,expires_at) VALUES(?,?,?,?,?)",
    )
    .bind(owner as i64)
    .bind(kind)
    .bind(payload)
    .bind(now)
    .bind(expiration(now, ttl_hours))
    .execute(pool)
    .await?
    .last_insert_rowid();
    audit(owner, id, "draft.create", pool).await;
    Ok(load(id, owner, kind, pool).await?.expect("new draft"))
}

pub async fn load(id: i64, owner: u64, kind: &str, pool: &SqlitePool) -> Result<Option<UiDraft>> {
    Ok(sqlx::query_as("SELECT * FROM ui_drafts WHERE id=? AND owner_id=? AND kind=? AND expires_at>? AND status IN ('editing','review','saved')")
        .bind(id).bind(owner as i64).bind(kind).bind(Utc::now().timestamp()).fetch_optional(pool).await?)
}

pub async fn latest(owner: u64, kind: &str, pool: &SqlitePool) -> Result<Option<UiDraft>> {
    Ok(sqlx::query_as("SELECT * FROM ui_drafts WHERE owner_id=? AND kind=? AND expires_at>? AND status IN ('editing','review') ORDER BY updated_at DESC,id DESC LIMIT 1")
        .bind(owner as i64).bind(kind).bind(Utc::now().timestamp()).fetch_optional(pool).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn save(
    id: i64,
    owner: u64,
    revision: i64,
    payload: &str,
    step: &str,
    return_context: &str,
    review: bool,
    ttl_hours: u64,
    pool: &SqlitePool,
) -> Result<i64> {
    let now = Utc::now().timestamp();
    let result = sqlx::query("UPDATE ui_drafts SET payload=?,step=?,return_context=?,status=?,revision=revision+1,updated_at=?,expires_at=? WHERE id=? AND owner_id=? AND revision=? AND expires_at>CAST(strftime('%s','now') AS INTEGER) AND status IN ('editing','review')")
        .bind(payload).bind(step).bind(return_context).bind(if review { "review" } else { "editing" })
        .bind(now).bind(expiration(now, ttl_hours)).bind(id).bind(owner as i64).bind(revision)
        .execute(pool).await?;
    if result.rows_affected() != 1 {
        bail!("Draft expired or changed; reopen its current version");
    }
    audit(owner, id, "draft.save", pool).await;
    Ok(revision + 1)
}

pub async fn cancel(id: i64, owner: u64, revision: i64, pool: &SqlitePool) -> Result<()> {
    let result = sqlx::query("UPDATE ui_drafts SET status='cancelled',revision=revision+1,updated_at=? WHERE id=? AND owner_id=? AND revision=? AND expires_at>CAST(strftime('%s','now') AS INTEGER) AND status IN ('editing','review')")
        .bind(Utc::now().timestamp()).bind(id).bind(owner as i64).bind(revision)
        .execute(pool).await?;
    if result.rows_affected() != 1 {
        bail!("Draft expired or changed; reopen its current version");
    }
    audit(owner, id, "draft.cancel", pool).await;
    Ok(())
}

async fn audit(owner: u64, id: i64, action: &str, pool: &SqlitePool) {
    let correlation = id.to_string();
    let _ = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: Some(owner),
            kind: "ui_draft",
            entity_type: "draft",
            entity_id: Some(&correlation),
            action,
            status: "ok",
            message: None,
        },
        pool,
    )
    .await;
}

fn expiration(now: i64, ttl_hours: u64) -> i64 {
    now.saturating_add(i64::try_from(ttl_hours.saturating_mul(3600)).unwrap_or(i64::MAX))
}

/// Bound each maintenance tick; saved-result links remain until their draft TTL.
pub async fn purge_expired(pool: &SqlitePool) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query("DELETE FROM ui_drafts WHERE id IN (SELECT id FROM ui_drafts WHERE expires_at <= ? ORDER BY expires_at LIMIT 500)")
        .bind(now).execute(pool).await?;
    sqlx::query("DELETE FROM command_choices WHERE id IN (SELECT id FROM command_choices WHERE expires_at <= ? ORDER BY expires_at LIMIT 500)")
        .bind(now).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn drafts_reject_wrong_owner_stale_revision_and_expiration() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let draft = create(7, "recording_rule", "{}", 24, &pool).await?;
        assert!(load(draft.id, 8, "recording_rule", &pool).await?.is_none());
        let revision = save(draft.id, 7, 0, "{\"a\":1}", "review", "", true, 24, &pool).await?;
        assert_eq!(revision, 1);
        assert!(save(draft.id, 7, 0, "{}", "", "", false, 24, &pool)
            .await
            .is_err());
        assert_eq!(
            load(draft.id, 7, "recording_rule", &pool)
                .await?
                .unwrap()
                .payload,
            "{\"a\":1}"
        );
        sqlx::query("UPDATE ui_drafts SET expires_at=0")
            .execute(&pool)
            .await?;
        assert!(load(draft.id, 7, "recording_rule", &pool).await?.is_none());
        assert!(save(draft.id, 7, 1, "{}", "", "", false, 24, &pool)
            .await
            .is_err());
        let active = create(8, "recording_rule", "{}", 24, &pool).await?;
        purge_expired(&pool).await?;
        assert!(load(active.id, 8, "recording_rule", &pool).await?.is_some());
        let expired_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ui_drafts WHERE id=?")
            .bind(draft.id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(expired_rows, 0);
        Ok(())
    }
}
