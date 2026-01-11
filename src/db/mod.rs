mod user;

use std::collections::HashMap;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::migrate::Migrator;
use std::str::FromStr;
use std::path::Path;
use anyhow::{Context, Result, anyhow};
use log::info;

pub use user::user_exists;

pub async fn init(db_url: &str, migration_path: &str) -> Result<SqlitePool> {

    prepare_db_dir(db_url).context("Error preparing db dir")?;

    let opts = SqliteConnectOptions::from_str(db_url)
        .context("Unsupported format DATABASE_URL")?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePool::connect_with(opts)
        .await
        .context("Error connecting to database")?;

    let migrations_dir = Path::new(migration_path);
    if migrations_dir.exists() {
        let migrator = Migrator::new(migrations_dir)
            .await
            .with_context(|| format!("Error init migration: {:?}", migrations_dir))?;

        migrator.run(&pool)
            .await
            .context("Error running migrations")?;
        info!("Migrations applied.");
    } else {
        log::warn!("Migration folder missing {:?}. check env.", migrations_dir);
    }

    Ok(pool)
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

pub async fn create_backup(pool: &sqlx::SqlitePool, backup_path: &str) -> Result<()> {
    let _ = std::fs::remove_file(backup_path);

    sqlx::query(&format!("VACUUM INTO '{}'", backup_path))
        .execute(pool)
        .await
        .context("DB backup create error")?;

    info!("✅ DB bacup successful: {}", backup_path);
    Ok(())
}

pub async fn get_subscribers(pool: &sqlx::SqlitePool, entity_id: &str) -> Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>("SELECT user_id FROM subscriptions WHERE entity_id = ?")
        .bind(entity_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn get_aliases_map(pool: &sqlx::SqlitePool) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>("SELECT entity_id, human_name FROM aliases")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub type StateMap = HashMap<String, HashMap<String, String>>;
pub async fn get_state_aliases(pool: &sqlx::SqlitePool) -> StateMap {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT entity_id, original_state, human_state FROM state_aliases")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    let mut map = HashMap::new();
    for (eid, orig, human) in rows {
        map.entry(eid).or_insert_with(HashMap::new).insert(orig, human);
    }
    map
}