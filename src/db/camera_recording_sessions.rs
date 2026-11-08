use crate::db::camera_recording_rules::RecordingRule;
use anyhow::Result;
use chrono::{DateTime, Duration, Local, NaiveTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::db::{camera_recording_rules as rules, cameras, rooms};

    #[tokio::test]
    async fn pre_roll_upgrade_separates_post_event_duration_from_historical_coverage() -> Result<()>
    {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let mut baseline = sqlx::migrate::Migrator::new(std::path::Path::new("migrations")).await?;
        baseline.migrations = std::borrow::Cow::Owned(
            baseline
                .iter()
                .filter(|migration| migration.version < 20260930170000)
                .cloned()
                .collect(),
        );
        baseline.run(&pool).await?;
        sqlx::raw_sql(r#"
            INSERT INTO cameras(id,key,name,stream_url) VALUES(1,'test','Test','rtsp://unused');
            INSERT INTO camera_recording_rules(id,name,camera_id) VALUES(1,'Rule',1);
            INSERT INTO camera_recording_sessions(id,event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at,pre_roll_from,pre_roll_seconds)
            VALUES
            (1,'test',1,1,'test','ready','2026-09-30T10:00:00Z','2026-09-30T10:00:00Z','2026-09-30T10:01:00Z','2026-10-30T10:00:00Z','2026-09-30T09:59:45Z',20),
            (2,'test',1,1,'test','ready','2026-09-30T10:00:00Z','2026-09-30T10:00:00Z','2026-09-30T10:01:00Z','2026-10-30T10:00:00Z','2026-09-30T10:00:05Z',20),
            (3,'test',1,1,'test','ready','2026-09-30T10:00:00Z','2026-09-30T10:00:00Z','2026-09-30T10:01:00Z','2026-10-30T10:00:00Z','2026-09-30T09:59:45Z',7),
            (4,'test',1,1,'test','recording','2026-09-30T10:00:00Z','2026-09-30T10:00:00Z','2026-09-30T10:01:00Z','2026-10-30T10:00:00Z',NULL,15);
        "#).execute(&pool).await?;
        let upgrade = sqlx::migrate::Migrator::new(std::path::Path::new("migrations")).await?;
        upgrade.run(&pool).await?;
        upgrade.run(&pool).await?;
        let coverage: Vec<(i64,i64,i64)> = sqlx::query_as("SELECT pre_roll_seconds,pre_roll_actual_seconds,pre_roll_attempted_seconds FROM camera_recording_sessions ORDER BY id")
            .fetch_all(&pool).await?;
        assert_eq!(
            coverage,
            vec![(20, 15, 20), (20, 0, 20), (7, 7, 7), (15, 0, 0)]
        );
        Ok(())
    }

    async fn recording_rule(pool: &SqlitePool) -> Result<RecordingRule> {
        sqlx::migrate!("./migrations").run(pool).await?;
        rooms::sync_rooms_from_ha("test", "Test", pool).await?;
        let camera_id = cameras::add_manual_camera(
            cameras::NewCamera {
                name: "test",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 10,
            },
            pool,
        )
        .await?;
        let id = rules::create_rule(
            rules::NewRecordingRule {
                name: "test",
                camera_id,
                condition_logic: rules::ConditionLogic::Any,
                tail_seconds: 5,
                max_segment_seconds: 60,
                cooldown_s: 0,
                retention_days: 30,
                pre_roll_enabled: true,
                pre_roll_seconds: 15,
            },
            pool,
        )
        .await?;
        Ok(rules::get_rule(id, pool).await?.unwrap())
    }

    async fn expired_session(
        rule: &RecordingRule,
        now: DateTime<Utc>,
        pool: &SqlitePool,
    ) -> Result<i64> {
        let SessionAction::Created(id) =
            start_or_extend_session(rule, "first", "first", now - Duration::seconds(10), pool)
                .await?
        else {
            panic!("expected new session")
        };
        mark_recording(id, pool).await?;
        assert!(get_session(id, pool).await?.unwrap().stop_after_at <= now);
        let segment =
            crate::db::camera_recording_segments::create_segment(id, rule.camera_id, 0, 5, pool)
                .await?;
        crate::db::camera_recording_segments::register_paths(segment, &["test.mp4".into()], pool)
            .await?;
        crate::db::camera_recording_segments::mark_ready(segment, "test.mp4", 1024, now, pool)
            .await?;
        mark_pre_roll_failure(
            id,
            rule.normalized_pre_roll_seconds(),
            "fixture has no pre-roll history",
            pool,
        )
        .await?;
        Ok(id)
    }

    #[tokio::test]
    async fn event_between_deadline_check_and_finalization_keeps_recording() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let rule = recording_rule(&pool).await?;
        let now = Utc::now();
        let id = expired_session(&rule, now, &pool).await?;
        assert_eq!(
            start_or_extend_session(&rule, "second", "second", now, &pool).await?,
            SessionAction::Extended(id)
        );
        assert!(!mark_ready_if_due(id, now, &pool).await?);
        let current = get_session(id, &pool).await?.unwrap();
        assert_eq!(current.status, "recording");
        assert_eq!(current.stop_after_at, now + Duration::seconds(5));
        assert!(current.completed_at.is_none());
        assert_eq!(current.pre_roll_seconds, 15);
        assert!(rules::get_rule(rule.id, &pool)
            .await?
            .unwrap()
            .last_completed_at
            .is_none());

        let deadline = current.stop_after_at;
        assert!(!mark_ready_if_due(id, deadline - Duration::milliseconds(1), &pool).await?);
        assert!(mark_ready_if_due(id, deadline, &pool).await?);
        assert!(!mark_ready_if_due(id, deadline + Duration::seconds(1), &pool).await?);
        let done = get_session(id, &pool).await?.unwrap();
        assert_eq!(done.status, "ready");
        assert_eq!(done.completed_at, Some(deadline));
        assert_eq!(
            rules::get_rule(rule.id, &pool)
                .await?
                .unwrap()
                .last_completed_at,
            Some(deadline)
        );

        let SessionAction::Created(next_id) = start_or_extend_session(
            &rule,
            "third",
            "third",
            deadline + Duration::seconds(1),
            &pool,
        )
        .await?
        else {
            panic!("an event after finalization must create a new session")
        };
        assert_ne!(next_id, id);
        Ok(())
    }

    #[tokio::test]
    async fn finalization_respects_manual_stop_terminal_states_and_cooldown() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let mut rule = recording_rule(&pool).await?;
        let now = Utc::now();
        let id = expired_session(&rule, now, &pool).await?;
        for state in ["queued", "failed", "deleted", "ready"] {
            sqlx::query(
                "UPDATE camera_recording_sessions SET status = ?, error = 'keep' WHERE id = ?",
            )
            .bind(state)
            .bind(id)
            .execute(&pool)
            .await?;
            assert!(!mark_ready_if_due(id, now, &pool).await?);
            let current = get_session(id, &pool).await?.unwrap();
            assert_eq!(current.status, state);
            assert_eq!(current.error.as_deref(), Some("keep"));
            assert!(current.completed_at.is_none());
        }
        sqlx::query("UPDATE camera_recording_sessions SET status = 'recording', deleted_at = ? WHERE id = ?")
            .bind(now).bind(id).execute(&pool).await?;
        assert!(!mark_ready_if_due(id, now, &pool).await?);
        sqlx::query("UPDATE camera_recording_sessions SET deleted_at = NULL WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await?;
        start_or_extend_session(&rule, "extend", "extend", now, &pool).await?;
        assert!(!mark_ready_if_due(id, now, &pool).await?);
        request_stop(id, &pool).await?;

        rule.cooldown_s = 30;
        sqlx::query("UPDATE camera_recording_rules SET cooldown_s = 30 WHERE id = ?")
            .bind(rule.id)
            .execute(&pool)
            .await?;
        let stopped_at = Utc::now();
        assert!(mark_ready_if_due(id, stopped_at, &pool).await?);
        assert!(rule.last_completed_at.is_none()); // Matcher still holds an older snapshot.
        assert_eq!(
            start_or_extend_session(&rule, "cooldown", "cooldown", stopped_at, &pool).await?,
            SessionAction::SkippedCooldown
        );
        assert!(matches!(
            start_or_extend_session(
                &rule,
                "next",
                "next",
                stopped_at + Duration::seconds(30),
                &pool
            )
            .await?,
            SessionAction::Created(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_finalization_and_event_preserve_an_active_tail_on_wal_database(
    ) -> Result<()> {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        let path = std::env::temp_dir().join(format!(
            "ha-recording-race-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::from_secs(5)),
            )
            .await?;
        let rule = recording_rule(&pool).await?;
        let now = Utc::now();
        let id = expired_session(&rule, now, &pool).await?;
        let (finished, event) = tokio::join!(
            mark_ready_if_due(id, now, &pool),
            start_or_extend_session(&rule, "race", "race", now, &pool),
        );
        let active_id = match (finished?, event?) {
            (false, SessionAction::Extended(extended)) => {
                assert_eq!(extended, id);
                extended
            }
            (true, SessionAction::Created(created)) => {
                assert_ne!(created, id);
                created
            }
            other => panic!("event was lost during finalization: {other:?}"),
        };
        let active = get_session(active_id, &pool).await?.unwrap();
        assert!(matches!(active.status.as_str(), "queued" | "recording"));
        assert_eq!(active.stop_after_at, now + Duration::seconds(5));
        assert!(active.completed_at.is_none());
        pool.close().await;
        tokio::fs::remove_file(path).await?;
        Ok(())
    }

    #[tokio::test]
    async fn repeated_events_only_extend_until_explicit_manual_stop() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        rooms::sync_rooms_from_ha("test", "Test", &pool).await?;
        let camera_id = cameras::add_manual_camera(
            cameras::NewCamera {
                name: "test",
                room_id: 1,
                stream_url: "rtsp://unused",
                snapshot_url: None,
                clip_seconds: 10,
            },
            &pool,
        )
        .await?;
        let mut saved_rules = Vec::new();
        for tail in [300, 5, 600] {
            let id = rules::create_rule(
                rules::NewRecordingRule {
                    name: "test",
                    camera_id,
                    condition_logic: rules::ConditionLogic::Any,
                    tail_seconds: tail,
                    max_segment_seconds: 60,
                    cooldown_s: 0,
                    retention_days: 30,
                    pre_roll_enabled: true,
                    pre_roll_seconds: 15,
                },
                &pool,
            )
            .await?;
            saved_rules.push(rules::get_rule(id, &pool).await?.unwrap());
        }
        let now = Utc::now();
        let SessionAction::Created(id) =
            start_or_extend_session(&saved_rules[0], "event", "first", now, &pool).await?
        else {
            panic!("expected creation")
        };
        for (index, elapsed, expected) in [(1, 10, 300), (1, 295, 300), (0, 10, 310), (2, 20, 620)]
        {
            assert_eq!(
                start_or_extend_session(
                    &saved_rules[index],
                    "event",
                    "next",
                    now + Duration::seconds(elapsed),
                    &pool
                )
                .await?,
                SessionAction::Extended(id)
            );
            let session = get_session(id, &pool).await?.unwrap();
            assert_eq!(session.stop_after_at, now + Duration::seconds(expected));
            assert_eq!(session.pre_roll_seconds, 15);
        }
        mark_recording(id, &pool).await?;
        start_or_extend_session(
            &saved_rules[1],
            "event",
            "recording",
            now + Duration::seconds(30),
            &pool,
        )
        .await?;
        assert_eq!(
            get_session(id, &pool).await?.unwrap().stop_after_at,
            now + Duration::seconds(620)
        );
        request_stop(id, &pool).await?;
        assert!(
            get_session(id, &pool).await?.unwrap().stop_after_at < now + Duration::seconds(300)
        );
        Ok(())
    }

    async fn another_rule(
        original: &RecordingRule,
        retention_days: i64,
        cooldown_s: i64,
        pool: &SqlitePool,
    ) -> Result<RecordingRule> {
        let id = rules::create_rule(
            rules::NewRecordingRule {
                name: "joined",
                camera_id: original.camera_id,
                condition_logic: rules::ConditionLogic::Any,
                tail_seconds: 5,
                max_segment_seconds: 60,
                cooldown_s,
                retention_days,
                pre_roll_enabled: true,
                pre_roll_seconds: 15,
            },
            pool,
        )
        .await?;
        Ok(rules::get_rule(id, pool).await?.unwrap())
    }

    #[tokio::test]
    async fn merged_retention_updates_existing_and_late_parts_without_shortening() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let mut short = recording_rule(&pool).await?;
        short.retention_days = 1;
        let long = another_rule(&short, 30, 0, &pool).await?;
        let now = Utc::now();
        let id = expired_session(&short, now, &pool).await?;
        let original_expiry = get_session(id, &pool).await?.unwrap().expires_at;
        let writer_part =
            crate::db::camera_recording_segments::create_segment(id, short.camera_id, 1, 5, &pool)
                .await?;

        assert_eq!(
            start_or_extend_session(&long, "joined", "long retention", now, &pool).await?,
            SessionAction::Extended(id)
        );
        let promised = now + Duration::days(30);
        assert!(original_expiry < promised);
        assert_eq!(get_session(id, &pool).await?.unwrap().expires_at, promised);
        // A writer selected its parameters before the merge, but inserts after
        // it. Its stale snapshot must not restore the old part expiry.
        crate::db::camera_recording_segments::create_segment(id, short.camera_id, 2, 5, &pool)
            .await?;
        start_or_extend_session(
            &short,
            "short",
            "short retention",
            now + Duration::seconds(1),
            &pool,
        )
        .await?;
        assert_eq!(get_session(id, &pool).await?.unwrap().expires_at, promised);
        let parts = crate::db::camera_recording_segments::list_session_segments(id, &pool).await?;
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.expires_at == promised));
        assert_eq!(
            parts
                .iter()
                .find(|part| part.id == writer_part)
                .unwrap()
                .status,
            "recording"
        );

        // A failed metadata write cannot commit just the session half.
        sqlx::query("CREATE TRIGGER reject_retention BEFORE UPDATE OF expires_at ON camera_recording_segments BEGIN SELECT RAISE(ABORT, 'test retention barrier'); END")
            .execute(&pool).await?;
        assert!(
            start_or_extend_session(&long, "later", "later", now + Duration::days(1), &pool)
                .await
                .is_err()
        );
        assert_eq!(get_session(id, &pool).await?.unwrap().expires_at, promised);
        assert!(
            crate::db::camera_recording_segments::list_session_segments(id, &pool)
                .await?
                .iter()
                .all(|part| part.expires_at == promised)
        );
        Ok(())
    }

    #[tokio::test]
    async fn completion_applies_cooldown_to_all_joined_rules_and_no_others() -> Result<()> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        let original = recording_rule(&pool).await?;
        let joined = another_rule(&original, 30, 300, &pool).await?;
        let unrelated = another_rule(&original, 30, 300, &pool).await?;
        let now = Utc::now();
        let id = expired_session(&original, now, &pool).await?;
        assert_eq!(
            start_or_extend_session(&joined, "joined", "joined", now, &pool).await?,
            SessionAction::Extended(id)
        );
        assert!(!mark_ready_if_due(id, now, &pool).await?);
        assert!(rules::get_rule(joined.id, &pool)
            .await?
            .unwrap()
            .last_completed_at
            .is_none());
        let completed_at = now + Duration::seconds(5);
        assert!(mark_ready_if_due(id, completed_at, &pool).await?);
        for rule in [&original, &joined] {
            assert_eq!(
                rules::get_rule(rule.id, &pool)
                    .await?
                    .unwrap()
                    .last_completed_at,
                Some(completed_at)
            );
        }
        assert!(rules::get_rule(unrelated.id, &pool)
            .await?
            .unwrap()
            .last_completed_at
            .is_none());
        // The matcher still owns the snapshot captured before completion.
        assert!(joined.last_completed_at.is_none());
        assert_eq!(
            start_or_extend_session(
                &joined,
                "too soon",
                "joined",
                completed_at + Duration::seconds(1),
                &pool
            )
            .await?,
            SessionAction::SkippedCooldown
        );
        assert!(!mark_ready_if_due(id, completed_at + Duration::seconds(100), &pool).await?);
        assert_eq!(
            rules::get_rule(joined.id, &pool)
                .await?
                .unwrap()
                .last_completed_at,
            Some(completed_at)
        );
        assert!(matches!(
            start_or_extend_session(
                &joined,
                "after cooldown",
                "joined",
                completed_at + Duration::seconds(300),
                &pool
            )
            .await?,
            SessionAction::Created(_)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn automatic_delete_rechecks_pin_after_waiting_for_database() -> Result<()> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let rule = recording_rule(&pool).await?;
        let now = Utc::now();
        let id = expired_session(&rule, now, &pool).await?;
        assert!(mark_ready_if_due(id, now, &pool).await?);
        let mut connection = pool.acquire().await?;
        let pending = soft_delete_session_if_unpinned(id, &pool);
        tokio::pin!(pending);
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
        sqlx::query("UPDATE camera_recording_sessions SET pinned_at=? WHERE id=?")
            .bind(now)
            .bind(id)
            .execute(&mut *connection)
            .await?;
        drop(connection);
        assert!(!pending.await?);
        let session = get_session(id, &pool).await?.unwrap();
        assert_eq!(session.status, "ready");
        assert_eq!(session.pinned_at, Some(now));
        let pending_files: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM recording_files WHERE delete_pending=1")
                .fetch_one(&pool)
                .await?;
        assert_eq!(pending_files, 0);
        assert_eq!(
            crate::db::camera_recording_segments::list_ready_segments(id, &pool)
                .await?
                .len(),
            1
        );
        soft_delete_session(id, &pool).await?;
        assert!(!toggle_pin(id, 1, &pool).await?);
        Ok(())
    }
}

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
    pub rule_id: Option<i64>,
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
    pub pinned_at: Option<DateTime<Utc>>,
    pub pinned_by: Option<i64>,
    pub pin_note: Option<String>,
    pub pre_roll_from: Option<DateTime<Utc>>,
    pub pre_roll_seconds: i64,
    pub pre_roll_attempted_seconds: i64,
    pub pre_roll_actual_seconds: i64,
    pub pre_roll_partial: i64,
    pub pre_roll_warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingArchiveFilter {
    All,
    Today,
    Week,
    Ready,
    Failed,
    Pinned,
    Rule(i64),
}

/// Import the already captured clip independently of any event-driven session.
/// Recheck the camera setting at the write boundary; manual imports never merge.
pub async fn create_manual_session(
    camera_id: i64,
    user_id: u64,
    seconds: u32,
    retention_days: u32,
    pool: &SqlitePool,
) -> Result<Option<i64>> {
    let now = Utc::now();
    Ok(sqlx::query_scalar(
        "INSERT INTO camera_recording_sessions(event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,started_at,expires_at,notification_sent_at)
         SELECT 'manual:' || lower(hex(randomblob(16))),NULL,id,?,'recording',?,?,?,?,?,?
         FROM cameras WHERE id=? AND enabled != 0 AND archive_manual_clips != 0 RETURNING id",
    )
    .bind(format!("Manual video · user #{user_id} · {seconds}s"))
    .bind(now - Duration::seconds(i64::from(seconds)))
    .bind(now)
    .bind(now)
    .bind(now - Duration::seconds(i64::from(seconds)))
    .bind(now + Duration::days(i64::from(retention_days.clamp(1,365))))
    .bind(now)
    .bind(camera_id)
    .fetch_optional(pool)
    .await?)
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
    // Acquire the writer lock before reading the active session. A deferred
    // read transaction can otherwise fail to upgrade if finalization races it.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let active = sqlx::query_as::<_, RecordingSession>(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE camera_id = ?
          AND deleted_at IS NULL
          AND status IN ('queued', 'recording')
          AND rule_id IS NOT NULL
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(rule.camera_id)
    .fetch_optional(&mut *tx)
    .await?;

    let stop_after_at = now + Duration::seconds(rule.tail_seconds);

    if let Some(active) = active {
        let stop_after_at = active.stop_after_at.max(stop_after_at);
        let expires_at = active
            .expires_at
            .max(now + Duration::days(rule.retention_days));
        let extended_ids = add_rule_id(active.extended_by_rule_ids.as_deref(), rule.id);
        let summary = append_summary(&active.trigger_summary, trigger_summary);
        let requested_pre_roll_seconds = if rule.pre_roll_enabled() {
            rule.normalized_pre_roll_seconds()
        } else {
            0
        };

        sqlx::query(
            r#"
            UPDATE camera_recording_sessions
            SET last_event_at = ?,
                stop_after_at = ?,
                extended_by_rule_ids = ?,
                trigger_summary = ?,
                pre_roll_seconds = MAX(pre_roll_seconds, ?),
                expires_at = ?
            WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(stop_after_at)
        .bind(extended_ids)
        .bind(summary)
        .bind(requested_pre_roll_seconds)
        .bind(expires_at)
        .bind(active.id)
        .execute(&mut *tx)
        .await?;

        // Existing parts and parts inserted after this transaction share the
        // session's longest retention promise, including an in-flight writer.
        sqlx::query("UPDATE camera_recording_segments SET expires_at = ? WHERE session_id = ? AND deleted_at IS NULL")
            .bind(expires_at)
            .bind(active.id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        return Ok(SessionAction::Extended(active.id));
    }

    // The matcher may have loaded the rule before another worker completed it.
    let last_completed_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_completed_at FROM camera_recording_rules WHERE id = ?")
            .bind(rule.id)
            .fetch_one(&mut *tx)
            .await?;
    if let Some(last_completed_at) = last_completed_at {
        let cooldown_until = last_completed_at + Duration::seconds(rule.cooldown_s);
        if cooldown_until > now {
            tx.commit().await?;
            return Ok(SessionAction::SkippedCooldown);
        }
    }

    let expires_at = now + Duration::days(rule.retention_days);
    let extended_ids = serde_json::to_string(&vec![rule.id])?;
    let pre_roll_seconds = if rule.pre_roll_enabled() {
        rule.normalized_pre_roll_seconds()
    } else {
        0
    };
    let result = sqlx::query(
        r#"
        INSERT INTO camera_recording_sessions (
            event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
            status, first_event_at, last_event_at, stop_after_at, expires_at, pre_roll_seconds
        )
        VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?)
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
    .bind(pre_roll_seconds)
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
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?)
}

#[allow(dead_code)]
pub async fn list_camera_sessions(
    camera_id: i64,
    limit: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingSession>> {
    list_camera_sessions_filtered(camera_id, RecordingArchiveFilter::All, limit, 0, pool).await
}

pub async fn list_camera_sessions_filtered(
    camera_id: i64,
    filter: RecordingArchiveFilter,
    limit: i64,
    offset: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingSession>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT id, event_group_id, rule_id, camera_id, extended_by_rule_ids, trigger_summary,
               status, error, first_event_at, last_event_at, stop_after_at, started_at,
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE camera_id =
        "#,
    );
    query.push(" ");
    query.push_bind(camera_id);
    query.push(" AND deleted_at IS NULL AND status != 'deleted'");

    match filter {
        RecordingArchiveFilter::All => {}
        RecordingArchiveFilter::Today => {
            let now = Local::now();
            let start_local = now
                .date_naive()
                .and_time(NaiveTime::MIN)
                .and_local_timezone(Local)
                .earliest()
                .unwrap_or(now);
            let end_local = start_local + Duration::days(1);
            query.push(" AND DATETIME(created_at) >= DATETIME(");
            query.push_bind(start_local.with_timezone(&Utc));
            query.push(") AND DATETIME(created_at) < DATETIME(");
            query.push_bind(end_local.with_timezone(&Utc));
            query.push(")");
        }
        RecordingArchiveFilter::Week => {
            query.push(" AND DATETIME(created_at) >= DATETIME(");
            query.push_bind(Utc::now() - Duration::days(7));
            query.push(")");
        }
        RecordingArchiveFilter::Ready => {
            query.push(" AND status = 'ready'");
        }
        RecordingArchiveFilter::Failed => {
            query.push(" AND status = 'failed'");
        }
        RecordingArchiveFilter::Pinned => {
            query.push(" AND pinned_at IS NOT NULL");
        }
        RecordingArchiveFilter::Rule(rule_id) => {
            query.push(
                r#"
                AND (
                    rule_id =
                "#,
            );
            query.push(" ");
            query.push_bind(rule_id);
            query.push(
                r#"
                    OR EXISTS (
                        SELECT 1
                        FROM json_each(COALESCE(extended_by_rule_ids, '[]'))
                        WHERE CAST(value AS INTEGER) =
                "#,
            );
            query.push(" ");
            query.push_bind(rule_id);
            query.push("))");
        }
    }

    query.push(
        r#"
        ORDER BY CASE WHEN pinned_at IS NULL THEN 1 ELSE 0 END,
                 created_at DESC, id DESC
        LIMIT
        "#,
    );
    query.push(" ");
    query.push_bind(limit);
    query.push(" OFFSET ");
    query.push_bind(offset);

    Ok(query
        .build_query_as::<RecordingSession>()
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
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE camera_id = ?
          AND deleted_at IS NULL
          AND status IN ('queued', 'recording')
          AND rule_id IS NOT NULL
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
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('queued', 'recording')
          AND rule_id IS NOT NULL
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
               completed_at, expires_at, notification_sent_at, deleted_at, created_at,
               pinned_at, pinned_by, pin_note, pre_roll_from, pre_roll_seconds,
               pre_roll_partial, pre_roll_warning, pre_roll_attempted_seconds, pre_roll_actual_seconds
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

pub async fn count_active_sessions(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status IN ('queued', 'recording')
        "#,
    )
    .fetch_one(pool)
    .await?)
}

pub async fn count_failed_sessions_since(since: DateTime<Utc>, pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM camera_recording_sessions
        WHERE deleted_at IS NULL
          AND status = 'failed'
          AND DATETIME(completed_at) >= DATETIME(?)
        "#,
    )
    .bind(since)
    .fetch_one(pool)
    .await?)
}

pub async fn mark_recording(session_id: i64, pool: &SqlitePool) -> Result<()> {
    let query = sqlx::query(
        r#"
        UPDATE camera_recording_sessions
        SET status = 'recording', started_at = COALESCE(started_at, ?), error = CASE WHEN julianday('now')-julianday(first_event_at)>2.0/86400 THEN 'Recording started late; requested interval is only partially covered' ELSE NULL END
        WHERE id = ? AND deleted_at IS NULL AND status IN ('queued','recording')
        "#,
    )
    .bind(Utc::now())
    .bind(session_id);
    let result = crate::db::log_slow_operation(
        "camera_recording_sessions.mark_recording",
        query.execute(pool),
    )
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "Recording session no longer active"
    );
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

/// Finish only a still-active session whose latest deadline has elapsed.
/// Returning false tells the worker to reload: an event may have extended it.
pub async fn mark_ready_if_due(
    session_id: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    crate::db::log_slow_operation(
        "camera_recording_sessions.mark_ready_if_due",
        mark_ready_if_due_inner(session_id, completed_at, pool),
    )
    .await
}

async fn mark_ready_if_due_inner(
    session_id: i64,
    completed_at: DateTime<Utc>,
    pool: &SqlitePool,
) -> Result<bool> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let finished = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE camera_recording_sessions
        SET status = CASE WHEN EXISTS(SELECT 1 FROM camera_recording_segments s WHERE s.session_id=camera_recording_sessions.id AND s.status='ready' AND s.deleted_at IS NULL) THEN 'ready' ELSE 'failed' END, completed_at = ?1, error = CASE WHEN EXISTS(SELECT 1 FROM camera_recording_segments s WHERE s.session_id=camera_recording_sessions.id AND s.status='ready' AND s.deleted_at IS NULL) THEN error ELSE 'recording interval expired without material' END
        WHERE id = ?2 AND deleted_at IS NULL AND status = 'recording'
          AND stop_after_at <= ?1
          AND pre_roll_attempted_seconds >= pre_roll_seconds
          AND NOT EXISTS(SELECT 1 FROM recording_writers w WHERE w.session_id=camera_recording_sessions.id)
        RETURNING id
        "#,
    )
    .bind(completed_at)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    if finished.is_some() {
        // Publish completion and its cooldown together, before accepting a new
        // session. An extension leaves both timestamps untouched.
        sqlx::query(
            r#"UPDATE camera_recording_rules
               SET last_completed_at = ?, updated_at = CURRENT_TIMESTAMP
               WHERE id IN (
                   SELECT rule_id FROM camera_recording_sessions WHERE id = ?
                   UNION
                   SELECT CAST(j.value AS INTEGER)
                   FROM camera_recording_sessions s,
                        json_each(CASE WHEN json_valid(s.extended_by_rule_ids)
                                       THEN s.extended_by_rule_ids ELSE '[]' END) j
                   WHERE s.id = ? AND j.type = 'integer'
               )"#,
        )
        .bind(completed_at)
        .bind(session_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(finished.is_some())
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

/// Record a completed attempt without weakening a concurrently merged request.
/// Failed attempts retain any previously published head and are retried only
/// when the requested window grows.
pub async fn mark_pre_roll_failure(
    session_id: i64,
    attempted_seconds: i64,
    warning: &str,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        "UPDATE camera_recording_sessions SET pre_roll_attempted_seconds=MAX(pre_roll_attempted_seconds,?),pre_roll_partial=1,pre_roll_warning=? WHERE id=? AND deleted_at IS NULL AND status IN ('queued','recording')",
    )
    .bind(attempted_seconds.max(0))
    .bind(crate::db::sanitize_error(warning))
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn toggle_pin(session_id: i64, user_id: i64, pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"UPDATE camera_recording_sessions
           SET pinned_at = CASE WHEN pinned_at IS NULL THEN ? ELSE NULL END,
               pinned_by = CASE WHEN pinned_at IS NULL THEN ? ELSE NULL END,
               pin_note = NULL
           WHERE id = ? AND deleted_at IS NULL AND status <> 'deleted'
           RETURNING pinned_at IS NOT NULL"#,
    )
    .bind(Utc::now())
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}

pub async fn soft_delete_session(session_id: i64, pool: &SqlitePool) -> Result<()> {
    soft_delete_session_inner(session_id, false, pool).await?;
    Ok(())
}

/// Automatic cleanup must recheck the pin after acquiring the write lock.
/// False means this candidate was pinned since it was selected.
pub async fn soft_delete_session_if_unpinned(session_id: i64, pool: &SqlitePool) -> Result<bool> {
    soft_delete_session_inner(session_id, true, pool).await
}

async fn soft_delete_session_inner(
    session_id: i64,
    only_unpinned: bool,
    pool: &SqlitePool,
) -> Result<bool> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM camera_recording_sessions WHERE id=?")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    anyhow::ensure!(
        !status
            .as_deref()
            .is_some_and(|s| s == "queued" || s == "recording"),
        "Сначала остановите запись и дождитесь завершения"
    );
    let marked = sqlx::query("UPDATE camera_recording_sessions SET status='deleted',deleted_at=COALESCE(deleted_at,?) WHERE id=? AND (?=0 OR pinned_at IS NULL)")
        .bind(Utc::now()).bind(session_id).bind(only_unpinned).execute(&mut *tx).await?.rows_affected();
    if marked == 0 {
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query("UPDATE camera_recording_segments SET status='deleted',deleted_at=COALESCE(deleted_at,?) WHERE session_id=?")
        .bind(Utc::now()).bind(session_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE recording_files SET delete_pending=1 WHERE session_id=?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn find_expired_sessions(pool: &SqlitePool) -> Result<Vec<RecordingSession>> {
    Ok(sqlx::query_as::<_, RecordingSession>(
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
