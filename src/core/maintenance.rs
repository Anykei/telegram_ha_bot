use std::sync::Arc;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use teloxide::prelude::*;
use teloxide::types::{ChatId, MessageId};
use log::{info, debug, error};

use crate::models::AppConfig;
use crate::db;
use crate::bot::handlers::render_current_view;
use crate::ha::models::Entity;
use crate::ha::Room;

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
                refresh_system_data(&config).await;
            }
            _ = cancel_token.cancelled() => {
                info!("⚙️ Core: Worker was stopped.");
                break;
            }
        }
    }
}

async fn refresh_system_data(config: &Arc<AppConfig>) {
    match config.ha_client.fetch_rooms().await {
        Ok(rooms) => {
            if let Err(e) = refresh_room(&rooms, config).await {
                error!("Background sync error: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to fetch rooms from HA: {}", e);
        }
    }
}

async fn refresh_room(rooms: &Vec<Room>, config: &Arc<AppConfig>) -> anyhow::Result<()> {
    for room in rooms {
        match db::rooms::sync_rooms_from_ha(&config.db, &room.id, &room.name).await {
            Ok(_) => {
                if let Err(e) = refresh_entities(&room.id, &room.entities, config).await {
                    error!("Failed to refresh entities for room {}: {}", room.id, e);
                }
            }
            Err(e) => error!("Failed to sync room {}: {}", room.id, e),
        }
    }
    Ok(())
}

async fn refresh_entities(area_id: &str, entities: &Vec<Entity>, config: &Arc<AppConfig>) -> anyhow::Result<()> {
    for ent in entities {
        let device_class = ent.device_class
            .as_deref()
            .unwrap_or("undefined");

        if let Err(e) = db::devices::sync_device(
            &config.db,
            &ent.entity_id,
            area_id,
            &ent.name,
            device_class,
        ).await {
            error!("Failed to sync device {}: {}", ent.entity_id, e);
        }
    }
    Ok(())
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