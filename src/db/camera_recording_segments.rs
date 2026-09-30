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
    pool: &SqlitePool,
) -> Result<i64> {
    // A merge may extend retention while a writer waits for the pool. Read the
    // canonical deadline in the INSERT instead of trusting its earlier snapshot.
    let query = sqlx::query(
        r#"
        INSERT INTO camera_recording_segments (
            session_id, camera_id, segment_index, duration_s, status, started_at, expires_at
        )
        SELECT id, ?, ?, ?, 'recording', ?, expires_at
        FROM camera_recording_sessions
        WHERE id=? AND deleted_at IS NULL AND status IN ('queued','recording')
        "#,
    )
    .bind(camera_id)
    .bind(segment_index)
    .bind(duration_s)
    .bind(Utc::now())
    .bind(session_id);
    let result = crate::db::log_slow_operation(
        "camera_recording_segments.create_segment",
        query.execute(pool),
    )
    .await?;

    anyhow::ensure!(
        result.rows_affected() == 1,
        "Recording session is no longer active"
    );
    Ok(result.last_insert_rowid())
}

pub async fn mark_ready(
    segment_id: i64,
    file_path: &str,
    size_bytes: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let updated = sqlx::query("UPDATE camera_recording_segments SET status='ready',file_path=?,size_bytes=?,completed_at=?,error=NULL WHERE id=? AND deleted_at IS NULL AND status='recording' AND EXISTS(SELECT 1 FROM camera_recording_sessions r WHERE r.id=camera_recording_segments.session_id AND r.deleted_at IS NULL AND r.status='recording') AND EXISTS(SELECT 1 FROM recording_files f WHERE f.path=? AND f.segment_id=camera_recording_segments.id AND f.session_id=camera_recording_segments.session_id AND f.delete_pending=0)")
        .bind(file_path).bind(size_bytes).bind(completed_at).bind(segment_id).bind(file_path).execute(&mut *tx).await?.rows_affected();
    if updated == 1 {
        sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=? AND segment_id=?")
            .bind(size_bytes)
            .bind(file_path)
            .bind(segment_id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE recording_files SET delete_pending=1 WHERE segment_id=?")
            .bind(segment_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    anyhow::ensure!(
        updated == 1,
        "Recording publication rejected: owner removed, cancelled, or path unregistered"
    );
    Ok(())
}

/// Reuse the head's stable index while each attempt writes a different owned
/// path. A ready head stays readable until a better candidate is published.
pub async fn prepare_pre_roll_segment(session_id: i64, pool: &SqlitePool) -> Result<i64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM camera_recording_sessions WHERE id=? AND deleted_at IS NULL AND status='recording')")
        .bind(session_id).fetch_one(&mut *tx).await?;
    anyhow::ensure!(active, "Pre-roll session is no longer active");
    sqlx::query("INSERT OR IGNORE INTO camera_recording_segments(session_id,camera_id,segment_index,duration_s,status,started_at,expires_at) SELECT id,camera_id,0,0,'recording',?,expires_at FROM camera_recording_sessions WHERE id=?")
        .bind(Utc::now()).bind(session_id).execute(&mut *tx).await?;
    let id: i64 = sqlx::query_scalar("UPDATE camera_recording_segments SET status=CASE WHEN status='failed' THEN 'recording' ELSE status END WHERE session_id=? AND segment_index=0 AND deleted_at IS NULL AND status IN ('recording','ready','failed') RETURNING id")
        .bind(session_id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

pub struct PreRollPublication<'a> {
    pub path: &'a str,
    pub paths: &'a [String],
    pub size_bytes: i64,
    pub duration_s: i64,
    pub from: DateTime<Utc>,
    pub requested_seconds: i64,
    pub actual_seconds: i64,
    pub partial: bool,
    pub warning: Option<&'a str>,
}

pub async fn publish_pre_roll(
    segment_id: i64,
    publication: PreRollPublication<'_>,
    pool: &SqlitePool,
) -> Result<bool> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    type PublishedHead = (i64, Option<String>, Option<DateTime<Utc>>, i64);
    let current: Option<PublishedHead> = sqlx::query_as(
        "SELECT r.id,s.file_path,r.pre_roll_from,r.pre_roll_actual_seconds FROM camera_recording_segments s JOIN camera_recording_sessions r ON r.id=s.session_id WHERE s.id=? AND s.deleted_at IS NULL AND s.status IN ('recording','ready') AND r.deleted_at IS NULL AND r.status='recording' AND EXISTS(SELECT 1 FROM recording_files f WHERE f.path=? AND f.segment_id=s.id AND f.delete_pending=1) AND EXISTS(SELECT 1 FROM recording_writers w WHERE w.segment_id=s.id AND w.kind='pre_roll')",
    ).bind(segment_id).bind(publication.path).fetch_optional(&mut *tx).await?;
    let (session_id, old_path, old_from, old_actual) = current.ok_or_else(|| {
        anyhow::anyhow!("Pre-roll publication rejected: owner inactive or path unregistered")
    })?;
    let replace = old_path.is_none()
        || publication.actual_seconds > old_actual
        || (publication.actual_seconds == old_actual
            && old_from.is_none_or(|from| publication.from < from));
    if replace {
        sqlx::query("UPDATE camera_recording_segments SET status='ready',file_path=?,size_bytes=?,duration_s=?,completed_at=?,error=NULL WHERE id=?")
            .bind(publication.path).bind(publication.size_bytes).bind(publication.duration_s).bind(Utc::now()).bind(segment_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE recording_files SET size_bytes=? WHERE path=? AND segment_id=?")
            .bind(publication.size_bytes)
            .bind(publication.path)
            .bind(segment_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE camera_recording_sessions SET pre_roll_from=?,pre_roll_actual_seconds=?,pre_roll_attempted_seconds=MAX(pre_roll_attempted_seconds,?),pre_roll_partial=?,pre_roll_warning=? WHERE id=?")
            .bind(publication.from).bind(publication.actual_seconds).bind(publication.requested_seconds).bind(publication.partial).bind(publication.warning.map(crate::db::sanitize_error)).bind(session_id).execute(&mut *tx).await?;
        for path in publication.paths {
            sqlx::query(
                "UPDATE recording_files SET delete_pending=0 WHERE segment_id=? AND path=?",
            )
            .bind(segment_id)
            .bind(path)
            .execute(&mut *tx)
            .await?;
        }
    } else {
        sqlx::query("UPDATE camera_recording_sessions SET pre_roll_attempted_seconds=MAX(pre_roll_attempted_seconds,?),pre_roll_partial=pre_roll_actual_seconds<pre_roll_seconds,pre_roll_warning=CASE WHEN pre_roll_actual_seconds<pre_roll_seconds THEN ? ELSE NULL END WHERE id=?")
            .bind(publication.requested_seconds).bind(publication.warning.unwrap_or("Requested pre-roll history is no longer available; previous head retained")).bind(session_id).execute(&mut *tx).await?;
    }
    // Old preview writers retain their leases. Maintenance unlinks these paths
    // only after every writer for the session has really stopped.
    let mut delete = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "UPDATE recording_files SET delete_pending=1 WHERE segment_id=",
    );
    delete.push_bind(segment_id).push(if replace {
        " AND path NOT IN ("
    } else {
        " AND path IN ("
    });
    let mut paths = delete.separated(",");
    for path in publication.paths {
        paths.push_bind(path);
    }
    paths.push_unseparated(")");
    delete.build().execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(replace)
}

pub async fn discard_pre_roll_candidate(
    segment_id: i64,
    error: &str,
    pool: &SqlitePool,
) -> Result<()> {
    // Candidate paths were already registered pending. Never re-mark paths
    // here: COMMIT may have succeeded even if its caller lost the receipt.
    sqlx::query("UPDATE camera_recording_segments SET status='failed',error=? WHERE id=? AND status='recording' AND file_path IS NULL")
        .bind(crate::db::sanitize_error(error)).bind(segment_id).execute(pool).await?;
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

    sqlx::query("UPDATE recording_files SET delete_pending=1 WHERE segment_id=?")
        .bind(segment_id)
        .execute(pool)
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
        SELECT COALESCE(SUM(size_bytes), 0) FROM recording_files
        "#,
    )
    .fetch_one(pool)
    .await?)
}

