use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingCommandSource {
    Text,
    Voice,
}

impl PendingCommandSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Voice => "voice",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingCommand {
    pub id: i64,
    pub intent_json: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn create(
    user_id: u64,
    source: PendingCommandSource,
    command_text: &str,
    intent_json: &str,
    reason: Option<&str>,
    expires_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO pending_commands
            (user_id, source, command_text, intent_json, reason, expires_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id as i64)
    .bind(source.as_str())
    .bind(command_text)
    .bind(intent_json)
    .bind(reason)
    .bind(expires_at.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn get_pending(
    id: i64,
    user_id: u64,
    pool: &SqlitePool,
) -> Result<Option<PendingCommand>> {
    let row = sqlx::query_as::<_, (i64, String, String)>(
        r#"
        SELECT id, intent_json, expires_at
        FROM pending_commands
        WHERE id = ? AND user_id = ? AND status = 'pending'
        "#,
    )
    .bind(id)
    .bind(user_id as i64)
    .fetch_optional(pool)
    .await?;

    let Some((id, intent_json, expires_at)) = row else {
        return Ok(None);
    };

    let expires_at = DateTime::parse_from_rfc3339(&expires_at)?.with_timezone(&Utc);

    Ok(Some(PendingCommand {
        id,
        intent_json,
        expires_at,
    }))
}

pub async fn mark_executed(id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE pending_commands
        SET status = 'executed', executed_at = ?
        WHERE id = ?
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_cancelled(id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE pending_commands
        SET status = 'cancelled', cancelled_at = ?
        WHERE id = ? AND status = 'pending'
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn expire_old(pool: &SqlitePool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE pending_commands
        SET status = 'expired'
        WHERE status = 'pending' AND expires_at <= ?
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}
