use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

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
    get_on_connection(camera_id, &mut *pool.acquire().await?).await
}

async fn get_on_connection(
    camera_id: i64,
    connection: &mut SqliteConnection,
) -> Result<Option<CameraHealth>> {
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
    .fetch_optional(connection)
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
    let failure_threshold =
        crate::db::settings::get_i64(crate::db::settings::CAMERA_HEALTH_FAILURE_THRESHOLD, pool)
            .await?
            .unwrap_or(3)
            .max(1);
    // Serialize the entire transition with other observations and alert acks.
    // Reading before the write lock loses concurrent failures or restores old acks.
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = get_on_connection(camera_id, &mut transaction).await?;
    let now = next_observation_time(Utc::now(), current.as_ref())?;
    let previous_state = current
        .as_ref()
        .map(|health| health.health_state.as_str())
        .unwrap_or(HEALTH_STATE_UNKNOWN);
    let consecutive_failures = current
        .as_ref()
        .map(|health| health.consecutive_failures)
        .unwrap_or(0)
        .saturating_add(1);

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
        &mut transaction,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
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

pub async fn mark_degradation_alert_sent(
    camera_id: i64,
    degraded_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    Ok(sqlx::query("UPDATE camera_health SET degradation_alert_sent_at = ? WHERE camera_id = ? AND health_state = 'degraded' AND degraded_at = ? AND degradation_alert_sent_at IS NULL")
        .bind(Utc::now())
        .bind(camera_id)
        .bind(degraded_at)
        .execute(pool)
        .await?.rows_affected() == 1)
}

pub async fn mark_recovery_alert_sent(
    camera_id: i64,
    recovered_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    Ok(sqlx::query("UPDATE camera_health SET recovery_alert_sent_at = ? WHERE camera_id = ? AND health_state = 'healthy' AND recovered_at = ? AND recovery_alert_sent_at IS NULL")
        .bind(Utc::now())
        .bind(camera_id)
        .bind(recovered_at)
        .execute(pool)
        .await?.rows_affected() == 1)
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
    let recovery_successes =
        crate::db::settings::get_i64(crate::db::settings::CAMERA_HEALTH_RECOVERY_SUCCESSES, pool)
            .await?
            .unwrap_or(1)
            .max(1);
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = get_on_connection(camera_id, &mut transaction).await?;
    let now = next_observation_time(Utc::now(), current.as_ref())?;
    let previous_state = current
        .as_ref()
        .map(|health| health.health_state.as_str())
        .unwrap_or(HEALTH_STATE_UNKNOWN);
    let consecutive_successes = if previous_state == HEALTH_STATE_DEGRADED {
        current
            .as_ref()
            .map(|health| health.consecutive_successes)
            .unwrap_or(0)
            .saturating_add(1)
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
        &mut transaction,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// The persisted transition timestamp also identifies an alert. Keep it unique
/// under the write lock even if the clock repeats a tick or moves backwards.
fn next_observation_time(
    now: DateTime<Utc>,
    current: Option<&CameraHealth>,
) -> Result<DateTime<Utc>> {
    let previous = current.and_then(|health| {
        [
            Some(health.updated_at),
            health.last_state_changed_at,
            health.degraded_at,
            health.recovered_at,
        ]
        .into_iter()
        .flatten()
        .max()
    });
    match previous {
        Some(previous) if previous >= now => previous
            .checked_add_signed(chrono::Duration::nanoseconds(1))
            .ok_or_else(|| anyhow::anyhow!("Camera health timestamp overflow")),
        _ => Ok(now),
    }
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

async fn upsert_health(upsert: HealthUpsert, connection: &mut SqliteConnection) -> Result<()> {
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

    // The optional column is an internal static name; all data stays bound.
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(upsert.camera_id);
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
        .execute(connection)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(failure_threshold: i64, recovery_successes: i64) -> Result<(SqlitePool, i64)> {
        let config = crate::test_support::config().await;
        let pool = config.db.clone();
        let id = setup_camera(&pool, failure_threshold, recovery_successes).await?;
        Ok((pool, id))
    }

    async fn setup_camera(
        pool: &SqlitePool,
        failure_threshold: i64,
        recovery_successes: i64,
    ) -> Result<i64> {
        crate::db::rooms::sync_rooms_from_ha("health", "Health", pool).await?;
        let id = crate::db::cameras::add_manual_camera(
            crate::db::cameras::NewCamera {
                name: "Health test",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 5,
            },
            pool,
        )
        .await?;
        crate::db::settings::set_i64(
            crate::db::settings::CAMERA_HEALTH_FAILURE_THRESHOLD,
            failure_threshold,
            pool,
        )
        .await?;
        crate::db::settings::set_i64(
            crate::db::settings::CAMERA_HEALTH_RECOVERY_SUCCESSES,
            recovery_successes,
            pool,
        )
        .await?;
        Ok(id)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn wal_connections_serialize_simultaneous_failures_and_recovery() -> Result<()> {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use std::sync::Arc;
        use std::time::Duration;

        struct TestDirectory(std::path::PathBuf);
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "camera-health-wal-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&directory)?;
        let directory = TestDirectory(directory);
        let pool = SqlitePoolOptions::new()
            .min_connections(3)
            .max_connections(3)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(directory.0.join("health.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        assert_eq!(pool.size(), 3);
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
                .fetch_one(&pool)
                .await?,
            "wal"
        );
        let id = setup_camera(&pool, 3, 3).await?;
        mark_snapshot_ok(id, 111, &pool).await?;
        let initial = get(id, &pool).await?.unwrap();
        let mut degraded_at = None;

        for success in [false, true] {
            let barrier = Arc::new(tokio::sync::Barrier::new(4));
            let mut jobs = tokio::task::JoinSet::new();
            for (index, kind) in [
                CameraHealthOperation::Snapshot,
                CameraHealthOperation::Clip,
                CameraHealthOperation::Recording,
            ]
            .into_iter()
            .enumerate()
            {
                let pool = pool.clone();
                let barrier = barrier.clone();
                jobs.spawn(async move {
                    barrier.wait().await;
                    if success {
                        mark_ok(id, kind, Some(222 + index as i64 * 111), &pool).await
                    } else {
                        mark_error_kind(id, kind, "concurrent WAL failure", &pool).await
                    }
                });
            }
            tokio::time::timeout(Duration::from_secs(10), async {
                barrier.wait().await;
                let mut completed = 0;
                while let Some(result) = jobs.join_next().await {
                    result??;
                    completed += 1;
                }
                assert_eq!(completed, 3);
                Ok::<_, anyhow::Error>(())
            })
            .await??;

            let health = get(id, &pool).await?.unwrap();
            if success {
                assert_eq!(health.consecutive_failures, 0);
                assert_eq!(health.consecutive_successes, 3);
                assert_eq!(health.health_state, HEALTH_STATE_HEALTHY);
                assert_eq!(health.degraded_at, degraded_at);
                assert!(health.recovered_at > degraded_at);
                assert!(health.last_snapshot_ok_at > degraded_at);
                assert!(health.last_clip_ok_at > degraded_at);
                assert!(health.last_recording_ok_at > degraded_at);
                assert!(health.last_error.is_none());
                assert!(health.degradation_alert_sent_at.is_some());
                assert!(health.recovery_alert_sent_at.is_none());
                assert!(matches!(health.last_file_size, Some(222 | 333 | 444)));
                assert_eq!(
                    list_pending_alerts(&pool).await?[0].alert_kind(),
                    HEALTH_ALERT_KIND_RECOVERY
                );
            } else {
                assert_eq!(health.consecutive_failures, 3);
                assert_eq!(health.consecutive_successes, 0);
                assert_eq!(health.health_state, HEALTH_STATE_DEGRADED);
                assert_eq!(health.last_file_size, Some(111));
                assert_eq!(health.last_snapshot_ok_at, initial.last_snapshot_ok_at);
                assert_eq!(list_pending_alerts(&pool).await?.len(), 1);
                degraded_at = health.degraded_at;
                assert!(mark_degradation_alert_sent(id, degraded_at.unwrap(), &pool).await?);
            }
        }
        pool.close().await;
        drop(directory);
        Ok(())
    }

    #[tokio::test]
    async fn parallel_failures_and_recovery_preserve_each_observation() -> Result<()> {
        let (pool, id) = fixture(3, 3).await?;
        mark_snapshot_ok(id, 111, &pool).await?;
        let initial = get(id, &pool).await?.unwrap();

        // Queue all operations behind the sole connection before any can read.
        // The old get/compute/upsert implementation read the same baseline three
        // times and then overwrote the counter with 1 for every operation.
        let held = pool.acquire().await?;
        let mut failures = Box::pin(async {
            tokio::try_join!(
                mark_error_kind(
                    id,
                    CameraHealthOperation::Snapshot,
                    "snapshot failed",
                    &pool
                ),
                mark_error_kind(id, CameraHealthOperation::Clip, "clip failed", &pool),
                mark_error_kind(
                    id,
                    CameraHealthOperation::Recording,
                    "recording failed",
                    &pool
                ),
            )
        });
        assert!(futures_util::poll!(&mut failures).is_pending());
        drop(held);
        failures.await?;
        let degraded = get(id, &pool).await?.unwrap();
        assert_eq!(degraded.consecutive_failures, 3);
        assert_eq!(degraded.consecutive_successes, 0);
        assert_eq!(degraded.health_state, HEALTH_STATE_DEGRADED);
        assert_eq!(degraded.last_file_size, Some(111));
        assert_eq!(degraded.last_snapshot_ok_at, initial.last_snapshot_ok_at);
        assert_eq!(list_pending_alerts(&pool).await?.len(), 1);
        let transition = degraded.degraded_at.unwrap();

        let held = pool.acquire().await?;
        let mut successes = Box::pin(async {
            tokio::try_join!(
                mark_snapshot_ok(id, 222, &pool),
                mark_clip_ok(id, 333, &pool),
                mark_recording_ok(id, 444, &pool),
                mark_degradation_alert_sent(id, transition, &pool),
            )
        });
        assert!(futures_util::poll!(&mut successes).is_pending());
        drop(held);
        assert!(successes.await?.3);
        let recovered = get(id, &pool).await?.unwrap();
        assert_eq!(recovered.consecutive_successes, 3);
        assert_eq!(recovered.consecutive_failures, 0);
        assert_eq!(recovered.health_state, HEALTH_STATE_HEALTHY);
        assert!(recovered.last_snapshot_ok_at > initial.last_snapshot_ok_at);
        assert!(recovered.last_clip_ok_at.is_some());
        assert!(recovered.last_recording_ok_at.is_some());
        assert!(recovered.last_error.is_none());
        assert!(recovered.degradation_alert_sent_at.is_some());
        assert!(recovered.recovery_alert_sent_at.is_none());
        assert!(recovered.recovered_at > degraded.degraded_at);
        assert_eq!(recovered.degraded_at, degraded.degraded_at);
        assert_eq!(list_pending_alerts(&pool).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_success_failure_and_ack_keep_size_and_operation_metadata() -> Result<()> {
        let (pool, id) = fixture(1, 3).await?;
        mark_snapshot_ok(id, 111, &pool).await?;
        mark_error(id, "offline", &pool).await?;
        let degraded = get(id, &pool).await?.unwrap();
        let held = pool.acquire().await?;
        let mut observations = Box::pin(async {
            tokio::try_join!(
                mark_clip_ok(id, 999, &pool),
                mark_error_kind(
                    id,
                    CameraHealthOperation::Recording,
                    "recording failed",
                    &pool
                ),
                mark_degradation_alert_sent(id, degraded.degraded_at.unwrap(), &pool),
            )
        });
        assert!(futures_util::poll!(&mut observations).is_pending());
        drop(held);
        assert!(observations.await?.2);
        let health = get(id, &pool).await?.unwrap();
        assert_eq!(health.last_file_size, Some(999));
        assert_eq!(health.last_snapshot_ok_at, degraded.last_snapshot_ok_at);
        assert!(health.last_clip_ok_at.is_some());
        assert_eq!(health.health_state, HEALTH_STATE_DEGRADED);
        assert_eq!(health.degraded_at, degraded.degraded_at);
        assert!(health.degradation_alert_sent_at.is_some());
        assert!(list_pending_alerts(&pool).await?.is_empty());
        // Either serialization order is valid; the streak must describe that
        // order, and a failure must never restore the previous file size.
        assert!(matches!(
            (health.consecutive_failures, health.consecutive_successes),
            (0, 1) | (1, 0)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn transition_identity_survives_clock_rollback_and_stale_acks() -> Result<()> {
        let (pool, id) = fixture(1, 1).await?;
        mark_snapshot_ok(id, 111, &pool).await?;
        let future = Utc::now() + chrono::Duration::days(1);
        sqlx::query(
            "UPDATE camera_health SET updated_at=?,last_state_changed_at=? WHERE camera_id=?",
        )
        .bind(future)
        .bind(future)
        .bind(id)
        .execute(&pool)
        .await?;
        mark_error(id, "first failure", &pool).await?;
        let first_failure = get(id, &pool).await?.unwrap().degraded_at.unwrap();
        assert_eq!(first_failure, future + chrono::Duration::nanoseconds(1));
        mark_check_ok(id, 222, &pool).await?;
        let first_recovery = get(id, &pool).await?.unwrap().recovered_at.unwrap();
        assert_eq!(first_recovery, future + chrono::Duration::nanoseconds(2));
        mark_error(id, "second failure", &pool).await?;
        let second_failure = get(id, &pool).await?.unwrap().degraded_at.unwrap();
        assert_eq!(second_failure, future + chrono::Duration::nanoseconds(3));
        assert!(!mark_degradation_alert_sent(id, first_failure, &pool).await?);
        assert!(!mark_recovery_alert_sent(id, first_recovery, &pool).await?);
        assert_eq!(
            list_pending_alerts(&pool).await?[0].state_changed_at(),
            Some(second_failure)
        );
        assert!(mark_degradation_alert_sent(id, second_failure, &pool).await?);
        let ack = get(id, &pool).await?.unwrap().degradation_alert_sent_at;
        assert!(!mark_degradation_alert_sent(id, second_failure, &pool).await?);
        assert_eq!(
            get(id, &pool).await?.unwrap().degradation_alert_sent_at,
            ack
        );
        mark_check_ok(id, 333, &pool).await?;
        let second_recovery = get(id, &pool).await?.unwrap().recovered_at.unwrap();
        assert_eq!(second_recovery, future + chrono::Duration::nanoseconds(4));
        assert!(!mark_recovery_alert_sent(id, first_recovery, &pool).await?);
        assert_eq!(
            list_pending_alerts(&pool).await?[0].state_changed_at(),
            Some(second_recovery)
        );
        assert!(mark_recovery_alert_sent(id, second_recovery, &pool).await?);
        assert!(list_pending_alerts(&pool).await?.is_empty());
        Ok(())
    }
}
