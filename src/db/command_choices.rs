use anyhow::Result;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct Choice {
    pub context_json: String,
    pub expires_at: i64,
    pub status: String,
}

pub async fn create(user: u64, context: &str, pool: &SqlitePool) -> Result<i64> {
    create_until(user, context, chrono::Utc::now().timestamp() + 120, pool).await
}

pub async fn create_until(
    user: u64,
    context: &str,
    expires_at: i64,
    pool: &SqlitePool,
) -> Result<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE command_choices SET status = 'cancelled' WHERE user_id = ? AND status = 'pending'",
    )
    .bind(user as i64)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "INSERT INTO command_choices(user_id, context_json, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user as i64)
    .bind(context)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.last_insert_rowid())
}
pub async fn get(id: i64, user: u64, pool: &SqlitePool) -> Result<Option<Choice>> {
    Ok(sqlx::query_as(
        "SELECT context_json, expires_at, status FROM command_choices WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user as i64)
    .fetch_optional(pool)
    .await?)
}
pub async fn claim(id: i64, user: u64, pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query("UPDATE command_choices SET status = 'selected' WHERE id = ? AND user_id = ? AND status = 'pending' AND expires_at > ?")
        .bind(id).bind(user as i64).bind(chrono::Utc::now().timestamp()).execute(pool).await?.rows_affected() == 1)
}
pub async fn cancel(id: i64, user: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE command_choices SET status = 'cancelled' WHERE id = ? AND user_id = ? AND status = 'pending'")
        .bind(id).bind(user as i64).execute(pool).await?;
    Ok(())
}
pub async fn cancel_all(user: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "UPDATE command_choices SET status = 'cancelled' WHERE user_id = ? AND status = 'pending'",
    )
    .bind(user as i64)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn choice_claim_is_single_use_owner_bound_and_expires() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::raw_sql("CREATE TABLE users(id INTEGER PRIMARY KEY); INSERT INTO users VALUES(1),(2); CREATE TABLE command_choices(id INTEGER PRIMARY KEY, user_id INTEGER, context_json TEXT, expires_at INTEGER, status TEXT DEFAULT 'pending');").execute(&pool).await?;
        let id = create(1, "{}", &pool).await?;
        assert!(!claim(id, 2, &pool).await?);
        let (a, b) = tokio::join!(claim(id, 1, &pool), claim(id, 1, &pool));
        assert_ne!(a?, b?);
        let old = create(1, "{}", &pool).await?;
        let current = create(1, "{}", &pool).await?;
        assert!(!claim(old, 1, &pool).await?);
        sqlx::query("UPDATE command_choices SET expires_at = 0 WHERE id = ?")
            .bind(current)
            .execute(&pool)
            .await?;
        assert!(!claim(current, 1, &pool).await?);
        Ok(())
    }
}
