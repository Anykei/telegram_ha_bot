use std::sync::Arc;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use teloxide::prelude::*;
use teloxide::types::{ChatId, MessageId};
use log::{info, debug, error};

use crate::models::AppConfig;
use crate::db;
use crate::bot::handlers::render_current_view;

pub fn spawn_background_maintenance(
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    info!("Core: Notification processor started");

    tokio::spawn(async move {
        start_background_maintenance(
            bot,
            config,
            cancel_token
        ).await;
    });
}

pub async fn start_background_maintenance(
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    let mut interval = interval(Duration::from_secs(config.background_maintenance_interval_s));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    info!("⚙️ Core: Worker Heartbeat View and Clear alerts started");

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let ttl = config.leak_time_notification_m;
                match db::device_event_log::EventLogger::purge_old_events(&config.db, ttl).await {
                    Ok(count) => if count > 0 { debug!("Maintenance: удалено {} старых записей лога", count); },
                    Err(e) => error!("Maintenance error: {}", e),
                }

                refresh_all_active_sessions(&bot, &config).await;
            }
            _ = cancel_token.cancelled() => {
                info!("⚙️ Core: Worker was stopped.");
                break;
            }
        }
    }
}

async fn refresh_all_active_sessions(bot: &Bot, config: &Arc<AppConfig>) {
    if config.sessions.is_empty() {
        return;
    }

    debug!("Heartbeat: refresh {} active session", config.sessions.len());

    for entry in config.sessions.iter() {
        let (user_id, session) = entry.pair();
        let bot_clone = bot.clone();
        let config_clone = config.clone();

        let uid = *user_id;
        let mid = MessageId(session.last_menu_id);
        let ctx = session.current_context.clone();

        tokio::spawn(async move {
            if let Err(e) = render_current_view(
                &bot_clone,
                &config_clone,
                uid,
                ChatId(uid as i64),
                mid,
                &ctx
            ).await {
                debug!("Fail update screen {}: {}", uid, e);
            }
        });
    }
}