pub async fn sum_pinned_ready_size_bytes(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(SUM(s.size_bytes), 0)
        FROM camera_recording_segments s
        JOIN camera_recording_sessions rs ON rs.id = s.session_id
        WHERE s.status = 'ready'
          AND s.deleted_at IS NULL
          AND rs.pinned_at IS NOT NULL
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
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('ready', 'failed')
          AND pinned_at IS NULL
        ORDER BY created_at ASC, id ASC
        "#,
        )
        .fetch_all(pool)
        .await?,
    )
}

/// Registers every possible output before any writer opens a file.
pub async fn register_paths(segment_id: i64, paths: &[String], pool: &SqlitePool) -> Result<()> {
    register_paths_inner(segment_id, paths, false, pool).await
}

/// Pending from registration until publication: cancellation or a crash cannot
/// strand a candidate beside a still-ready older head. The writer lease stops
/// cleanup from unlinking it while native work can still write.
pub async fn register_pre_roll_candidate(
    segment_id: i64,
    paths: &[String],
    pool: &SqlitePool,
) -> Result<()> {
    register_paths_inner(segment_id, paths, true, pool).await
}

async fn register_paths_inner(
    segment_id: i64,
    paths: &[String],
    pending: bool,
    pool: &SqlitePool,
) -> Result<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let owner: Option<i64> = sqlx::query_scalar("SELECT s.session_id FROM camera_recording_segments s JOIN camera_recording_sessions r ON r.id=s.session_id WHERE s.id=? AND s.deleted_at IS NULL AND r.deleted_at IS NULL AND s.status IN ('recording','ready','failed') AND r.status IN ('queued','recording','ready','failed')")
        .bind(segment_id).fetch_optional(&mut *tx).await?;
    let session_id =
        owner.ok_or_else(|| anyhow::anyhow!("Recording owner no longer permits writing"))?;
    if pending {
        let writing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM recording_writers WHERE segment_id=? AND kind='pre_roll')",
        )
        .bind(segment_id)
        .fetch_one(&mut *tx)
        .await?;
        anyhow::ensure!(writing, "Pre-roll registration requires a live writer");
    }
    for path in paths {
        anyhow::ensure!(safe_relative_path(path), "Unsafe recording path");
        sqlx::query(
            "INSERT OR IGNORE INTO recording_files(path, session_id, segment_id, delete_pending) VALUES(?,?,?,?)",
        )
        .bind(path)
        .bind(session_id)
        .bind(segment_id)
        .bind(pending)
        .execute(&mut *tx)
        .await?;
        let owner: (i64, i64, bool) = sqlx::query_as(
            "SELECT session_id,segment_id,delete_pending FROM recording_files WHERE path=?",
        )
        .bind(path)
        .fetch_one(&mut *tx)
        .await?;
        anyhow::ensure!(
            (owner.0, owner.1) == (session_id, segment_id),
            "Recording path already belongs to another owner"
        );
        anyhow::ensure!(
            !pending || owner.2,
            "Pre-roll candidate must not reuse a published path"
        );
    }
    tx.commit().await?;
    Ok(())
}

pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && std::path::Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

pub async fn begin_writer(segment_id: i64, kind: &str, pool: &SqlitePool) -> Result<i64> {
    let id = sqlx::query_scalar::<_, i64>("INSERT INTO recording_writers(session_id,segment_id,kind) SELECT s.session_id,s.id,? FROM camera_recording_segments s JOIN camera_recording_sessions r ON r.id=s.session_id WHERE s.id=? AND s.deleted_at IS NULL AND r.deleted_at IS NULL AND s.status IN ('recording','ready') RETURNING id")
        .bind(kind).bind(segment_id).fetch_optional(pool).await?;
    id.ok_or_else(|| anyhow::anyhow!("Recording owner no longer permits writing"))
}

/// Authorize the exact published source and register outputs at the same write
/// boundary as the lease. A head swap cannot turn an old path into a new writer.
pub async fn begin_derivative_writer(
    source_path: &str,
    kind: &str,
    paths: &[String],
    pool: &SqlitePool,
) -> Result<i64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let owner: Option<(i64, i64)> = sqlx::query_as(
        "SELECT s.id,s.session_id FROM camera_recording_segments s JOIN camera_recording_sessions r ON r.id=s.session_id JOIN recording_files f ON f.path=s.file_path AND f.segment_id=s.id AND f.session_id=s.session_id WHERE s.file_path=? AND s.status='ready' AND s.deleted_at IS NULL AND r.deleted_at IS NULL AND r.status IN ('recording','ready','failed') AND f.delete_pending=0",
    ).bind(source_path).fetch_optional(&mut *tx).await?;
    let (segment_id, session_id) = owner
        .ok_or_else(|| anyhow::anyhow!("Recording derivative source changed or was removed"))?;
    for path in paths {
        anyhow::ensure!(safe_relative_path(path), "Unsafe recording derivative path");
        sqlx::query(
            "INSERT OR IGNORE INTO recording_files(path,session_id,segment_id) VALUES(?,?,?)",
        )
        .bind(path)
        .bind(session_id)
        .bind(segment_id)
        .execute(&mut *tx)
        .await?;
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recording_files WHERE path=? AND session_id=? AND segment_id=? AND delete_pending=0)")
            .bind(path).bind(session_id).bind(segment_id).fetch_one(&mut *tx).await?;
        anyhow::ensure!(
            valid,
            "Recording derivative path belongs to a removed or different owner"
        );
    }
    let writer: i64 = sqlx::query_scalar(
        "INSERT INTO recording_writers(session_id,segment_id,kind) VALUES(?,?,?) RETURNING id",
    )
    .bind(session_id)
    .bind(segment_id)
    .bind(kind)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(writer)
}

pub async fn finish_writer(id: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM recording_writers WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn pending_file_count(pool: &SqlitePool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM recording_files WHERE delete_pending=1")
            .fetch_one(pool)
            .await?,
    )
}
