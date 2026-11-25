use crate::db::models::AggregatedAlert;

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;

pub struct EventLogger;

impl EventLogger {
    pub async fn record_event(eid: &str, state: &str, pool: &SqlitePool) -> Result<()> {
        let query = sqlx::query(
            "INSERT INTO device_event_log (entity_id, state, created_at) VALUES (?, ?, ?)",
        )
        .bind(eid)
        .bind(state)
        .bind(Utc::now());
        crate::db::log_slow_operation("device_event_log.record_event", query.execute(pool)).await?;
        Ok(())
    }

    pub async fn fetch_active_alerts(
        user_id: u64,
        window_mins: u64,
        pool: &SqlitePool,
    ) -> Result<Vec<AggregatedAlert>> {
        let now = Utc::now();
        let limit = now - chrono::Duration::minutes(window_mins as i64);

        let rows = sqlx::query_as::<sqlx::Sqlite, AggregatedAlert>(
            r#"
        SELECT
            log.entity_id,
            log.state as last_state,
            COUNT(*) as event_count,
            MAX(log.created_at) as last_updated
        FROM device_event_log as log
        JOIN subscriptions as sub ON log.entity_id = sub.entity_id
        WHERE sub.user_id = ?
          AND DATETIME(log.created_at) >= DATETIME(?)
        GROUP BY log.entity_id
        ORDER BY last_updated DESC
        "#,
        )
        .bind(user_id as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        let mut visible = Vec::new();
        for row in rows {
            if crate::db::access::can_notify_entity(user_id, &row.entity_id, pool).await? {
                visible.push(row);
            }
        }
        Ok(visible)
    }

    pub async fn purge_old_events(minutes: u64, pool: &SqlitePool) -> Result<u64> {
        let horizon = Utc::now() - chrono::Duration::minutes(minutes as i64);
        // Even an empty DELETE waits for SQLite's writer. Leave idle cleanup
        // read-only and keep the same timestamp comparison for legacy rows.
        let expired: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM device_event_log WHERE DATETIME(created_at) < DATETIME(?))",
        )
        .bind(horizon)
        .fetch_one(pool)
        .await?;
        if !expired {
            return Ok(0);
        }

        let result =
            sqlx::query("DELETE FROM device_event_log WHERE DATETIME(created_at) < DATETIME(?)")
                .bind(horizon)
                .execute(pool)
                .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::time::Duration;

    #[tokio::test]
    async fn event_purge_handles_mixed_timestamps_and_skips_idle_writer_lock() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "ha-event-purge-{}-{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query(
            "INSERT INTO device_event_log (entity_id, state, created_at) VALUES
                ('legacy-old', 'on', DATETIME('now', '-2 hours')),
                ('legacy-fresh', 'on', DATETIME('now')),
                ('sqlx-old', 'on', ?)",
        )
        .bind(Utc::now() - chrono::Duration::hours(2))
        .execute(&pool)
        .await?;
        EventLogger::record_event("sqlx-fresh", "on", &pool).await?;

        assert_eq!(EventLogger::purge_old_events(60, &pool).await?, 2);
        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT entity_id FROM device_event_log ORDER BY entity_id")
                .fetch_all(&pool)
                .await?;
        assert_eq!(remaining, ["legacy-fresh", "sqlx-fresh"]);

        let writer = pool.begin_with("BEGIN IMMEDIATE").await?;
        let idle = tokio::time::timeout(
            Duration::from_secs(2),
            EventLogger::purge_old_events(60, &pool),
        )
        .await;
        writer.rollback().await?;
        assert_eq!(idle??, 0);

        pool.close().await;
        std::fs::remove_file(path)?;
        Ok(())
    }
}
