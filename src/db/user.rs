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
    if REQUEST_ADMISSION
        .try_with(|request| request.user_id == user_id)
        .unwrap_or(false)
    {
        return admission_generation(user_id, pool)
            .await
            .ok()
            .flatten()
            .is_some();
    }
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

pub async fn list_admin_users(root_user: u64, pool: &SqlitePool) -> Result<Vec<u64>> {
    let mut ids = vec![root_user];
    let rows = sqlx::query_as::<_, (i64,)>(
        r#"
        SELECT DISTINCT u.id
        FROM users u
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE COALESCE(u.is_admin, 0) != 0
           OR p.role = 'admin'
        ORDER BY u.id
        "#,
    )
    .fetch_all(pool)
    .await?;

    for (id,) in rows {
        let user_id = id as u64;
        if !ids.contains(&user_id) {
            ids.push(user_id);
        }
    }

    Ok(ids)
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
    let expected = REQUEST_ADMISSION
        .try_with(|request| (request.user_id == user_id).then(|| request.generation.clone()))
        .ok()
        .flatten();
    let result = sqlx::query(
        "UPDATE users SET language = ? WHERE id = ? AND (? IS NULL OR admission_generation = ?)",
    )
    .bind(language.code())
    .bind(user_id as i64)
    .bind(&expected)
    .bind(&expected)
    .execute(pool)
    .await?;
    anyhow::ensure!(result.rows_affected() == 1, "User admission revoked");

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
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_device_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM user_room_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM user_profiles WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM subscriptions WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM pinned_headers WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    for (table, owner_column) in [
        ("user_notification_access", "user_id"),
        ("legacy_device_acl_review", "user_id"),
        ("pending_commands", "user_id"),
        ("command_choices", "user_id"),
        ("user_favorites", "owner"),
        ("ui_drafts", "owner_id"),
        ("user_display_metadata", "user_id"),
        ("ui_text_pages", "user_id"),
    ] {
        // Identifiers come only from the fixed table/column pairs above.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE {owner_column} = ?"
        )))
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;
    }
    for namespace in ["list", "chart", "archive", "group-items"] {
        sqlx::query("DELETE FROM app_settings WHERE key LIKE ?")
            .bind(format!("ui:{namespace}:{user_id}:%"))
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id as i64)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
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
        "SELECT id, last_menu_id, current_context, last_seen_at FROM users WHERE private_menu_verified = 1 AND last_menu_id > 0 AND current_context != ''",
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
    generation: &str,
    pool: &SqlitePool,
) -> Result<SessionSave> {
    let result = sqlx::query(
        "UPDATE users SET last_menu_id = ?, current_context = ?, last_seen_at = COALESCE(last_seen_at, ?), private_menu_verified = 1 WHERE id = ? AND admission_generation = ?",
    )
    .bind(msg_id).bind(context).bind(last_seen_at.to_rfc3339())
    .bind(user_id as i64).bind(generation).execute(pool).await?;
    Ok(if result.rows_affected() == 1 {
        SessionSave::Saved
    } else {
        SessionSave::Revoked
    })
}

/// Only an accepted user update advances activity; redraws preserve it.
pub async fn record_user_activity(
    user_id: u64,
    generation: &str,
    at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    Ok(
        sqlx::query("UPDATE users SET last_seen_at = ? WHERE id = ? AND admission_generation = ?")
            .bind(at.to_rfc3339())
            .bind(user_id as i64)
            .bind(generation)
            .execute(pool)
            .await?
            .rows_affected()
            == 1,
    )
}

