use crate::db::camera_recording_rules::RecordingRule;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Created(i64),
    Extended(i64),
    SkippedCooldown,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingSession {
    pub id: i64,
    pub event_group_id: String,
    pub rule_id: i64,
    pub camera_id: i64,
    pub extended_by_rule_ids: Option<String>,
    pub trigger_summary: String,
    pub status: String,
    pub error: Option<String>,
    pub first_event_at: DateTime<Utc>,
    pub last_event_at: DateTime<Utc>,
    pub stop_after_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub notification_sent_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn start_or_extend_session(
    rule: &RecordingRule,
    event_group_id: &str,
    trigger_summary: &str,
    now: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<SessionAction> {
    crate::db::log_slow_operation("camera_recording_sessions.start_or_extend_session", async {
        start_or_extend_session_inner(rule, event_group_id, trigger_summary, now, pool).await
    })
    .await
}

async fn start_or_extend_session_inner(
    rule: &RecordingRule,
    event_group_id: &str,
    trigger_summary: &str,
    now: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<SessionAction> {
    let mut tx = pool.begin().await?;

    let active = sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE camera_id = ?
          AND deleted_at IS NULL
          AND status IN ('queued', 'recording')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(rule.camera_id)
    .fetch_optional(&mut *tx)
    .await?;

    let stop_after_at = now + Duration::seconds(rule.tail_seconds);

    if let Some(active) = active {
        let extended_ids = add_rule_id(active.extended_by_rule_ids.as_deref(), rule.id);
        let summary = append_summary(&active.trigger_summary, trigger_summary);

        sqlx::query(
            r#"
            UPDATE camera_recording_sessions
            SET last_event_at = ?,
                stop_after_at = ?,
                extended_by_rule_ids = ?,
                trigger_summary = ?
            WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(stop_after_at)
        .bind(extended_ids)
        .bind(summary)
        .bind(active.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        return Ok(SessionAction::Extended(active.id));
    }

    if let Some(last_completed_at) = rule.last_completed_at {
        let cooldown_until = last_completed_at + Duration::seconds(rule.cooldown_s);
        if cooldown_until > now {
            tx.commit().await?;
            return Ok(SessionAction::SkippedCooldown);
        }
    }

    let expires_at = now + Duration::days(rule.retention_days);
    let extended_ids = serde_json::to_string(&vec![rule.id])?;
    let result = sqlx::query(
        r#"
        INSERT INTO camera_recording_sessions (
            event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
            status, first_event_at, last_event_at, stop_after_at, expires_at
        )
        VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?)
        "#,
    )
    .bind(event_group_id)
    .bind(rule.id)
    .bind(rule.camera_id)
    .bind(extended_ids)
    .bind(trigger_summary)
    .bind(now)
    .bind(now)
    .bind(stop_after_at)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;

    let session_id = result.last_insert_rowid();
    tx.commit().await?;
    Ok(SessionAction::Created(session_id))
}

pub async fn get_session(session_id: i64, pool: &SqlitePool) -> Result<Option<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_camera_sessions(
    camera_id: i64,
    limit: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE camera_id = ?
          AND deleted_at IS NULL
          AND status != 'deleted'
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(camera_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn get_active_camera_session(
    camera_id: i64,
    pool: &SqlitePool,
) -> Result<Option<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE camera_id = ?
          AND deleted_at IS NULL
          AND status IN ('queued', 'recording')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(camera_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_active_sessions_for_cameras(
    camera_ids: &[i64],
    pool: &SqlitePool,
) -> Result<Vec<RecordingSession>> {
    if camera_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('queued', 'recording')
          AND camera_id IN (
        "#,
    );

    let mut separated = query.separated(", ");
    for camera_id in camera_ids {
        separated.push_bind(camera_id);
    }
    separated.push_unseparated(") ORDER BY created_at DESC");

    Ok(query
        .build_query_as::<RecordingSession>()
        .fetch_all(pool)
        .await?)
}

pub async fn get_last_rule_session(
    rule_id: i64,
    pool: &SqlitePool,
) -> Result<Option<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE rule_id = ?
          AND deleted_at IS NULL
          AND status != 'deleted'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn count_rule_sessions_since(
    rule_id: i64,
    since: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM camera_recording_sessions
        WHERE rule_id = ?
          AND deleted_at IS NULL
          AND status != 'deleted'
          AND DATETIME(created_at) >= DATETIME(?)
        "#,
    )
    .bind(rule_id)
    .bind(since)
    .fetch_one(pool)
    .await?)
}

pub async fn mark_recording(session_id: i64, pool: &SqlitePool) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'recording', started_at = COALESCE(started_at, ?), error = NULL
        WHERE id = ?
        "#,
    )
    .bind(Utc::now())
    .bind(session_id);
    crate::db::log_slow_operation(
        "camera_recording_sessions.mark_recording",
        query.execute(pool),
    )
    .await?;
    Ok(())
}

pub async fn request_stop(session_id: i64, pool: &SqlitePool) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET stop_after_at = ?
        WHERE id = ?
          AND deleted_at IS NULL
          AND status IN ('queued', 'recording')
        "#,
    )
    .bind(Utc::now())
    .bind(session_id);
    crate::db::log_slow_operation(
        "camera_recording_sessions.request_stop",
        query.execute(pool),
    )
    .await?;
    Ok(())
}

pub async fn mark_ready(
    session_id: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'ready', completed_at = ?, error = NULL
        WHERE id = ?
        "#,
    )
    .bind(completed_at)
    .bind(session_id);
    crate::db::log_slow_operation("camera_recording_sessions.mark_ready", query.execute(pool))
        .await?;
    Ok(())
}

