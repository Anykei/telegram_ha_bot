use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

pub const HEALTH_STATE_UNKNOWN: &str = "unknown";
pub const HEALTH_STATE_HEALTHY: &str = "healthy";
pub const HEALTH_STATE_DEGRADED: &str = "degraded";
pub const HEALTH_ALERT_KIND_DEGRADED: &str = "degraded";
pub const HEALTH_ALERT_KIND_RECOVERY: &str = "recovery";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraHealthOperation {
    Snapshot,
    Clip,
    Recording,
    HealthCheck,
}

impl CameraHealthOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Clip => "clip",
            Self::Recording => "recording",
            Self::HealthCheck => "health_check",
        }
    }

    fn ok_column(self) -> &'static str {
        match self {
            Self::Snapshot | Self::HealthCheck => "last_snapshot_ok_at",
            Self::Clip => "last_clip_ok_at",
            Self::Recording => "last_recording_ok_at",
        }
    }
}

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
    pub consecutive_failures: i64,
    pub consecutive_successes: i64,
    pub health_state: String,
    pub last_state_changed_at: Option<DateTime<Utc>>,
    pub degraded_at: Option<DateTime<Utc>>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub degradation_alert_sent_at: Option<DateTime<Utc>>,
    pub recovery_alert_sent_at: Option<DateTime<Utc>>,
    pub last_failure_kind: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct PendingHealthAlert {
    pub camera_id: i64,
    pub health_state: String,
    pub last_error: Option<String>,
    pub last_failure_kind: Option<String>,
    pub consecutive_failures: i64,
    pub degraded_at: Option<DateTime<Utc>>,
    pub recovered_at: Option<DateTime<Utc>>,
}

impl PendingHealthAlert {
    pub fn alert_kind(&self) -> &'static str {
        if self.health_state == HEALTH_STATE_DEGRADED {
            HEALTH_ALERT_KIND_DEGRADED
        } else {
            HEALTH_ALERT_KIND_RECOVERY
        }
    }

    pub fn state_changed_at(&self) -> Option<DateTime<Utc>> {
        if self.health_state == HEALTH_STATE_DEGRADED {
            self.degraded_at
        } else {
            self.recovered_at
        }
    }
}

pub async fn get(camera_id: i64, pool: &SqlitePool) -> Result<Option<CameraHealth>> {
    Ok(sqlx::query_as::<_, CameraHealth>(
        r#"
        SELECT camera_id, last_snapshot_ok_at, last_clip_ok_at, last_recording_ok_at,
               last_check_at, last_error, last_file_size, updated_at,
               consecutive_failures, consecutive_successes, health_state,
               last_state_changed_at, degraded_at, recovered_at,
               degradation_alert_sent_at, recovery_alert_sent_at, last_failure_kind
        FROM camera_health
        WHERE camera_id = ?
        "#,
    )
    .bind(camera_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn mark_snapshot_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    mark_ok(
        camera_id,
        CameraHealthOperation::Snapshot,
        Some(size_bytes),
        pool,
    )
    .await
}

pub async fn mark_clip_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    mark_ok(
        camera_id,
        CameraHealthOperation::Clip,
        Some(size_bytes),
        pool,
    )
    .await
}

pub async fn mark_recording_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    mark_ok(
        camera_id,
        CameraHealthOperation::Recording,
        Some(size_bytes),
        pool,
    )
    .await
}

pub async fn mark_check_ok(camera_id: i64, size_bytes: i64, pool: &SqlitePool) -> Result<()> {
    mark_ok(
        camera_id,
        CameraHealthOperation::HealthCheck,
        Some(size_bytes),
        pool,
    )
    .await
}

pub async fn mark_error(camera_id: i64, error: &str, pool: &SqlitePool) -> Result<()> {
    mark_error_kind(camera_id, CameraHealthOperation::HealthCheck, error, pool).await
}

