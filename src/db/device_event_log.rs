use crate::db::models::AggregatedAlert;

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;

pub struct EventLogger;

impl EventLogger {
    pub async fn record_event(eid: &str, state: &str, pool: &SqlitePool, ) -> Result<()> {
        sqlx::query("INSERT INTO device_event_log (entity_id, state, created_at) VALUES (?, ?, ?)")
            .bind(eid)
            .bind(state)
            .bind(Utc::now())
            .execute(pool)
            .await?;
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
        "#
        )
            .bind(user_id as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?;

        Ok(rows)
    }

    pub async fn purge_old_events(minutes: u64, pool: &SqlitePool) -> Result<u64> {
        let horizon = Utc::now() - chrono::Duration::minutes(minutes as i64);
        let result = sqlx::query(
            "DELETE FROM device_event_log WHERE DATETIME(created_at) < DATETIME(?)"
        )
            .bind(horizon)
            .execute(pool)
            .await?;

        Ok(result.rows_affected())
    }
}