pub async fn mark_failed(session_id: i64, error: &str, pool: &SqlitePool) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'failed', completed_at = COALESCE(completed_at, ?), error = ?
        WHERE id = ?
        "#,
    )
    .bind(Utc::now())
    .bind(crate::db::sanitize_error(error))
    .bind(session_id);
    crate::db::log_slow_operation("camera_recording_sessions.mark_failed", query.execute(pool))
        .await?;
    Ok(())
}

pub async fn mark_notification_sent(session_id: i64, pool: &SqlitePool) -> Result<()> {
    let query =
        sqlx::query("UPDATE camera_recording_sessions SET notification_sent_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(session_id);
    crate::db::log_slow_operation(
        "camera_recording_sessions.mark_notification_sent",
        query.execute(pool),
    )
    .await?;
    Ok(())
}

pub async fn soft_delete_session(session_id: i64, pool: &SqlitePool) -> Result<()> {
    let now = Utc::now();
    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'deleted', deleted_at = ?
        WHERE id = ?
        "#,
    )
    .bind(now)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE camera_recording_segments
        SET status = 'deleted', deleted_at = ?, file_path = NULL
        WHERE session_id = ?
        "#,
    )
    .bind(now)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn find_expired_sessions(pool: &SqlitePool) -> Result<Vec<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('ready', 'failed')
          AND DATETIME(expires_at) < DATETIME(?)
        ORDER BY expires_at
        "#,
    )
    .bind(Utc::now())
    .fetch_all(pool)
    .await?)
}

pub async fn recover_stale_sessions(pool: &SqlitePool) -> Result<u64> {
    let now = Utc::now();
    let result = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'failed',
            completed_at = COALESCE(completed_at, ?),
            error = 'service restarted before recording completed'
        WHERE deleted_at IS NULL
          AND status IN ('queued', 'recording')
        "#,
    )
    .bind(now)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE camera_recording_segments
        SET status = 'failed',
            completed_at = COALESCE(completed_at, ?),
            error = 'service restarted before recording completed'
        WHERE deleted_at IS NULL
          AND status IN ('queued', 'recording')
        "#,
    )
    .bind(now)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

fn add_rule_id(existing: Option<&str>, rule_id: i64) -> String {
    let mut ids: Vec<i64> = existing
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();

    if !ids.contains(&rule_id) {
        ids.push(rule_id);
    }

    serde_json::to_string(&ids).unwrap_or_else(|_| format!("[{}]", rule_id))
}

fn append_summary(existing: &str, addition: &str) -> String {
    if existing.contains(addition) {
        existing.to_string()
    } else {
        format!("{}; {}", existing, addition)
    }
}
