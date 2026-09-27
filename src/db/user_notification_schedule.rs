use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

pub const DEFAULT_QUIET_FROM: &str = "23:00";
pub const DEFAULT_QUIET_TO: &str = "07:00";

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct UserNotificationSchedule {
    pub user_id: i64,
    pub quiet_enabled: i64,
    pub quiet_from: Option<String>,
    pub quiet_to: Option<String>,
    pub timezone: Option<String>,
    pub critical_only: i64,
    pub updated_at: DateTime<Utc>,
}

pub async fn get(user_id: i64, pool: &SqlitePool) -> Result<Option<UserNotificationSchedule>> {
    Ok(sqlx::query_as::<_, UserNotificationSchedule>(
        r#"
        SELECT user_id, quiet_enabled, quiet_from, quiet_to, timezone,
               critical_only, updated_at
        FROM user_notification_schedule
        WHERE user_id = ?
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn toggle_quiet_hours(user_id: u64, pool: &SqlitePool) -> Result<bool> {
    let existing = get(user_id as i64, pool).await?;
    let now = Utc::now();

    if existing.is_some_and(|schedule| schedule.quiet_enabled != 0) {
        sqlx::query(
            r#"
            UPDATE user_notification_schedule
            SET quiet_enabled = 0, updated_at = ?
            WHERE user_id = ?
            "#,
        )
        .bind(now)
        .bind(user_id as i64)
        .execute(pool)
        .await?;
        return Ok(false);
    }

    sqlx::query(
        r#"
        INSERT INTO user_notification_schedule (
            user_id, quiet_enabled, quiet_from, quiet_to, timezone, critical_only, updated_at
        )
        VALUES (?, 1, ?, ?, NULL, 1, ?)
        ON CONFLICT(user_id) DO UPDATE SET
            quiet_enabled = 1,
            quiet_from = COALESCE(user_notification_schedule.quiet_from, excluded.quiet_from),
            quiet_to = COALESCE(user_notification_schedule.quiet_to, excluded.quiet_to),
            timezone = user_notification_schedule.timezone,
            critical_only = 1,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(user_id as i64)
    .bind(DEFAULT_QUIET_FROM)
    .bind(DEFAULT_QUIET_TO)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(true)
}

pub fn valid_hh_mm(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    if hours.len() != 2 || minutes.len() != 2 {
        return false;
    }
    let Ok(hours) = hours.parse::<u32>() else {
        return false;
    };
    let Ok(minutes) = minutes.parse::<u32>() else {
        return false;
    };
    hours <= 23 && minutes <= 59
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn toggle_quiet_hours_creates_default_schedule_and_disables_it() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query(
            r#"
            CREATE TABLE user_notification_schedule (
                user_id INTEGER PRIMARY KEY,
                quiet_enabled INTEGER NOT NULL DEFAULT 0,
                quiet_from TEXT,
                quiet_to TEXT,
                timezone TEXT,
                critical_only INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await?;

        assert!(toggle_quiet_hours(42, &pool).await?);
        let schedule = get(42, &pool).await?.expect("schedule");
        assert_eq!(schedule.quiet_enabled, 1);
        assert_eq!(schedule.quiet_from.as_deref(), Some(DEFAULT_QUIET_FROM));
        assert_eq!(schedule.quiet_to.as_deref(), Some(DEFAULT_QUIET_TO));
        assert_eq!(schedule.critical_only, 1);

        assert!(!toggle_quiet_hours(42, &pool).await?);
        let schedule = get(42, &pool).await?.expect("schedule");
        assert_eq!(schedule.quiet_enabled, 0);

        Ok(())
    }
}
