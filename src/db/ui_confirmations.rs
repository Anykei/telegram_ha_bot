use anyhow::Result;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct Confirmation {
    pub action_json: String,
    pub expires_at: i64,
    pub status: String,
    pub result_text: Option<String>,
    pub result_json: Option<String>,
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
    Ok(sqlx::query_as("SELECT action_json, expires_at, status, result_text, result_json FROM ui_confirmations WHERE id = ? AND user_id = ?")
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

pub async fn finish(
    id: i64,
    result: &crate::core::action_results::ActionResult,
    text: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query("UPDATE ui_confirmations SET status = ?, result_text = ?, result_json = ? WHERE id = ? AND status = 'executing'")
        .bind(if result.outcome == crate::core::action_results::Outcome::Sent { "executed" } else { "failed" })
        .bind(text).bind(serde_json::to_string(result)?).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn create_independent(user: u64, action: &str, pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query(
        "INSERT INTO ui_confirmations(user_id, action_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(action)
    .bind(chrono::Utc::now().timestamp() + 120)
    .execute(pool)
    .await?
    .last_insert_rowid())
}

pub async fn callback_operation(
    user: u64,
    key: &str,
    pool: &SqlitePool,
) -> Result<Option<(i64, bool)>> {
    Ok(sqlx::query_as("SELECT confirmation_id, requires_review FROM ui_action_buttons WHERE user_id = ? AND button_key = ?")
        .bind(user as i64).bind(key).fetch_optional(pool).await?)
}

pub async fn bind_callback(
    user: u64,
    key: &str,
    action: &str,
    review: bool,
    pool: &SqlitePool,
) -> Result<(i64, bool)> {
    let mut tx = pool.begin().await?;
    // Acquire SQLite's writer lock before the read so two concurrent adaptations serialize.
    let candidate = sqlx::query(
        "INSERT INTO ui_confirmations(user_id, action_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(action)
    .bind(chrono::Utc::now().timestamp() + 120)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    sqlx::query("INSERT OR IGNORE INTO ui_action_buttons(user_id, button_key, confirmation_id, requires_review) VALUES (?, ?, ?, ?)")
        .bind(user as i64).bind(key).bind(candidate).bind(review).execute(&mut *tx).await?;
    let saved: (i64, bool) = sqlx::query_as("SELECT confirmation_id, requires_review FROM ui_action_buttons WHERE user_id = ? AND button_key = ?")
        .bind(user as i64).bind(key).fetch_one(&mut *tx).await?;
    if saved.0 != candidate {
        sqlx::query("DELETE FROM ui_confirmations WHERE id = ?")
            .bind(candidate)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(saved)
}
