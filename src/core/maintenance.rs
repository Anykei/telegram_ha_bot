use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use log::{debug, error, info};
use teloxide::prelude::*;
use teloxide::types::{ChatId, MessageId};
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::bot::handlers::refresh_current_view;
use crate::db;
use crate::ha::models::Entity;
use crate::ha::Room;
use crate::models::AppConfig;

pub fn spawn_background_maintenance(
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    info!("Core: Notification processor started");

    tokio::spawn(async move {
        start_background_maintenance(bot, config, cancel_token).await;
    });
}

pub async fn start_background_maintenance(
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    let mut interval = interval(Duration::from_secs(
        config.background_maintenance_interval_s,
    ));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    info!("⚙️ Core: Worker Heartbeat View and Clear alerts started");

    loop {
        tokio::select! {
            _ = interval.tick() => {
                record_maintenance_tick(&config).await;

                let ttl = config.ttl_notifications;
                match db::device_event_log::EventLogger::purge_old_events(ttl, &config.db).await {
                    Ok(count) => if count > 0 { debug!("Maintenance: удалено {} старых записей лога", count); },
                    Err(e) => error!("Maintenance error: {}", e),
                }

                cleanup_expired_sessions(&config).await;
                cleanup_activity_log(&config).await;
                cleanup_camera_recordings(&config).await;
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

async fn cleanup_activity_log(config: &Arc<AppConfig>) {
    let retention_days =
        db::settings::get_i64(db::settings::ACTIVITY_LOG_RETENTION_DAYS, &config.db)
            .await
            .ok()
            .flatten()
            .unwrap_or(30);

    if retention_days <= 0 {
        return;
    }

    let cutoff = Utc::now() - ChronoDuration::days(retention_days);
    match db::activity_log::purge_older_than(cutoff, &config.db).await {
        Ok(count) if count > 0 => debug!("Maintenance: removed {} old activity log rows", count),
        Err(e) => error!("Activity log cleanup error: {}", e),
        _ => {}
    }
}

async fn refresh_system_data(config: &Arc<AppConfig>) {
    match config.ha_client.fetch_rooms().await {
        Ok(rooms) => match refresh_room(&rooms, config).await {
            Ok(()) => record_ha_sync_success(config).await,
            Err(e) => {
                let message = e.to_string();
                error!("Background sync error: {}", message);
                record_ha_sync_error(config, message).await;
            }
        },
        Err(e) => {
            let message = e.to_string();
            error!("Failed to fetch rooms from HA: {}", message);
            record_ha_sync_error(config, message).await;
        }
    }
}

async fn record_maintenance_tick(config: &Arc<AppConfig>) {
    config.runtime_status.write().await.last_maintenance_tick_at = Some(Utc::now());
}

async fn record_ha_sync_success(config: &Arc<AppConfig>) {
    let mut status = config.runtime_status.write().await;
    status.last_ha_sync_at = Some(Utc::now());
    status.last_ha_sync_error = None;
}

async fn record_ha_sync_error(config: &Arc<AppConfig>, error: String) {
    config.runtime_status.write().await.last_ha_sync_error = Some(error);
}

async fn refresh_room(rooms: &Vec<Room>, config: &Arc<AppConfig>) -> anyhow::Result<()> {
    let mut all_synced_entity_ids = Vec::new();

    for room in rooms {
        match db::rooms::sync_rooms_from_ha(&room.id, &room.name, &config.db).await {
            Ok(_) => {
                // Собираем все entity_id, которые были синхронизированы
                for entity in &room.entities {
                    all_synced_entity_ids.push(entity.entity_id.clone());
                }

                if let Err(e) = refresh_entities(&room.id, &room.entities, config).await {
                    error!("Failed to refresh entities for room {}: {}", room.id, e);
                }
            }
            Err(e) => error!("Failed to sync room {}: {}", room.id, e),
        }
    }

    // После синхронизации всех устройств архивируем те, которых больше нет
    match db::devices::archive_missing_devices(&all_synced_entity_ids, &config.db).await {
        Ok(count) if count > 0 => info!("Архивировано {} устройств", count),
        Err(e) => error!("Failed to archive missing devices: {}", e),
        _ => {}
    }

    Ok(())
}

async fn refresh_entities(
    area_id: &str,
    entities: &Vec<Entity>,
    config: &Arc<AppConfig>,
) -> anyhow::Result<()> {
    for ent in entities {
        let device_class = ent.device_class.as_deref().unwrap_or("undefined");

        if let Err(e) =
            db::devices::sync_device(&ent.entity_id, area_id, &ent.name, device_class, &config.db)
                .await
        {
            error!("Failed to sync device {}: {}", ent.entity_id, e);
        }
    }
    Ok(())
}

async fn cleanup_expired_sessions(config: &Arc<AppConfig>) {
    let ttl_hours = i64::try_from(config.session_ttl_hours).unwrap_or(i64::MAX);
    let cutoff = Utc::now() - ChronoDuration::hours(ttl_hours);

    let expired_user_ids: Vec<u64> = config
        .sessions
        .iter()
        .filter_map(|entry| {
            if entry.value().last_seen_at < cutoff {
                Some(*entry.key())
            } else {
                None
            }
        })
        .collect();

    if expired_user_ids.is_empty() {
        return;
    }

    for user_id in expired_user_ids {
        config.sessions.remove(&user_id);
        config.ui_locks.remove(&user_id);

        if let Err(e) = db::clear_user_session(user_id, &config.db).await {
            error!("Failed to clear expired session {}: {}", user_id, e);
        } else {
            debug!("Maintenance: cleared expired session {}", user_id);
        }
    }
}

async fn cleanup_camera_recordings(config: &Arc<AppConfig>) {
    match db::camera_recording_sessions::find_expired_sessions(&config.db).await {
        Ok(sessions) => {
            for session in sessions {
                if let Err(error) = crate::core::camera_recording::delete_recording_session_files(
                    config, session.id,
                )
                .await
                {
                    error!(
                        "Maintenance: failed to delete expired recording session {}: {}",
                        session.id, error
                    );
                }
            }
        }
        Err(error) => error!("Maintenance: failed to list expired recordings: {}", error),
    }

    let quota_mb = db::settings::get_i64(db::settings::CAMERA_RECORDING_MAX_STORAGE_MB, &config.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    if quota_mb <= 0 {
        return;
    }

    let quota_bytes = quota_mb.saturating_mul(1024 * 1024);
    loop {
        let size = match db::camera_recording_segments::sum_ready_size_bytes(&config.db).await {
            Ok(size) => size,
            Err(error) => {
                error!(
                    "Maintenance: failed to calculate recording quota: {}",
                    error
                );
                return;
            }
        };

        if size <= quota_bytes {
            break;
        }

        let sessions = match db::camera_recording_segments::list_deletable_sessions_for_quota(
            &config.db,
        )
        .await
        {
            Ok(sessions) => sessions,
            Err(error) => {
                error!("Maintenance: failed to list quota recordings: {}", error);
                return;
            }
        };

        let Some(session) = sessions.first() else {
            break;
        };

        if let Err(error) =
            crate::core::camera_recording::delete_recording_session_files(config, session.id).await
        {
            error!(
                "Maintenance: failed to delete quota recording session {}: {}",
                session.id, error
            );
            break;
        }
    }
}

async fn refresh_all_active_sessions(bot: &Bot, config: &Arc<AppConfig>) {
    if config.sessions.is_empty() {
        return;
    }

    debug!(
        "Heartbeat: refresh {} active session",
        config.sessions.len()
    );

    let now = Utc::now();

    for entry in config.sessions.iter() {
        let (user_id, session) = entry.pair();

        if session.is_ui_refresh_blocked(now) {
            continue;
        }

        let bot_clone = bot.clone();
        let config_clone = config.clone();

        let uid = *user_id;
        let mid = MessageId(session.last_menu_id);
        let ctx = session.current_context.clone();

        tokio::spawn(async move {
            if let Err(e) = refresh_current_view(
                &bot_clone,
                &config_clone,
                uid,
                ChatId(uid as i64),
                mid,
                &ctx,
            )
            .await
            {
                debug!("Fail update screen {}: {}", uid, e);
            }
        });
    }
}