pub async fn user_activity_at(
    user_id: u64,
    generation: &str,
    pool: &SqlitePool,
) -> Result<Option<DateTime<Utc>>> {
    let value: Option<Option<String>> = sqlx::query_scalar(
        "SELECT last_seen_at FROM users WHERE id = ? AND admission_generation = ?",
    )
    .bind(user_id as i64)
    .bind(generation)
    .fetch_optional(pool)
    .await?;
    Ok(parse_saved_timestamp(value.flatten()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSave {
    Saved,
    Revoked,
}

#[derive(Clone)]
pub struct Admission {
    pub user_id: u64,
    pub generation: String,
}

tokio::task_local! { static REQUEST_ADMISSION: Admission; }

const MAX_ADMISSION_TASKS: usize = 256;

#[derive(Default)]
struct AdmissionTaskState {
    stopping: bool,
    next_id: u64,
    tasks: std::collections::HashMap<u64, tokio::task::AbortHandle>,
}

#[derive(Default)]
struct AdmissionTasks {
    state: std::sync::Mutex<AdmissionTaskState>,
    changed: tokio::sync::Notify,
}

struct AdmissionTaskGuard {
    tasks: std::sync::Arc<AdmissionTasks>,
    id: u64,
}

impl Drop for AdmissionTaskGuard {
    fn drop(&mut self) {
        self.tasks.state.lock().unwrap().tasks.remove(&self.id);
        self.tasks.changed.notify_waiters();
    }
}

impl AdmissionTasks {
    fn spawn<F>(self: &std::sync::Arc<Self>, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let mut state = self.state.lock().unwrap();
        if state.stopping || state.tasks.len() >= MAX_ADMISSION_TASKS {
            log::warn!(
                "operation=admission_task stage=enqueue result=rejected stopping={}",
                state.stopping
            );
            // Keep the generic JoinHandle API without ever polling rejected work.
            let task = tokio::spawn(async move {
                std::future::pending::<()>().await;
                future.await
            });
            task.abort();
            return task;
        }
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        let guard = AdmissionTaskGuard {
            tasks: self.clone(),
            id,
        };
        let task = tokio::spawn(async move {
            let _guard = guard;
            future.await
        });
        state.tasks.insert(id, task.abort_handle());
        task
    }

    fn stop(&self) {
        self.state.lock().unwrap().stopping = true;
    }

    fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.stopping = true;
        for task in state.tasks.values() {
            task.abort();
        }
    }

    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().unwrap().tasks.is_empty() {
                return;
            }
            changed.await;
        }
    }
}

fn admission_tasks() -> &'static std::sync::Arc<AdmissionTasks> {
    static TASKS: std::sync::OnceLock<std::sync::Arc<AdmissionTasks>> = std::sync::OnceLock::new();
    TASKS.get_or_init(Default::default)
}

pub fn stop_admission_tasks() {
    admission_tasks().stop();
}
pub fn abort_admission_tasks() {
    admission_tasks().abort();
}
pub async fn wait_admission_tasks() {
    admission_tasks().wait().await;
}

/// Tokio tasks do not inherit task-local values; retain the initiating admission
/// and track the task through graceful or forced application shutdown.
pub fn spawn_with_admission<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let admission = REQUEST_ADMISSION.try_with(Clone::clone).ok();
    admission_tasks().spawn(async move {
        match admission {
            Some(admission) => with_admission(admission, future).await,
            None => future.await,
        }
    })
}

pub async fn with_admission<F: std::future::Future>(admission: Admission, future: F) -> F::Output {
    REQUEST_ADMISSION.scope(admission, future).await
}

pub async fn admission_generation(user_id: u64, pool: &SqlitePool) -> Result<Option<String>> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT admission_generation FROM users WHERE id = ?")
            .bind(user_id as i64)
            .fetch_optional(pool)
            .await?;
    let revoked = REQUEST_ADMISSION
        .try_with(|request| {
            request.user_id == user_id && current.as_ref() != Some(&request.generation)
        })
        .unwrap_or(false);
    Ok(if revoked { None } else { current })
}

