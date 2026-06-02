use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ActivityLogEntry {
    pub id: i64,
    pub user_id: Option<i64>,
    pub kind: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub action: String,
    pub status: String,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct NewActivity<'a> {
    pub user_id: Option<u64>,
    pub kind: &'a str,
    pub entity_type: &'a str,
    pub entity_id: Option<&'a str>,
    pub action: &'a str,
    pub status: &'a str,
    pub message: Option<&'a str>,
}

pub async fn log(entry: NewActivity<'_>, pool: &SqlitePool) -> Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO activity_log (
            user_id, kind, entity_type, entity_id, action, status, message
        )
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(entry.user_id.map(|id| id as i64))
    .bind(entry.kind)
    .bind(entry.entity_type)
    .bind(entry.entity_id)
    .bind(entry.action)
    .bind(entry.status)
    .bind(entry.message.map(crate::db::sanitize_error))
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn list_recent(
    kind: Option<&str>,
    only_errors: bool,
    limit: i64,
    pool: &SqlitePool,
) -> Result<Vec<ActivityLogEntry>> {
    let limit = limit.clamp(1, 100);
    let rows = match (kind, only_errors) {
        (Some(kind), true) => {
            sqlx::query_as::<_, ActivityLogEntry>(
                r#"
                SELECT id, user_id, kind, entity_type, entity_id, action, status, message, created_at
                FROM activity_log
                WHERE kind = ? AND status = 'error'
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(kind)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (Some(kind), false) => {
            sqlx::query_as::<_, ActivityLogEntry>(
                r#"
                SELECT id, user_id, kind, entity_type, entity_id, action, status, message, created_at
                FROM activity_log
                WHERE kind = ?
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(kind)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (None, true) => {
            sqlx::query_as::<_, ActivityLogEntry>(
                r#"
                SELECT id, user_id, kind, entity_type, entity_id, action, status, message, created_at
                FROM activity_log
                WHERE status = 'error'
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (None, false) => {
            sqlx::query_as::<_, ActivityLogEntry>(
                r#"
                SELECT id, user_id, kind, entity_type, entity_id, action, status, message, created_at
                FROM activity_log
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };

    Ok(rows)
}

pub async fn purge_older_than(cutoff: DateTime<Utc>, pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query("DELETE FROM activity_log WHERE DATETIME(created_at) < DATETIME(?)")
        .bind(cutoff)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

pub async fn count_recording_rule_events_since(
    rule_id: i64,
    since: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM activity_log
        WHERE kind = 'recording'
          AND entity_type = 'rule'
          AND entity_id = ?
          AND action IN ('recording_created', 'recording_extended')
          AND status = 'ok'
          AND DATETIME(created_at) >= DATETIME(?)
        "#,
    )
    .bind(rule_id.to_string())
    .bind(since)
    .fetch_one(pool)
    .await?)
}
