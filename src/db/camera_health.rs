use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct CameraHealth {
    pub camera_id: i64,
    pub last_snapshot_ok_at: Option<DateTime<Utc>>,
    pub last_clip_ok_at: Option<DateTime<Utc>>,
    pub last_recording_ok_at: Option<DateTime<Utc>>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_file_size: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

pub async fn get(camera_id: i64, pool: &SqlitePool) -> Result<Option<CameraHealth>> {
    Ok(sqlx::query_as::<_, CameraHealth>(
        r#"
        SELECT camera_id, last_snapshot_ok_at, last_clip_ok_at, last_recording_ok_at,
               last_check_at, last_error, last_file_size, updated_at
        FROM camera_health
        WHERE camera_id = ?
        "#,
    )
    .bind(camera_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn mark_snapshot_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    upsert_ok(camera_id, "last_snapshot_ok_at", Some(size_bytes), pool).await
}

pub async fn mark_clip_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    upsert_ok(camera_id, "last_clip_ok_at", Some(size_bytes), pool).await
}

pub async fn mark_recording_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    upsert_ok(camera_id, "last_recording_ok_at", Some(size_bytes), pool).await
}

pub async fn mark_check_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO camera_health (
            camera_id, last_snapshot_ok_at, last_check_at, last_error, last_file_size, updated_at
        )
        VALUES (?, ?, ?, NULL, ?, ?)
        ON CONFLICT(camera_id) DO UPDATE SET
            last_snapshot_ok_at = excluded.last_snapshot_ok_at,
            last_check_at = excluded.last_check_at,
            last_error = NULL,
            last_file_size = excluded.last_file_size,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(camera_id)
    .bind(now)
    .bind(now)
    .bind(size_bytes)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_error(camera_id: i64, error: &str, pool: &SqlitePool) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO camera_health (camera_id, last_check_at, last_error, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(camera_id) DO UPDATE SET
            last_check_at = excluded.last_check_at,
            last_error = excluded.last_error,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(camera_id)
    .bind(now)
    .bind(crate::db::sanitize_error(error))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

async fn upsert_ok(
    camera_id: i64,
    timestamp_column: &str,
    size_bytes: Option<i64>,
    pool: &SqlitePool,
) -> Result<()> {
    let now = Utc::now();
    let sql = format!(
        r#"
        INSERT INTO camera_health (
            camera_id, {timestamp_column}, last_error, last_file_size, updated_at
        )
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(camera_id) DO UPDATE SET
            {timestamp_column} = excluded.{timestamp_column},
            last_error = NULL,
            last_file_size = excluded.last_file_size,
            updated_at = excluded.updated_at
        "#
    );

    sqlx::query(&sql)
        .bind(camera_id)
        .bind(now)
        .bind(size_bytes)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}
