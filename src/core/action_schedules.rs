use crate::core::action_groups::{self, ActionActor};
use crate::db;
use crate::models::AppConfig;
use chrono::{Datelike, Local, Timelike};
use log::{error, info, warn};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

pub fn spawn_action_schedule_worker(
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        start_action_schedule_worker(config, cancel_token).await;
    })
}

pub async fn start_action_schedule_worker(config: Arc<AppConfig>, cancel_token: CancellationToken) {
    let mut interval = interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    info!("Core: Action schedule worker started");

    loop {
        tokio::select! {
            _ = interval.tick() => {
                run_due_action_schedules(config.clone()).await;
            }
            _ = cancel_token.cancelled() => {
                info!("Core: Action schedule worker stopped");
                break;
            }
        }
    }
}

pub async fn run_due_action_schedules(config: Arc<AppConfig>) {
    let now = Local::now();
    let time_minute = i64::from(now.hour()) * 60 + i64::from(now.minute());
    let day_bit = 1_i64 << now.weekday().num_days_from_monday();
    let run_key = now.format("%Y-%m-%d %H:%M").to_string();

    let schedules = match db::action_groups::list_due_action_schedules(
        time_minute,
        day_bit,
        &run_key,
        &config.db,
    )
    .await
    {
        Ok(schedules) => schedules,
        Err(error) => {
            error!(
                "Action schedule worker failed to list due schedules: {}",
                error
            );
            return;
        }
    };

    for schedule in schedules {
        let status = match run_one_schedule(config.clone(), &run_key, schedule).await {
            Ok(status) => status,
            Err(error) => {
                warn!("Action schedule run failed: {}", error);
                "error"
            }
        };
        if status == "error" {
            continue;
        }
    }
}

async fn run_one_schedule(
    config: Arc<AppConfig>,
    run_key: &str,
    schedule: db::action_groups::ActionSchedule,
) -> anyhow::Result<&'static str> {
    let schedule_id = schedule.id;
    let target = schedule.target_ref();
    let command = schedule.command();

    let status = match (target, command) {
        (Ok(target), Ok(command)) => {
            match action_groups::execute_action_target(
                target,
                command,
                ActionActor::Schedule { schedule_id },
                config.clone(),
            )
            .await
            {
                Ok(result) => {
                    log_schedule_run(
                        &config,
                        schedule_id,
                        result.status(),
                        &result.user_message(),
                    )
                    .await;
                    result.status()
                }
                Err(error) => {
                    let message = error.to_string();
                    log_schedule_run(&config, schedule_id, "error", &message).await;
                    "error"
                }
            }
        }
        (Err(error), _) | (_, Err(error)) => {
            let message = error.to_string();
            log_schedule_run(&config, schedule_id, "error", &message).await;
            "error"
        }
    };

    if let Err(error) =
        db::action_groups::mark_action_schedule_run(schedule_id, run_key, &config.db).await
    {
        error!(
            "Action schedule worker failed to mark schedule {} run: {}",
            schedule_id, error
        );
    }

    Ok(status)
}

async fn log_schedule_run(config: &Arc<AppConfig>, schedule_id: i64, status: &str, message: &str) {
    let entity_id = schedule_id.to_string();
    let _ = db::activity_log::log(
        db::activity_log::NewActivity {
            user_id: None,
            kind: "action_group",
            entity_type: "action_schedule",
            entity_id: Some(&entity_id),
            action: "action_group.schedule_run",
            status,
            message: Some(message),
        },
        &config.db,
    )
    .await;
}
