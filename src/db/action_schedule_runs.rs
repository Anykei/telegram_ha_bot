//! Durable schedule reservations. SQLite owns uniqueness and the right to dispatch;
//! a started run is never eligible for a second attempt, even after a crash.
use super::{ActionSchedule, ActionScheduleCommand, ActionTargetRef};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

pub const MAX_PENDING_RUNS: i64 = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct RunTarget {
    pub id: i64,
    pub entity_id: String,
    pub domain: String,
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)]
pub struct ScheduleRun {
    pub id: i64,
    pub schedule_id: i64,
    pub local_run_key: String,
    pub planned_at: i64,
    pub deadline_at: i64,
    pub revision: i64,
    pub target_type: String,
    pub target_id: i64,
    pub command: String,
    pub snapshot_json: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub result_json: String,
    pub reason: Option<String>,
}

impl ScheduleRun {
    pub fn target(&self) -> Result<ActionTargetRef> {
        match self.target_type.as_str() {
            "bot_group" => Ok(ActionTargetRef::BotGroup(self.target_id)),
            "ha_native" => Ok(ActionTargetRef::HaNative(self.target_id)),
            _ => anyhow::bail!("Invalid schedule target"),
        }
    }

    pub fn targets(&self) -> Result<Vec<RunTarget>> {
        Ok(serde_json::from_str(&self.snapshot_json)?)
    }
}

async fn target_snapshot(
    target_type: &str,
    target_id: i64,
    command: &str,
    conn: &mut SqliteConnection,
) -> Result<Option<Vec<RunTarget>>> {
    let command = ActionScheduleCommand::from_db(command)?;
    if target_type == "bot_group"
        && matches!(
            command,
            ActionScheduleCommand::TurnOn | ActionScheduleCommand::TurnOff
        )
    {
        let enabled =
            sqlx::query_scalar::<_, i64>("SELECT enabled FROM action_groups WHERE id = ?")
                .bind(target_id)
                .fetch_optional(&mut *conn)
                .await?;
        if enabled != Some(1) {
            return Ok(None);
        }
        let targets = sqlx::query_as::<_, RunTarget>(
            "SELECT d.id, d.entity_id, COALESCE(NULLIF(TRIM(d.device_domain), ''), substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)) AS domain FROM action_group_items i
             JOIN devices d ON d.id = i.device_id
             WHERE i.group_id = ? AND COALESCE(d.archived, 0) = 0 AND COALESCE(NULLIF(TRIM(d.device_domain), ''), substr(d.entity_id, 1, instr(d.entity_id, '.') - 1)) IN ('light', 'switch')
             ORDER BY d.id",
        )
        .bind(target_id)
        .fetch_all(conn)
        .await?;
        return Ok(Some(targets));
    }
    if target_type == "ha_native" && command == ActionScheduleCommand::Execute {
        return Ok(sqlx::query_as::<_, RunTarget>(
            "SELECT id, entity_id, domain FROM ha_native_targets WHERE id = ?
             AND enabled = 1 AND archived = 0 AND domain IN ('script', 'scene')",
        )
        .bind(target_id)
        .fetch_optional(conn)
        .await?
        .map(|target| vec![target]));
    }
    Ok(None)
}

