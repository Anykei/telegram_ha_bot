use anyhow::Result;
use sqlx::SqlitePool;

pub const UI_BACKGROUND_CAMERA_ID: &str = "ui_background_camera_id";
pub const UI_BACKGROUND_REFRESH_S: &str = "ui_background_refresh_s";
#[allow(dead_code)]
pub const CAMERA_RECORDING_DEFAULT_TAIL_SECONDS: &str = "camera_recording_default_tail_seconds";
#[allow(dead_code)]
pub const CAMERA_RECORDING_DEFAULT_MAX_SEGMENT_SECONDS: &str =
    "camera_recording_default_max_segment_seconds";
#[allow(dead_code)]
pub const CAMERA_RECORDING_DEFAULT_COOLDOWN_S: &str = "camera_recording_default_cooldown_s";
pub const CAMERA_RECORDING_DEFAULT_RETENTION_DAYS: &str = "camera_recording_default_retention_days";
pub const CAMERA_RECORDING_MAX_STORAGE_MB: &str = "camera_recording_max_storage_mb";
pub const NOTIFICATION_NOISE_WINDOW_S: &str = "notification_noise_window_s";
pub const NOTIFICATION_NOISE_THRESHOLD: &str = "notification_noise_threshold";
pub const NOTIFICATION_SUMMARY_COOLDOWN_S: &str = "notification_summary_cooldown_s";
pub const ACTIVITY_LOG_RETENTION_DAYS: &str = "activity_log_retention_days";
pub const CAMERA_HEALTH_CHECK_ENABLED: &str = "camera_health_check_enabled";
pub const CAMERA_HEALTH_CHECK_INTERVAL_S: &str = "camera_health_check_interval_s";
pub const CAMERA_HEALTH_CHECK_BATCH_SIZE: &str = "camera_health_check_batch_size";
pub const CAMERA_HEALTH_FAILURE_THRESHOLD: &str = "camera_health_failure_threshold";
pub const CAMERA_HEALTH_RECOVERY_SUCCESSES: &str = "camera_health_recovery_successes";
#[allow(dead_code)]
pub const CAMERA_HEALTH_ALERT_COOLDOWN_S: &str = "camera_health_alert_cooldown_s";
#[allow(dead_code)]
pub const CAMERA_PRE_ROLL_ENABLED: &str = "camera_pre_roll_enabled";
#[allow(dead_code)]
pub const CAMERA_PRE_ROLL_MAX_BUFFER_BYTES_PER_CAMERA: &str =
    "camera_pre_roll_max_buffer_bytes_per_camera";
#[allow(dead_code)]
pub const CAMERA_PRE_ROLL_MAX_TOTAL_BUFFER_BYTES: &str = "camera_pre_roll_max_total_buffer_bytes";

pub async fn get_string(key: &str, pool: &SqlitePool) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn set_string(key: &str, value: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO app_settings (key, value)
        VALUES (?, ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete(key: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM app_settings WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_i64(key: &str, pool: &SqlitePool) -> Result<Option<i64>> {
    Ok(get_string(key, pool)
        .await?
        .and_then(|value| value.parse().ok()))
}

pub async fn set_i64(key: &str, value: i64, pool: &SqlitePool) -> Result<()> {
    set_string(key, &value.to_string(), pool).await
}

pub async fn get_u32_or(key: &str, default: u32, pool: &SqlitePool) -> u32 {
    get_string(key, pool)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default)
}
