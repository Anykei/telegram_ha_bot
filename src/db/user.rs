use anyhow::{Result};

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

pub async fn get_all_active_sessions(pool: &sqlx::SqlitePool) -> Result<Vec<(i64, i32, String)>> {
    let rows = sqlx::query_as::<_, (i64, i32, String)>(
        "SELECT id, last_menu_id, current_context FROM users WHERE last_menu_id IS NOT NULL"
    )
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn save_user_session(pool: &sqlx::SqlitePool, user_id: u64, msg_id: i32, context: &str) {
    let ctx = context.to_string();
    let pool_local = pool.clone();
    let uid = user_id as i64;

    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO users (id, last_menu_id, current_context)
            VALUES (?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                last_menu_id = excluded.last_menu_id,
                current_context = excluded.current_context
            "#
        )
            .bind(uid)
            .bind(msg_id)
            .bind(ctx)
            .execute(&pool_local)
            .await;

        match res {
            Ok(result) => {
                if result.rows_affected() == 0 {
                    log::warn!("Session user {} no changed", uid);
                } else {
                    log::debug!("Session user {} save on disk", uid);
                }
            }
            Err(e) => log::error!("Critical error save session on disk: {}", e),
        }
    });
}