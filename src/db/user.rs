//! Database module for managing users.
//!
//! This module handles user existence checks and session management.

use anyhow::Result;
use sqlx::SqlitePool;

/// Checks if a user exists in the database.
///
/// # Arguments
/// * `user_id` - ID of the user to check
/// * `pool` - Database connection pool
///
/// # Returns
/// * `bool` - True if user exists, false otherwise
pub async fn user_exists(user_id: u64, pool: &SqlitePool) -> bool {
    let result = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM users WHERE id = ? LIMIT 1",
    )
    .bind(user_id as i64)
    .fetch_optional(pool)
    .await;

    match result {
        Ok(maybe_one) => maybe_one.is_some(),
        Err(e) => {
            log::error!("Error checking if user {} exists in DB: {}", user_id, e);
            false
        }
    }
}

/// Retrieves all active user sessions.
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// * `Result<Vec<(i64, i32, String)>>` - List of active sessions
pub async fn get_all_active_sessions(pool: &SqlitePool) -> Result<Vec<(i64, i32, String)>> {
    let rows = sqlx::query_as::<_, (i64, i32, String)>(
        "SELECT id, last_menu_id, current_context FROM users WHERE last_menu_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Saves a user session to the database.
///
/// # Arguments
/// * `user_id` - ID of the user
/// * `msg_id` - Message ID
/// * `context` - Current context string
/// * `pool` - Database connection pool
pub async fn save_user_session(
    user_id: u64,
    msg_id: i32,
    context: &str,
    pool: &SqlitePool,
) {
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
            "#,
        )
        .bind(uid)
        .bind(msg_id)
        .bind(ctx)
        .execute(&pool_local)
        .await;

        match res {
            Ok(result) => {
                if result.rows_affected() == 0 {
                    log::warn!("Session for user {} was not changed", uid);
                } else {
                    log::debug!("Session for user {} saved to disk", uid);
                }
            }
            Err(e) => log::error!("Critical error saving session to disk: {}", e),
        }
    });
}