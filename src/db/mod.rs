pub(crate) mod access;
pub(crate) mod activity_log;
pub(crate) mod camera_health;
pub(crate) mod camera_recording_rule_conditions;
pub(crate) mod camera_recording_rule_groups;
pub(crate) mod camera_recording_rules;
pub(crate) mod camera_recording_segments;
pub(crate) mod camera_recording_sessions;
pub(crate) mod cameras;
pub(crate) mod device_event_log;
pub(crate) mod devices;
mod models;
pub(crate) mod pending_commands;
pub(crate) mod rooms;
pub(crate) mod settings;
pub(crate) mod subscriptions;
mod user;

use anyhow::{anyhow, Context, Result};
use log::info;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

pub use user::*;

const SLOW_SQLITE_OPERATION_WARNING: Duration = Duration::from_secs(2);

pub async fn init(db_url: &str, migration_path: &str) -> Result<SqlitePool> {
    prepare_db_dir(db_url).context("Error preparing db dir")?;

    let opts = SqliteConnectOptions::from_str(db_url)
        .context("Unsupported format DATABASE_URL")?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30));

    let pool = SqlitePool::connect_with(opts)
        .await
        .context("Error connecting to database")?;

    let migrations_dir = Path::new(migration_path);
    if migrations_dir.exists() {
        let migrator = Migrator::new(migrations_dir)
            .await
            .with_context(|| format!("Error init migration: {:?}", migrations_dir))?;

        migrator
            .run(&pool)
            .await
            .context("Error running migrations")?;
        info!("Migrations applied.");
    } else {
        log::warn!("Migration folder missing {:?}. check env.", migrations_dir);
    }

    Ok(pool)
}

pub(crate) fn sanitize_error(error: &str) -> String {
    let mut value = error.replace('\n', " ");
    if value.len() > 500 {
        value.truncate(500);
    }
    value
}

pub(crate) async fn log_slow_operation<T, F>(operation: &'static str, future: F) -> T
where
    F: Future<Output = T>,
{
    let started_at = Instant::now();
    let result = future.await;
    let elapsed = started_at.elapsed();

    if elapsed >= SLOW_SQLITE_OPERATION_WARNING {
        log::warn!("Slow SQLite operation {} took {:?}", operation, elapsed);
    }

    result
}

fn prepare_db_dir(uri: &str) -> Result<()> {
    if let Some(path_str) = uri.strip_prefix("sqlite://") {
        let path = Path::new(path_str);

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                info!("Create DB folder: {:?}", parent);
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Fail create dir {:?}", parent))?;
            }
        }
    } else {
        return Err(anyhow!("DATABASE_URL start with 'sqlite://'"));
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct SystemStats {
    pub users: i64,
    pub rooms: i64,
    pub active_devices: i64,
    pub archived_devices: i64,
    pub subscriptions: i64,
    pub recent_events: i64,
}

pub async fn get_system_stats(pool: &SqlitePool) -> Result<SystemStats> {
    Ok(SystemStats {
        users: count_query("SELECT COUNT(*) FROM users", pool).await?,
        rooms: count_query("SELECT COUNT(*) FROM rooms WHERE hide = 0", pool).await?,
        active_devices: count_query("SELECT COUNT(*) FROM devices WHERE archived = 0", pool)
            .await?,
        archived_devices: count_query("SELECT COUNT(*) FROM devices WHERE archived != 0", pool)
            .await?,
        subscriptions: count_query("SELECT COUNT(*) FROM subscriptions", pool).await?,
        recent_events: count_query("SELECT COUNT(*) FROM device_event_log", pool).await?,
    })
}

async fn count_query(sql: &str, pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await?)
}

pub async fn create_timestamped_backup(pool: &SqlitePool) -> Result<PathBuf> {
    let backup_dir = PathBuf::from("backups");
    std::fs::create_dir_all(&backup_dir)
        .with_context(|| format!("Fail create backup dir {:?}", backup_dir))?;

    let filename = format!(
        "bot_data_{}.db",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    );
    let path = backup_dir.join(filename);
    create_backup(path.to_string_lossy().as_ref(), pool).await?;
    Ok(path)
}

pub async fn create_backup(backup_path: &str, pool: &SqlitePool) -> Result<()> {
    let _ = std::fs::remove_file(backup_path);

    let escaped_path = backup_path.replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{}'", escaped_path))
        .execute(pool)
        .await
        .context("DB backup create error")?;

    info!("✅ DB bacup successful: {}", backup_path);
    Ok(())
}

pub type StateMap = std::collections::HashMap<String, std::collections::HashMap<String, String>>;
pub async fn get_state_aliases(pool: &SqlitePool) -> StateMap {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT entity_id, original_state, human_state FROM state_aliases")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    let mut map = std::collections::HashMap::new();
    for (eid, orig, human) in rows {
        map.entry(eid)
            .or_insert_with(std::collections::HashMap::new)
            .insert(orig, human);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn system_stats_counts_core_tables() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await?;
        sqlx::query("CREATE TABLE rooms (id INTEGER PRIMARY KEY, hide INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool)
            .await?;
        sqlx::query(
            "CREATE TABLE devices (id INTEGER PRIMARY KEY, archived INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE TABLE subscriptions (user_id INTEGER, entity_id TEXT)")
            .execute(&pool)
            .await?;
        sqlx::query("CREATE TABLE device_event_log (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await?;

        sqlx::query("INSERT INTO users (id) VALUES (1), (2)")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO rooms (id, hide) VALUES (1, 0), (2, 1)")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO devices (id, archived) VALUES (1, 0), (2, 1)")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO subscriptions (user_id, entity_id) VALUES (1, 'sensor.a')")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO device_event_log (id) VALUES (1), (2), (3)")
            .execute(&pool)
            .await?;

        let stats = get_system_stats(&pool).await?;

        assert_eq!(stats.users, 2);
        assert_eq!(stats.rooms, 1);
        assert_eq!(stats.active_devices, 1);
        assert_eq!(stats.archived_devices, 1);
        assert_eq!(stats.subscriptions, 1);
        assert_eq!(stats.recent_events, 3);

        Ok(())
    }
}
