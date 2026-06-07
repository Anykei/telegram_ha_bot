use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingSegment {
    pub id: i64,
    pub session_id: i64,
    pub camera_id: i64,
    pub segment_index: i64,
    pub file_path: Option<String>,
    pub duration_s: i64,
    pub size_bytes: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_segment(
    session_id: i64,
    camera_id: i64,
    segment_index: i64,
    duration_s: i64,
    expires_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<i64> {
    let query = sqlx::query(
        r#"
        INSERT INTO camera_recording_segments (
            session_id, camera_id, segment_index, duration_s, status, started_at, expires_at
        )
        VALUES (?, ?, ?, ?, 'recording', ?, ?)
        "#,
    )
    .bind(session_id)
    .bind(camera_id)
    .bind(segment_index)
    .bind(duration_s)
    .bind(Utc::now())
    .bind(expires_at);
    let result = crate::db::log_slow_operation(
        "camera_recording_segments.create_segment",
        query.execute(pool),
    )
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn mark_ready(
    segment_id: i64,
    file_path: &str,
    size_bytes: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_segments
        SET status = 'ready',
            file_path = ?,
            size_bytes = ?,
            completed_at = ?,
            error = NULL
        WHERE id = ?
        "#,
    )
    .bind(file_path)
    .bind(size_bytes)
    .bind(completed_at)
    .bind(segment_id);
    crate::db::log_slow_operation("camera_recording_segments.mark_ready", query.execute(pool))
        .await?;

    Ok(())
}

pub async fn mark_failed(segment_id: i64, error: &str, pool: &SqlitePool) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_segments
        SET status = 'failed',
            completed_at = COALESCE(completed_at, ?),
            error = ?
        WHERE id = ?
        "#,
    )
    .bind(Utc::now())
    .bind(crate::db::sanitize_error(error))
    .bind(segment_id);
    crate::db::log_slow_operation("camera_recording_segments.mark_failed", query.execute(pool))
        .await?;

    Ok(())
}

pub async fn list_session_segments(
    session_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingSegment>> {
    Ok(sqlx::query_as::<_, RecordingSegment>(
        r#"
        SELECT id, session_id, camera_id, segment_index, file_path, duration_s, size_bytes,
               status, error, started_at, completed_at, expires_at, deleted_at, created_at
        FROM camera_recording_segments
        WHERE session_id = ?
          AND deleted_at IS NULL
        ORDER BY segment_index
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_ready_segments(
    session_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingSegment>> {
    Ok(sqlx::query_as::<_, RecordingSegment>(
        r#"
        SELECT id, session_id, camera_id, segment_index, file_path, duration_s, size_bytes,
               status, error, started_at, completed_at, expires_at, deleted_at, created_at
        FROM camera_recording_segments
        WHERE session_id = ?
          AND status = 'ready'
          AND deleted_at IS NULL
          AND file_path IS NOT NULL
        ORDER BY segment_index
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?)
}

pub async fn get_segment(segment_id: i64, pool: &SqlitePool) -> Result<Option<RecordingSegment>> {
    Ok(sqlx::query_as::<_, RecordingSegment>(
        r#"
        SELECT id, session_id, camera_id, segment_index, file_path, duration_s, size_bytes,
               status, error, started_at, completed_at, expires_at, deleted_at, created_at
        FROM camera_recording_segments
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(segment_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn sum_ready_size_bytes(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(SUM(size_bytes), 0)
        FROM camera_recording_segments
        WHERE status = 'ready'
          AND deleted_at IS NULL
        "#,
    )
    .fetch_one(pool)
    .await?)
}

pub async fn list_deletable_sessions_for_quota(
    pool: &SqlitePool,
) -> Result<Vec<crate::db::camera_recording_sessions::RecordingSession>> {
    Ok(
        sqlx::query_as::<_, crate::db::camera_recording_sessions::RecordingSession>(
            r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('ready', 'failed')
        ORDER BY created_at ASC
        "#,
        )
        .fetch_all(pool)
        .await?,
    )
}

pub async fn soft_delete_segments_for_session(
    session_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<String>> {
    let segments = list_session_segments(session_id, pool).await?;
    let paths = segments
        .iter()
        .filter_map(|segment| segment.file_path.clone())
        .collect::<Vec<_>>();

    sqlx::query(
        r#"
        UPDATE camera_recording_segments
        SET status = 'deleted', deleted_at = ?, file_path = NULL
        WHERE session_id = ?
        "#,
    )
    .bind(Utc::now())
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(paths)
}
