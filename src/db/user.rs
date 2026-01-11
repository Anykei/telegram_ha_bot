

pub async fn user_exists(pool: &sqlx::SqlitePool, user_id: u64) -> bool {
    let result = sqlx::query_scalar::<_, i32>("SELECT 1 FROM users WHERE id = ? LIMIT 1")
        .bind(user_id as i64) // Убрали лишний cast
        .fetch_optional(pool)
        .await;

    match result {
        Ok(maybe_one) => maybe_one.is_some(),
        Err(e) => {
            error!("Error check user exists {} in DB: {}", user_id, e);
            false
        }
    }
}