/// Insert the snapshot and reserve the local minute in one short write transaction.
/// A full queue records a terminal skip instead of growing an unbounded backlog.
pub async fn reserve(
    schedule: &ActionSchedule,
    local_run_key: &str,
    planned_at: i64,
    now: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = sqlx::query_as::<_, (i64, String, i64, String)>(
        "SELECT revision, target_type, target_id, command FROM action_schedules
         WHERE id = ? AND enabled = 1 AND time_minute = ? AND days_mask = ?
         AND COALESCE(last_run_at, '') != ?",
    )
    .bind(schedule.id)
    .bind(schedule.time_minute)
    .bind(schedule.days_mask)
    .bind(local_run_key)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((revision, target_type, target_id, command)) = current else {
        return Ok(false);
    };
    let Some(targets) = target_snapshot(&target_type, target_id, &command, &mut tx).await? else {
        return Ok(false);
    };
    let snapshot = serde_json::to_string(&targets)?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM action_schedule_runs WHERE status = 'pending'")
            .fetch_one(&mut *tx)
            .await?;
    let reason = if now >= planned_at + 60 {
        Some("deadline_expired")
    } else if targets.is_empty() {
        Some("no_available_targets")
    } else if count >= MAX_PENDING_RUNS {
        Some("queue_full")
    } else {
        None
    };
    let result = sqlx::query(
        "INSERT INTO action_schedule_runs
         (schedule_id, local_run_key, planned_at, deadline_at, revision, target_type,
          target_id, command, snapshot_json, status, reason, finished_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(schedule_id, local_run_key) DO NOTHING",
    )
    .bind(schedule.id)
    .bind(local_run_key)
    .bind(planned_at)
    .bind(planned_at + 60)
    .bind(revision)
    .bind(target_type)
    .bind(target_id)
    .bind(command)
    .bind(snapshot)
    .bind(if reason.is_some() {
        "skipped"
    } else {
        "pending"
    })
    .bind(reason)
    .bind(reason.map(|_| now))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn pending(pool: &SqlitePool) -> Result<Vec<ScheduleRun>> {
    Ok(sqlx::query_as("SELECT * FROM action_schedule_runs WHERE status = 'pending' ORDER BY deadline_at, id LIMIT 256")
        .fetch_all(pool).await?)
}

async fn valid_snapshot(run: &ScheduleRun, conn: &mut SqliteConnection) -> Result<bool> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM action_schedules WHERE id = ? AND enabled = 1
         AND revision = ? AND target_type = ? AND target_id = ? AND command = ?)",
    )
    .bind(run.schedule_id)
    .bind(run.revision)
    .bind(&run.target_type)
    .bind(run.target_id)
    .bind(&run.command)
    .fetch_one(&mut *conn)
    .await?;
    if !valid {
        return Ok(false);
    }
    Ok(
        target_snapshot(&run.target_type, run.target_id, &run.command, conn).await?
            == Some(run.targets()?),
    )
}

/// The target mutex is acquired before this operation. No network request or lock
/// wait is inside the transaction, and only the winner may contact HA.
#[cfg(test)]
pub async fn claim(run: &ScheduleRun, now: i64, pool: &SqlitePool) -> Result<bool> {
    claim_with_clock(run, || now, pool).await
}