pub async fn mark_error_kind(
    camera_id: i64,
    kind: CameraHealthOperation,
    error: &str,
    pool: &SqlitePool,
) -> Result<()> {
    let now = Utc::now();
    let current = get(camera_id, pool).await?;
    let previous_state = current
        .as_ref()
        .map(|health| health.health_state.as_str())
        .unwrap_or(HEALTH_STATE_UNKNOWN);
    let consecutive_failures = current
        .as_ref()
        .map(|health| health.consecutive_failures)
        .unwrap_or(0)
        + 1;
    let failure_threshold =
        crate::db::settings::get_i64(crate::db::settings::CAMERA_HEALTH_FAILURE_THRESHOLD, pool)
            .await?
            .unwrap_or(3)
            .max(1);

    let next_state = if consecutive_failures >= failure_threshold {
        HEALTH_STATE_DEGRADED.to_string()
    } else {
        previous_state.to_string()
    };
    let became_degraded =
        previous_state != HEALTH_STATE_DEGRADED && next_state == HEALTH_STATE_DEGRADED;
    let degraded_at = if became_degraded {
        Some(now)
    } else {
        current.as_ref().and_then(|health| health.degraded_at)
    };
    let last_state_changed_at = if became_degraded {
        Some(now)
    } else {
        current
            .as_ref()
            .and_then(|health| health.last_state_changed_at)
    };
    let degradation_alert_sent_at = if became_degraded {
        None
    } else {
        current
            .as_ref()
            .and_then(|health| health.degradation_alert_sent_at)
    };

    upsert_health(
        HealthUpsert {
            camera_id,
            ok_column: None,
            last_check_at: Some(now),
            last_error: Some(crate::db::sanitize_error(error)),
            last_file_size: current.as_ref().and_then(|health| health.last_file_size),
            updated_at: now,
            consecutive_failures,
            consecutive_successes: 0,
            health_state: next_state,
            last_state_changed_at,
            degraded_at,
            recovered_at: current.as_ref().and_then(|health| health.recovered_at),
            degradation_alert_sent_at,
            recovery_alert_sent_at: current
                .as_ref()
                .and_then(|health| health.recovery_alert_sent_at),
            last_failure_kind: Some(kind.as_str().to_string()),
        },
        pool,
    )
    .await
}

