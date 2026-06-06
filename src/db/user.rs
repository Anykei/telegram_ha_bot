//! Database module for managing users.
//!
//! This module handles user existence checks and session management.

use crate::i18n::Language;
use anyhow::Result;
use chrono::{DateTime, Utc};
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
    let result = sqlx::query_scalar::<_, i32>("SELECT 1 FROM users WHERE id = ? LIMIT 1")
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

pub async fn list_users(pool: &SqlitePool) -> Result<Vec<u64>> {
    let rows = sqlx::query_as::<_, (i64,)>("SELECT id FROM users ORDER BY id")
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|(id,)| id as u64).collect())
}

pub async fn add_user(user_id: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO users (id) VALUES (?)")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_user_language(user_id: u64, pool: &SqlitePool) -> Result<Option<Language>> {
    let raw: Option<String> = sqlx::query_scalar("SELECT language FROM users WHERE id = ?")
        .bind(user_id as i64)
        .fetch_optional(pool)
        .await?;

    Ok(raw.as_deref().and_then(Language::parse))
}

pub async fn set_user_language(user_id: u64, language: Language, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO users (id, language)
        VALUES (?, ?)
        ON CONFLICT(id) DO UPDATE SET language = excluded.language
        "#,
    )
    .bind(user_id as i64)
    .bind(language.code())
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn cycle_user_language(
    user_id: u64,
    fallback: Language,
    pool: &SqlitePool,
) -> Result<Language> {
    let current = get_user_language(user_id, pool).await?.unwrap_or(fallback);
    let next = current.next();
    set_user_language(user_id, next, pool).await?;
    Ok(next)
}

pub async fn delete_user(user_id: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM user_device_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM user_room_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM user_profiles WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM subscriptions WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM pinned_headers WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    Ok(())
}

/// Retrieves all active user sessions.
///
/// # Arguments
/// * `pool` - Database connection pool
///
/// # Returns
/// * `Result<Vec<(i64, i32, String, DateTime<Utc>)>>` - List of active sessions
pub async fn get_all_active_sessions(
    pool: &SqlitePool,
) -> Result<Vec<(i64, i32, String, DateTime<Utc>)>> {
    let rows = sqlx::query_as::<_, (i64, i32, String, Option<String>)>(
        "SELECT id, last_menu_id, current_context, last_seen_at FROM users WHERE last_menu_id > 0 AND current_context != ''",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, last_menu_id, current_context, last_seen_at)| {
            (
                id,
                last_menu_id,
                current_context,
                parse_saved_timestamp(last_seen_at).unwrap_or_else(Utc::now),
            )
        })
        .collect())
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
    last_seen_at: DateTime<Utc>,
    pool: &SqlitePool,
) {
    let ctx = context.to_string();
    let uid = user_id as i64;
    let last_seen_at = last_seen_at.to_rfc3339();

    if pool.is_closed() {
        log::debug!(
            "Skip saving session for user {}: database pool is closed",
            uid
        );
        return;
    }

    let res = sqlx::query(
        r#"
        INSERT INTO users (id, last_menu_id, current_context, last_seen_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            last_menu_id = excluded.last_menu_id,
            current_context = excluded.current_context,
            last_seen_at = excluded.last_seen_at
        "#,
    )
    .bind(uid)
    .bind(msg_id)
    .bind(ctx)
    .bind(last_seen_at)
    .execute(pool)
    .await;

    match res {
        Ok(result) => {
            if result.rows_affected() == 0 {
                log::warn!("Session for user {} was not changed", uid);
            } else {
                log::debug!("Session for user {} saved to disk", uid);
            }
        }
        Err(e) => {
            if pool.is_closed() {
                log::debug!(
                    "Skip saving session for user {} during database shutdown: {}",
                    uid,
                    e
                );
            } else {
                log::error!("Critical error saving session to disk: {}", e);
            }
        }
    }
}

pub async fn clear_user_session(user_id: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "UPDATE users SET last_menu_id = -1, current_context = '', last_seen_at = NULL WHERE id = ?",
    )
    .bind(user_id as i64)
    .execute(pool)
    .await?;

    Ok(())
}

fn parse_saved_timestamp(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clear_user_session_removes_session_from_active_list() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                last_menu_id INTEGER DEFAULT -1,
                current_context TEXT DEFAULT '',
                last_seen_at TEXT
            )
            "#,
        )
        .execute(&pool)
        .await?;

        let last_seen_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO users (id, last_menu_id, current_context, last_seen_at) VALUES (?, ?, ?, ?)",
        )
        .bind(42_i64)
        .bind(7_i32)
        .bind("Home")
        .bind(last_seen_at)
        .execute(&pool)
        .await?;

        assert_eq!(get_all_active_sessions(&pool).await?.len(), 1);

        clear_user_session(42, &pool).await?;

        assert!(get_all_active_sessions(&pool).await?.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn user_language_can_be_saved_and_cycled() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                language TEXT
            )
            "#,
        )
        .execute(&pool)
        .await?;

        assert_eq!(get_user_language(42, &pool).await?, None);

        add_user(7, &pool).await?;
        assert_eq!(get_user_language(7, &pool).await?, None);

        set_user_language(42, Language::En, &pool).await?;
        assert_eq!(get_user_language(42, &pool).await?, Some(Language::En));

        let next = cycle_user_language(42, Language::Ru, &pool).await?;
        assert_eq!(next, Language::Ru);
        assert_eq!(get_user_language(42, &pool).await?, Some(Language::Ru));

        let next = cycle_user_language(7, Language::En, &pool).await?;
        assert_eq!(next, Language::Ru);
        assert_eq!(get_user_language(7, &pool).await?, Some(Language::Ru));

        Ok(())
    }
}