pub async fn claim_with_clock(
    run: &ScheduleRun,
    clock: impl Fn() -> i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM action_schedule_runs WHERE id = ? AND status = 'pending')",
    )
    .bind(run.id)
    .fetch_one(&mut *tx)
    .await?;
    if !pending {
        return Ok(false);
    }
    let valid = valid_snapshot(run, &mut tx).await?;
    // Acquiring SQLite's write lock and validating the snapshot may wait. Read
    // the clock here, never before BEGIN IMMEDIATE, or stale work could start.
    let now = clock();
    let reason = if now >= run.deadline_at {
        Some("deadline_expired")
    } else if !valid {
        Some("schedule_or_target_changed")
    } else {
        None
    };
    if let Some(reason) = reason {
        sqlx::query("UPDATE action_schedule_runs SET status = 'skipped', reason = ?, finished_at = ? WHERE id = ? AND status = 'pending'")
            .bind(reason).bind(now).bind(run.id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query("UPDATE action_schedule_runs SET status = 'executing', started_at = ? WHERE id = ? AND status = 'pending'")
        .bind(now).bind(run.id).execute(&mut *tx).await?;
    sqlx::query("UPDATE action_schedules SET last_run_at = ? WHERE id = ?")
        .bind(&run.local_run_key)
        .bind(run.schedule_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Recheck between group members so edits/disable cannot start another request.
pub async fn still_current(run: &ScheduleRun, pool: &SqlitePool) -> Result<bool> {
    valid_snapshot(run, &mut *pool.acquire().await?).await
}

pub async fn save_progress(
    id: i64,
    targets: &[crate::core::action_results::TargetResult],
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        "UPDATE action_schedule_runs SET result_json = ? WHERE id = ? AND status = 'executing'",
    )
    .bind(serde_json::to_string(targets)?)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish(
    id: i64,
    result: &crate::core::action_results::ActionResult,
    pool: &SqlitePool,
) -> Result<()> {
    use crate::core::action_results::Outcome;
    let status = match result.outcome {
        Outcome::Sent => "sent",
        Outcome::Partial => "partial",
        Outcome::Failed => "rejected",
        Outcome::Unknown => "unknown",
    };
    sqlx::query("UPDATE action_schedule_runs SET status = ?, result_json = ?, finished_at = ? WHERE id = ? AND status = 'executing'")
        .bind(status).bind(serde_json::to_string(&result.targets)?).bind(chrono::Utc::now().timestamp())
        .bind(id).execute(pool).await?;
    Ok(())
}

/// A claim can commit before the deadline while the first dispatch validation
/// finishes after it. No request was sent; keep the terminal reason explicit.
pub async fn skip_before_dispatch(id: i64, now: i64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE action_schedule_runs SET status = 'skipped', reason = 'deadline_expired', finished_at = ? WHERE id = ? AND status = 'executing'")
        .bind(now).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn expire(now: i64, pool: &SqlitePool) -> Result<u64> {
    Ok(sqlx::query("UPDATE action_schedule_runs SET status = 'skipped', reason = 'deadline_expired', finished_at = ? WHERE status = 'pending' AND deadline_at <= ?")
        .bind(now).bind(now).execute(pool).await?.rows_affected())
}

pub async fn recover_started(pool: &SqlitePool) -> Result<u64> {
    recover_interrupted(None, pool).await
}

pub async fn recover_one(id: i64, pool: &SqlitePool) -> Result<u64> {
    recover_interrupted(Some(id), pool).await
}

async fn recover_interrupted(id: Option<i64>, pool: &SqlitePool) -> Result<u64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let runs: Vec<ScheduleRun> = sqlx::query_as(
        "SELECT * FROM action_schedule_runs WHERE status = 'executing' AND (? IS NULL OR id = ?)",
    )
    .bind(id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    for run in &runs {
        use crate::core::action_results::{Outcome, TargetKind, TargetResult};
        let mut results: Vec<TargetResult> =
            serde_json::from_str(&run.result_json).context("Invalid persisted schedule results")?;
        let kind = if run.target_type == "bot_group" {
            TargetKind::Device
        } else {
            TargetKind::HaNative
        };
        for target in run.targets()? {
            if !results
                .iter()
                .any(|result| result.id == target.id && result.kind == kind)
            {
                results.push(TargetResult::new(
                    kind,
                    target.id,
                    Outcome::Unknown,
                    Some("Process stopped after execution began; never retried"),
                ));
            }
        }
        sqlx::query("UPDATE action_schedule_runs SET status = 'unknown', reason = 'interrupted_after_start', result_json = ?, finished_at = ? WHERE id = ? AND status = 'executing'")
            .bind(serde_json::to_string(&results)?).bind(chrono::Utc::now().timestamp()).bind(run.id)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(runs.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::action_groups::{self, ALL_DAYS_MASK};

    async fn fixture() -> Result<(SqlitePool, ActionSchedule)> {
        let config = crate::test_support::config().await;
        let pool = config.db.clone();
        let group = action_groups::create_action_group("Lights", &pool).await?;
        sqlx::query("INSERT INTO rooms (id, area) VALUES (1, 'kitchen')")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO devices (id, room_id, entity_id, device_domain, archived) VALUES (1, 1, 'light.first', 'light', 0), (2, 1, 'light.second', 'light', 0)")
            .execute(&pool).await?;
        sqlx::query("INSERT INTO action_group_items (group_id, device_id) VALUES (?, 1), (?, 2)")
            .bind(group)
            .bind(group)
            .execute(&pool)
            .await?;
        let id = action_groups::create_action_schedule(
            ActionTargetRef::BotGroup(group),
            ActionScheduleCommand::TurnOn,
            720,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;
        Ok((
            pool.clone(),
            action_groups::get_action_schedule(id, &pool)
                .await?
                .unwrap(),
        ))
    }

    async fn get(id: i64, pool: &SqlitePool) -> Result<ScheduleRun> {
        Ok(
            sqlx::query_as("SELECT * FROM action_schedule_runs WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?,
        )
    }

    #[tokio::test]
    async fn unique_reservation_atomic_claim_and_restart_never_replay() -> Result<()> {
        let (pool, schedule) = fixture().await?;
        // Crash before reservation leaves nothing to recover.
        assert_eq!(recover_started(&pool).await?, 0);
        let (a, b) = tokio::join!(
            reserve(&schedule, "2026-09-30 12:00", 1000, 1050, &pool),
            reserve(&schedule, "2026-09-30 12:00", 1000, 1050, &pool)
        );
        assert_ne!(a?, b?);
        let run = pending(&pool).await?.remove(0);
        assert_eq!(run.deadline_at, 1060);
        // A reserved run survives restart within its original minute.
        assert_eq!(recover_started(&pool).await?, 0);
        let (a, b) = tokio::join!(claim(&run, 1051, &pool), claim(&run, 1051, &pool));
        assert_ne!(a?, b?);
        // Crash after claim, including after HA acceptance but before result save.
        assert_eq!(recover_started(&pool).await?, 1);
        let recovered = get(run.id, &pool).await?;
        assert_eq!(recovered.status, "unknown");
        assert_eq!(recovered.reason.as_deref(), Some("interrupted_after_start"));
        assert!(!claim(&run, 1052, &pool).await?);
        assert!(!reserve(&schedule, &run.local_run_key, 1000, 1052, &pool).await?);
        assert_eq!(pending(&pool).await?.len(), 0);
        let outcomes: Vec<crate::core::action_results::TargetResult> =
            serde_json::from_str(&recovered.result_json)?;
        assert!(outcomes
            .iter()
            .all(|target| target.outcome == crate::core::action_results::Outcome::Unknown));
        assert_eq!(outcomes.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn crash_preserves_completed_group_members_and_pending_expires() -> Result<()> {
        use crate::core::action_results::{Outcome, TargetKind, TargetResult};
        let (pool, schedule) = fixture().await?;
        reserve(&schedule, "2026-09-30 12:00", 1000, 1059, &pool).await?;
        let run = pending(&pool).await?.remove(0);
        assert!(claim(&run, 1059, &pool).await?);
        save_progress(
            run.id,
            &[TargetResult::new(
                TargetKind::Device,
                1,
                Outcome::Sent,
                None,
            )],
            &pool,
        )
        .await?;
        recover_started(&pool).await?;
        let recovered = get(run.id, &pool).await?;
        let results: Vec<TargetResult> = serde_json::from_str(&recovered.result_json)?;
        assert_eq!(results[0].outcome, Outcome::Sent);
        assert_eq!(results[1].outcome, Outcome::Unknown);
        assert_eq!(recovered.status, "unknown");
        reserve(&schedule, "2026-10-01 12:00", 2000, 2059, &pool).await?;
        let pending_run = pending(&pool).await?.remove(0);
        assert!(!claim(&pending_run, 2060, &pool).await?);
        assert_eq!(get(pending_run.id, &pool).await?.status, "skipped");
        // Repeated local minute at a DST overlap cannot extend the deadline.
        assert!(!reserve(&schedule, "2026-10-01 12:00", 5600, 5610, &pool).await?);
        let unchanged = get(pending_run.id, &pool).await?;
        assert_eq!(unchanged.deadline_at, 2060);
        assert_eq!(unchanged.status, "skipped");
        Ok(())
    }

    #[tokio::test]
    async fn changed_or_disabled_pending_schedule_and_target_are_skipped() -> Result<()> {
        for change in [
            "UPDATE action_schedules SET enabled = 0",
            "UPDATE action_schedules SET command = 'turn_off'",
            "UPDATE action_schedules SET days_mask = 1",
            "UPDATE action_schedules SET time_minute = 721",
            "UPDATE action_groups SET enabled = 0",
            "DELETE FROM action_group_items WHERE device_id = 2",
            "UPDATE devices SET entity_id = 'light.replaced' WHERE id = 2",
            "UPDATE devices SET archived = 1 WHERE id = 2",
            "DELETE FROM action_schedules",
        ] {
            let (pool, schedule) = fixture().await?;
            reserve(&schedule, "2026-09-30 12:00", 1000, 1001, &pool).await?;
            let run = pending(&pool).await?.remove(0);
            sqlx::query(change).execute(&pool).await?;
            assert!(!claim(&run, 1002, &pool).await?, "{change}");
            assert_eq!(get(run.id, &pool).await?.status, "skipped", "{change}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn legacy_current_minute_is_respected_and_queue_is_bounded() -> Result<()> {
        let (pool, schedule) = fixture().await?;
        action_groups::mark_action_schedule_run(schedule.id, "2026-09-30 12:00", &pool).await?;
        assert!(!reserve(&schedule, "2026-09-30 12:00", 1000, 1001, &pool).await?);
        // Separate schedule IDs with the same parameters retain separate runs.
        let second = action_groups::create_action_schedule(
            schedule.target_ref()?,
            schedule.command()?,
            720,
            ALL_DAYS_MASK,
            &pool,
        )
        .await?;
        let second = action_groups::get_action_schedule(second, &pool)
            .await?
            .unwrap();
        assert!(reserve(&second, "2026-09-30 12:00", 1000, 1001, &pool).await?);
        for i in 1..=MAX_PENDING_RUNS {
            reserve(&schedule, &format!("minute-{i}"), 1000, 1001, &pool).await?;
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM action_schedule_runs WHERE status = 'pending'"
            )
            .fetch_one(&pool)
            .await?,
            MAX_PENDING_RUNS
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM action_schedule_runs WHERE reason = 'queue_full'"
            )
            .fetch_one(&pool)
            .await?,
            1
        );
        assert_eq!(expire(1060, &pool).await?, MAX_PENDING_RUNS as u64);
        assert!(pending(&pool).await?.is_empty());
        Ok(())
    }
    #[tokio::test]
    async fn reopening_sqlite_preserves_consumed_run_and_known_member_results() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "schedule-runs-{}-{}.sqlite",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts.clone())
            .await?;
        sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
            .await?
            .run(&pool)
            .await?;
        sqlx::query("INSERT INTO ha_native_targets (id, entity_id, domain, ha_name) VALUES (1, 'script.test', 'script', 'Test')").execute(&pool).await?;
        let schedule = action_groups::create_action_schedule(
            ActionTargetRef::HaNative(1),
            ActionScheduleCommand::Execute,
            720,
            127,
            &pool,
        )
        .await?;
        let schedule = action_groups::get_action_schedule(schedule, &pool)
            .await?
            .unwrap();
        reserve(&schedule, "2026-09-30 12:00", 1000, 1001, &pool).await?;
        let run = pending(&pool).await?.remove(0);
        assert!(claim(&run, 1001, &pool).await?);
        pool.close().await;
        let reopened = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        assert_eq!(recover_started(&reopened).await?, 1);
        assert!(!claim(&run, 1002, &reopened).await?);
        assert!(!reserve(&schedule, &run.local_run_key, 1000, 1002, &reopened).await?);
        assert_eq!(get(run.id, &reopened).await?.status, "unknown");
        reopened.close().await;
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[tokio::test]
    async fn waiting_for_sqlite_cannot_start_after_the_deadline() -> Result<()> {
        use std::sync::{
            atomic::{AtomicI64, Ordering},
            Arc,
        };
        let (pool, schedule) = fixture().await?;
        reserve(&schedule, "2026-09-30 12:00", 1000, 1050, &pool).await?;
        let run = pending(&pool).await?.remove(0);
        let lock = pool.begin_with("BEGIN IMMEDIATE").await?;
        let clock = Arc::new(AtomicI64::new(1050));
        let claim = {
            let pool = pool.clone();
            let run = run.clone();
            let clock = clock.clone();
            tokio::spawn(async move {
                claim_with_clock(&run, || clock.load(Ordering::SeqCst), &pool).await
            })
        };
        tokio::task::yield_now().await;
        assert!(!claim.is_finished());
        clock.store(1060, Ordering::SeqCst);
        lock.rollback().await?;
        assert!(!claim.await??);
        assert_eq!(get(run.id, &pool).await?.status, "skipped");
        Ok(())
    }
}
