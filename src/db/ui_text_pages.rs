use anyhow::Result;
use sqlx::SqlitePool;

pub async fn register(user: u64, context: &str, pool: &SqlitePool) -> Result<i64> {
    sqlx::query("INSERT INTO ui_text_pages (user_id, context) VALUES (?, ?) ON CONFLICT(user_id, context) DO NOTHING")
        .bind(user as i64).bind(context).execute(pool).await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM ui_text_pages WHERE user_id = ? AND context = ?")
            .bind(user as i64)
            .bind(context)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn context(id: i64, user: u64, pool: &SqlitePool) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT context FROM ui_text_pages WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user as i64)
            .fetch_optional(pool)
            .await?,
    )
}
