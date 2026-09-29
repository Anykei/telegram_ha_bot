use anyhow::Result;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct Confirmation {
    pub action_json: String,
    pub expires_at: i64,
    pub status: String,
    pub result_text: Option<String>,
}

pub async fn create(user: u64, action: &str, pool: &SqlitePool) -> Result<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE ui_confirmations SET status = 'cancelled' WHERE user_id = ? AND status = 'pending'",
    )
    .bind(user as i64)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "INSERT INTO ui_confirmations (user_id, action_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(action)
    .bind(chrono::Utc::now().timestamp() + 120)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.last_insert_rowid())
}

pub async fn get(id: i64, user: u64, pool: &SqlitePool) -> Result<Option<Confirmation>> {
    Ok(sqlx::query_as("SELECT action_json, expires_at, status, result_text FROM ui_confirmations WHERE id = ? AND user_id = ?")
        .bind(id).bind(user as i64).fetch_optional(pool).await?)
}

/// Claim before sending anything to HA. A timeout or process restart must not replay the action.
pub async fn claim(id: i64, user: u64, pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query("UPDATE ui_confirmations SET status = 'executing' WHERE id = ? AND user_id = ? AND status = 'pending' AND expires_at > ?")
        .bind(id).bind(user as i64).bind(chrono::Utc::now().timestamp()).execute(pool).await?.rows_affected() == 1)
}

pub async fn cancel(id: i64, user: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE ui_confirmations SET status = 'cancelled' WHERE id = ? AND user_id = ? AND status = 'pending'")
        .bind(id).bind(user as i64).execute(pool).await?;
    Ok(())
}

pub async fn finish(id: i64, success: bool, text: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE ui_confirmations SET status = ?, result_text = ? WHERE id = ? AND status = 'executing'")
        .bind(if success { "executed" } else { "failed" }).bind(text).bind(id).execute(pool).await?;
    Ok(())
}
