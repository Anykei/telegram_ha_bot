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
    pub status: String,
    pub result_text: Option<String>,
    pub result_json: Option<String>,
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
    let row = sqlx::query_as::<_, (i64, String, String, String, Option<String>, Option<String>)>(
        r#"
        SELECT id, intent_json, expires_at, status, result_text, result_json
        FROM pending_commands
        WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(id)
    .bind(user_id as i64)
    .fetch_optional(pool)
    .await?;

    let Some((id, intent_json, expires_at, status, result_text, result_json)) = row else {
        return Ok(None);
    };

    let expires_at = DateTime::parse_from_rfc3339(&expires_at)?.with_timezone(&Utc);

    Ok(Some(PendingCommand {
        id,
        intent_json,
        expires_at,
        status,
        result_text,
        result_json,
    }))
}

/// Claim before the first external effect. Confirmed commands are never replayed.
pub async fn claim(id: i64, user: u64, pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query("UPDATE pending_commands SET status = 'confirmed', confirmed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND user_id = ? AND status = 'pending' AND julianday(expires_at) > julianday('now')")
        .bind(id).bind(user as i64)
        .execute(pool).await?.rows_affected() == 1)
}

pub async fn finish(
    id: i64,
    result: &crate::core::action_results::ActionResult,
    text: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query("UPDATE pending_commands SET status = ?, executed_at = ?, result_text = ?, result_json = ? WHERE id = ? AND status = 'confirmed'")
        .bind(if result.outcome == crate::core::action_results::Outcome::Sent { "executed" } else { "failed" })
        .bind(Utc::now().to_rfc3339()).bind(text).bind(serde_json::to_string(result)?).bind(id).execute(pool).await?;
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
        WHERE status = 'pending' AND julianday(expires_at) <= julianday('now')
        "#,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pending_claim_is_atomic_and_consumed_after_restart() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::raw_sql("CREATE TABLE users(id INTEGER PRIMARY KEY); INSERT INTO users VALUES(1);")
            .execute(&pool)
            .await?;
        sqlx::raw_sql("CREATE TABLE pending_commands(id INTEGER PRIMARY KEY, user_id INTEGER, source TEXT, command_text TEXT, intent_json TEXT, reason TEXT, status TEXT DEFAULT 'pending', expires_at TEXT, confirmed_at TEXT, cancelled_at TEXT, executed_at TEXT, result_text TEXT, result_json TEXT);").execute(&pool).await?;
        let id = create(
            1,
            PendingCommandSource::Text,
            "test",
            "{}",
            None,
            Utc::now() + chrono::Duration::seconds(60),
            &pool,
        )
        .await?;
        assert!(!claim(id, 2, &pool).await?);
        let (a, b) = tokio::join!(claim(id, 1, &pool), claim(id, 1, &pool));
        assert_ne!(a?, b?);
        assert!(!claim(id, 1, &pool).await?);
        assert_eq!(
            get_pending(id, 1, &pool).await?.unwrap().status,
            "confirmed"
        );
        let id = create(
            1,
            PendingCommandSource::Voice,
            "test",
            "{}",
            None,
            Utc::now() - chrono::Duration::seconds(1),
            &pool,
        )
        .await?;
        assert!(!claim(id, 1, &pool).await?);
        Ok(())
    }
}