pub async fn admission_is_current(
    user_id: u64,
    generation: &str,
    pool: &SqlitePool,
) -> Result<bool> {
    Ok(admission_generation(user_id, pool).await?.as_deref() == Some(generation))
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
    async fn admission_task_tracker_stops_drains_and_waits_for_abort_cleanup() -> Result<()> {
        let tasks = std::sync::Arc::new(AdmissionTasks::default());
        let (release, wait) = tokio::sync::oneshot::channel();
        let active = tasks.spawn(async move {
            wait.await.unwrap();
        });
        tasks.stop();
        let rejected = tasks.spawn(async { panic!("Rejected task must never be polled") });
        assert!(rejected.await.unwrap_err().is_cancelled());
        assert_eq!(tasks.state.lock().unwrap().tasks.len(), 1);
        release.send(()).unwrap();
        active.await?;
        tasks.wait().await;
        assert!(tasks.state.lock().unwrap().tasks.is_empty());

        let tasks = std::sync::Arc::new(AdmissionTasks::default());
        let blocked = tasks.spawn(std::future::pending::<()>());
        tasks.abort();
        assert!(blocked.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), tasks.wait()).await?;
        Ok(())
    }

    #[tokio::test]
    async fn revoked_request_cannot_change_language_in_new_admission_or_spawned_task() -> Result<()>
    {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        add_user(42, &pool).await?;
        let generation = admission_generation(42, &pool).await?.unwrap();
        delete_user(42, &pool).await?;
        add_user(42, &pool).await?;
        with_admission(
            Admission {
                user_id: 42,
                generation,
            },
            async {
                assert!(admission_generation(42, &pool).await?.is_none());
                assert!(set_user_language(42, Language::En, &pool).await.is_err());
                let child_pool = pool.clone();
                assert!(
                    !spawn_with_admission(async move { user_exists(42, &child_pool).await })
                        .await?
                );
                Ok::<_, anyhow::Error>(())
            },
        )
        .await?;
        assert_eq!(get_user_language(42, &pool).await?, None);
        assert!(user_exists(42, &pool).await);
        Ok(())
    }

    #[tokio::test]
    async fn delayed_session_cannot_restore_revoked_or_readded_admission() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        add_user(42, &pool).await?;
        let old_generation = admission_generation(42, &pool).await?.unwrap();
        let (release, barrier) = tokio::sync::oneshot::channel::<()>();
        let task_pool = pool.clone();
        let task = tokio::spawn(async move {
            barrier.await.unwrap();
            save_user_session(42, 9, "old-form", Utc::now(), &old_generation, &task_pool).await
        });
        delete_user(42, &pool).await?;
        assert!(set_user_language(42, Language::En, &pool).await.is_err());
        assert!(!user_exists(42, &pool).await);
        add_user(42, &pool).await?;
        release.send(()).unwrap();
        assert_eq!(task.await??, SessionSave::Revoked);
        assert!(get_all_active_sessions(&pool).await?.is_empty());
        assert_eq!(get_user_language(42, &pool).await?, None);
        let fresh = admission_generation(42, &pool).await?.unwrap();
        assert_eq!(
            save_user_session(42, 10, "new", Utc::now(), &fresh, &pool).await?,
            SessionSave::Saved
        );
        assert_eq!(get_all_active_sessions(&pool).await?.len(), 1);
        delete_user(42, &pool).await?;
        assert_eq!(
            save_user_session(42, 11, "late", Utc::now(), &fresh, &pool).await?,
            SessionSave::Revoked
        );
        assert!(!user_exists(42, &pool).await);
        Ok(())
    }

    #[tokio::test]
    async fn clear_user_session_removes_session_from_active_list() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                last_menu_id INTEGER DEFAULT -1,
                current_context TEXT DEFAULT '',
                last_seen_at TEXT,
                private_menu_verified INTEGER DEFAULT 1
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
                language TEXT,
                admission_generation TEXT NOT NULL DEFAULT 'test'
            )
            "#,
        )
        .execute(&pool)
        .await?;

        assert_eq!(get_user_language(42, &pool).await?, None);

        add_user(7, &pool).await?;
        assert_eq!(get_user_language(7, &pool).await?, None);

        assert!(set_user_language(42, Language::En, &pool).await.is_err());
        assert!(!user_exists(42, &pool).await);
        add_user(42, &pool).await?;
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