pub async fn list_pending_alerts(pool: &SqlitePool) -> Result<Vec<PendingHealthAlert>> {
    Ok(sqlx::query_as::<_, PendingHealthAlert>(
        r#"
        SELECT camera_id, health_state, last_error, last_failure_kind,
               consecutive_failures, degraded_at, recovered_at
        FROM camera_health
        WHERE (health_state = 'degraded'
               AND degraded_at IS NOT NULL
               AND degradation_alert_sent_at IS NULL)
           OR (health_state = 'healthy'
               AND recovered_at IS NOT NULL
               AND recovery_alert_sent_at IS NULL)
        ORDER BY updated_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn mark_degradation_alert_sent(camera_id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE camera_health SET degradation_alert_sent_at = ? WHERE camera_id = ?")
        .bind(Utc::now())
        .bind(camera_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_recovery_alert_sent(camera_id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE camera_health SET recovery_alert_sent_at = ? WHERE camera_id = ?")
        .bind(Utc::now())
        .bind(camera_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn alert_delivery_sent(
    camera_id: i64,
    alert_kind: &str,
    recipient_id: i64,
    state_changed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    let sent_at: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        r#"
        SELECT sent_at
        FROM camera_health_alert_delivery
        WHERE camera_id = ?
          AND alert_kind = ?
          AND recipient_id = ?
          AND state_changed_at = ?
        "#,
    )
    .bind(camera_id)
    .bind(alert_kind)
    .bind(recipient_id)
    .bind(state_changed_at)
    .fetch_optional(pool)
    .await?;

    Ok(sent_at.flatten().is_some())
}

pub async fn mark_alert_delivery_sent(
    camera_id: i64,
    alert_kind: &str,
    recipient_id: i64,
    state_changed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO camera_health_alert_delivery (
            camera_id, alert_kind, recipient_id, state_changed_at, sent_at, last_error, updated_at
        )
        VALUES (?, ?, ?, ?, ?, NULL, ?)
        ON CONFLICT(camera_id, alert_kind, recipient_id, state_changed_at) DO UPDATE SET
            sent_at = excluded.sent_at,
            last_error = NULL,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(camera_id)
    .bind(alert_kind)
    .bind(recipient_id)
    .bind(state_changed_at)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_alert_delivery_error(
    camera_id: i64,
    alert_kind: &str,
    recipient_id: i64,
    state_changed_at: DateTime<Utc>,
    error: &str,
    pool: &SqlitePool,
) -> Result<()> {
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO camera_health_alert_delivery (
            camera_id, alert_kind, recipient_id, state_changed_at, sent_at, last_error, updated_at
        )
        VALUES (?, ?, ?, ?, NULL, ?, ?)
        ON CONFLICT(camera_id, alert_kind, recipient_id, state_changed_at) DO UPDATE SET
            last_error = excluded.last_error,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(camera_id)
    .bind(alert_kind)
    .bind(recipient_id)
    .bind(state_changed_at)
    .bind(crate::db::sanitize_error(error))
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

async fn mark_ok(
    camera_id: i64,
    kind: CameraHealthOperation,
    size_bytes: Option<i64>,
    pool: &SqlitePool,
) -> Result<()> {
    let now = Utc::now();
    let current = get(camera_id, pool).await?;
    let previous_state = current
        .as_ref()
        .map(|health| health.health_state.as_str())
        .unwrap_or(HEALTH_STATE_UNKNOWN);
    let recovery_successes =
        crate::db::settings::get_i64(crate::db::settings::CAMERA_HEALTH_RECOVERY_SUCCESSES, pool)
            .await?
            .unwrap_or(1)
            .max(1);
    let consecutive_successes = if previous_state == HEALTH_STATE_DEGRADED {
        current
            .as_ref()
            .map(|health| health.consecutive_successes)
            .unwrap_or(0)
            + 1
    } else {
        0
    };
    let recovered =
        previous_state == HEALTH_STATE_DEGRADED && consecutive_successes >= recovery_successes;
    let first_healthy = previous_state == HEALTH_STATE_UNKNOWN;
    let next_state = if recovered || first_healthy {
        HEALTH_STATE_HEALTHY.to_string()
    } else {
        previous_state.to_string()
    };
    let last_state_changed_at = if recovered || first_healthy {
        Some(now)
    } else {
        current
            .as_ref()
            .and_then(|health| health.last_state_changed_at)
    };
    let recovered_at = if recovered {
        Some(now)
    } else {
        current.as_ref().and_then(|health| health.recovered_at)
    };

    upsert_health(
        HealthUpsert {
            camera_id,
            ok_column: Some(kind.ok_column()),
            last_check_at: Some(now),
            last_error: None,
            last_file_size: size_bytes,
            updated_at: now,
            consecutive_failures: 0,
            consecutive_successes,
            health_state: next_state,
            last_state_changed_at,
            degraded_at: current.as_ref().and_then(|health| health.degraded_at),
            recovered_at,
            degradation_alert_sent_at: current
                .as_ref()
                .and_then(|health| health.degradation_alert_sent_at),
            recovery_alert_sent_at: if recovered {
                None
            } else {
                current
                    .as_ref()
                    .and_then(|health| health.recovery_alert_sent_at)
            },
            last_failure_kind: current
                .as_ref()
                .and_then(|health| health.last_failure_kind.clone()),
        },
        pool,
    )
    .await
}

struct HealthUpsert {
    camera_id: i64,
    ok_column: Option<&'static str>,
    last_check_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    last_file_size: Option<i64>,
    updated_at: DateTime<Utc>,
    consecutive_failures: i64,
    consecutive_successes: i64,
    health_state: String,
    last_state_changed_at: Option<DateTime<Utc>>,
    degraded_at: Option<DateTime<Utc>>,
    recovered_at: Option<DateTime<Utc>>,
    degradation_alert_sent_at: Option<DateTime<Utc>>,
    recovery_alert_sent_at: Option<DateTime<Utc>>,
    last_failure_kind: Option<String>,
}

async fn upsert_health(upsert: HealthUpsert, pool: &SqlitePool) -> Result<()> {
    let (insert_ok_column, insert_ok_value, update_ok_column) = match upsert.ok_column {
        Some(column) => (
            format!(", {column}"),
            ", ?".to_string(),
            format!("{column} = excluded.{column},"),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    let sql = format!(
        r#"
        INSERT INTO camera_health (
            camera_id{insert_ok_column}, last_check_at, last_error, last_file_size,
            updated_at, consecutive_failures, consecutive_successes, health_state,
            last_state_changed_at, degraded_at, recovered_at, degradation_alert_sent_at,
            recovery_alert_sent_at, last_failure_kind
        )
        VALUES (?{insert_ok_value}, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(camera_id) DO UPDATE SET
            {update_ok_column}
            last_check_at = excluded.last_check_at,
            last_error = excluded.last_error,
            last_file_size = excluded.last_file_size,
            updated_at = excluded.updated_at,
            consecutive_failures = excluded.consecutive_failures,
            consecutive_successes = excluded.consecutive_successes,
            health_state = excluded.health_state,
            last_state_changed_at = excluded.last_state_changed_at,
            degraded_at = excluded.degraded_at,
            recovered_at = excluded.recovered_at,
            degradation_alert_sent_at = excluded.degradation_alert_sent_at,
            recovery_alert_sent_at = excluded.recovery_alert_sent_at,
            last_failure_kind = excluded.last_failure_kind
        "#
    );

    let mut query = sqlx::query(&sql).bind(upsert.camera_id);
    if upsert.ok_column.is_some() {
        query = query.bind(upsert.updated_at);
    }
    query
        .bind(upsert.last_check_at)
        .bind(upsert.last_error)
        .bind(upsert.last_file_size)
        .bind(upsert.updated_at)
        .bind(upsert.consecutive_failures)
        .bind(upsert.consecutive_successes)
        .bind(upsert.health_state)
        .bind(upsert.last_state_changed_at)
        .bind(upsert.degraded_at)
        .bind(upsert.recovered_at)
        .bind(upsert.degradation_alert_sent_at)
        .bind(upsert.recovery_alert_sent_at)
        .bind(upsert.last_failure_kind)
        .execute(pool)
        .await?;

    Ok(())
}
