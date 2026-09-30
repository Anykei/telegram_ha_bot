use crate::core::action_groups;
use crate::db::action_groups::runs;
use crate::models::AppConfig;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use log::{error, info, warn};
use std::sync::Arc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

const EXECUTORS: usize = 4;
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(10);

pub fn spawn_action_schedule_worker(
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(start_action_schedule_worker(config, cancel_token))
}

pub async fn start_action_schedule_worker(config: Arc<AppConfig>, cancel_token: CancellationToken) {
    match runs::recover_started(&config.db).await {
        Ok(count) if count > 0 => warn!("schedule_recovery_unknown count={count}"),
        Ok(_) => {}
        Err(error) => {
            error!("Schedule recovery failed; execution disabled: {error}");
            return;
        }
    }
    let mut workers = JoinSet::new();
    for _ in 0..EXECUTORS {
        workers.spawn(executor(config.clone(), cancel_token.clone()));
    }
    let mut ticks = interval(Duration::from_secs(1));
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    info!("Action schedule planner started; interval=1s executors={EXECUTORS}");
    loop {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => break,
            _ = ticks.tick() => {
                if let Err(error) = plan_at(&config, Local::now()).await {
                    error!("Schedule planning failed: {error}");
                }
            }
        }
    }
    // Begun requests may finish after their planned minute. Shutdown uses a finite
    // budget; aborted requests become unknown, never pending again.
    if tokio::time::timeout(SHUTDOWN_BUDGET, async {
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                error!("Schedule executor stopped unexpectedly: {error}");
            }
        }
    })
    .await
    .is_err()
    {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        warn!("Schedule shutdown budget exceeded; interrupted sends remain unknown");
    }
    if let Err(error) = runs::recover_started(&config.db).await {
        error!("Failed to persist interrupted schedule outcomes: {error}");
    }
    info!("Action schedule worker stopped");
}

/// Only the current server-local minute is considered, including at startup.
/// UTC floor preserves the actual occurrence at a DST overlap; local key
/// uniqueness prevents a repeated local minute from gaining a new deadline.
pub(crate) async fn plan_at<Tz: TimeZone>(
    config: &AppConfig,
    now: DateTime<Tz>,
) -> anyhow::Result<()> {
    let now_utc = now.timestamp();
    runs::expire(now_utc, &config.db).await?;
    let time_minute = i64::from(now.hour()) * 60 + i64::from(now.minute());
    let day_bit = 1_i64 << now.weekday().num_days_from_monday();
    let local_run_key = now.naive_local().format("%Y-%m-%d %H:%M").to_string();
    let planned_at = now_utc - i64::from(now.second());
    let schedules = crate::db::action_groups::list_due_action_schedules(
        time_minute,
        day_bit,
        &local_run_key,
        &config.db,
    )
    .await?;
    for schedule in schedules {
        runs::reserve(&schedule, &local_run_key, planned_at, now_utc, &config.db).await?;
    }
    Ok(())
}

async fn executor(config: Arc<AppConfig>, cancel: CancellationToken) {
    let mut ticks = interval(Duration::from_millis(100));
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            _ = ticks.tick() => {}
        }
        let pending = match runs::pending(&config.db).await {
            Ok(pending) => pending,
            Err(error) => {
                error!("Schedule queue read failed: {error}");
                continue;
            }
        };
        for run in pending {
            if cancel.is_cancelled() {
                return;
            }
            if let Err(error) = execute_reserved(&config, &run).await {
                // A begun run is consumed even on a DB failure after dispatch.
                // Recovery records unknown; this loop only reads pending rows.
                error!("Schedule run {} failed: {error}", run.id);
                if let Err(recovery) = runs::recover_one(run.id, &config.db).await {
                    error!("Failed to persist interrupted run {}: {recovery}", run.id);
                }
            }
        }
    }
}

async fn execute_reserved(config: &Arc<AppConfig>, run: &runs::ScheduleRun) -> anyhow::Result<()> {
    execute_reserved_with_clock(config, run, || Utc::now().timestamp()).await
}

#[cfg(test)]
pub(crate) async fn execute_reserved_at(
    config: &Arc<AppConfig>,
    run: &runs::ScheduleRun,
    now: i64,
) -> anyhow::Result<()> {
    execute_reserved_with_clock(config, run, || now).await
}

pub(crate) async fn execute_reserved_with_clock(
    config: &Arc<AppConfig>,
    run: &runs::ScheduleRun,
    clock: impl Fn() -> i64,
) -> anyhow::Result<()> {
    let target = run.target()?;
    // Busy targets remain pending until their deadline without occupying a worker.
    // The planner expires them even while all executors are busy.
    let Some(_guard) = action_groups::try_lock_target(config, target) else {
        return Ok(());
    };
    if !runs::claim_with_clock(run, &clock, &config.db).await? {
        return Ok(());
    }
    let Some(result) = action_groups::execute_schedule_run(run, config.clone(), &clock).await?
    else {
        runs::skip_before_dispatch(run.id, clock(), &config.db).await?;
        return Ok(());
    };
    runs::finish(run.id, &result, &config.db).await?;
    let id = run.schedule_id.to_string();
    let status = match result.outcome {
        crate::core::action_results::Outcome::Sent => "sent",
        crate::core::action_results::Outcome::Partial => "partial",
        crate::core::action_results::Outcome::Failed => "rejected",
        crate::core::action_results::Outcome::Unknown => "unknown",
    };
    let message = result.summary(config.default_language);
    if let Err(error) = crate::db::activity_log::log(
        crate::db::activity_log::NewActivity {
            user_id: None,
            kind: "action_group",
            entity_type: "action_schedule",
            entity_id: Some(&id),
            action: "action_group.schedule_run",
            status,
            message: Some(&message),
        },
        &config.db,
    )
    .await
    {
        warn!("Schedule activity log failed; durable result retained: {error}");
    }
    Ok(())
}
