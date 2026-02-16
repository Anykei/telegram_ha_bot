use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use std::sync::Arc;
use log::{info, error, warn};
use teloxide::Bot;
use teloxide::prelude::ChatId;
use teloxide::types::MessageId;

use crate::bot::router::{ControlPayload, Payload};
use crate::models::{AppConfig, NotificationData, UserSession};
use crate::db;
use crate::ha::NotifyEvent;

pub fn spawn_notification_processor(
    mut rx: mpsc::Receiver<NotifyEvent>,
    bot: Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    info!("Core: Notification processor started");

    tokio::spawn(async move {
        // Create bounded task queue (max 32 events waiting)
        let (queue_tx, queue_rx) = mpsc::channel::<NotifyEvent>(32);
        
        // Spawn worker task that consumes from bounded queue
        let worker_config = config.clone();
        let worker_bot = bot.clone();
        let worker_cancel = cancel_token.clone();
        
        tokio::spawn(async move {
            let mut queue = queue_rx;
            loop {
                tokio::select! {
                    Some(event) = queue.recv() => {
                        if let Err(e) = process_and_dispatch(worker_bot.clone(), worker_config.clone(), event).await {
                            error!("Core: Error processing event: {}", e);
                        }
                    }
                    _ = worker_cancel.cancelled() => break,
                }
            }
        });
        
        // Main loop: forward events from HA to bounded queue
        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Send to bounded queue (returns error if queue is full)
                    if let Err(e) = queue_tx.send(event).await {
                        warn!("Core: Event queue full (capacity 32), dropping event: {}", e);
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!("Core: Notification processor shutting down");
                    break;
                }
            }
        }
    });
}

async fn process_and_dispatch(bot: Bot, config: Arc<AppConfig>, event: NotifyEvent) -> anyhow::Result<()> {
    if event.new_state == event.old_state {
        return Ok(());
    }
    info!("Core: New state change {}", event.entity_id, );

    db::device_event_log::EventLogger::record_event(&event.entity_id, &event.new_state, &config.db, ).await?;

    let room_id_opt = db::devices::get_room_id_by_entity(&event.entity_id, &config.db).await.unwrap_or(None);

    let recipients = db::subscriptions::get_subscribers(&event.entity_id, &config.db).await.unwrap_or_default();

    let recipients_set: std::collections::HashSet<u64> = recipients.iter().map(|&id| id as u64).collect();

    for entry in config.sessions.iter() {
        let user_id = *entry.key();
        let session = entry.value();

        let is_watching = room_id_opt.map_or(false, |rid| is_user_watching_room(session, rid));

        let is_subscriber = recipients_set.contains(&user_id);

        if is_watching || is_subscriber {
            let b = bot.clone();
            let c = config.clone();
            let mid = MessageId(session.last_menu_id);
            let ctx_str = session.current_context.clone();

            tokio::spawn(async move {
                let _ = crate::bot::handlers::render_current_view(
                    &b, &c, user_id, ChatId(user_id as i64), mid, &ctx_str
                ).await;
            });
        }
    }

    // let recipients = db::subscriptions::get_subscribers(&config.db, &event.entity_id).await?;
    if !recipients.is_empty() {
        use crate::core::presentation::StateFormatter;

        let room_prefix = if let Some(rid) = room_id_opt {
            if let Ok(Some(room)) = db::rooms::get_room_by_id(rid, &config.db).await {
                format!("*{}* • ", room.alias.as_deref().unwrap_or(&room.area))
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        };

        // Определяем домен и класс для форматирования
        let domain = event.entity_id.split('.').next().unwrap_or("");
        let class = event.device_class.as_deref().unwrap_or("");

        // Используем наше ядро для красоты
        let icon = StateFormatter::get_icon(domain, class, &event.new_state);
        let human_state = StateFormatter::format_state_value(domain, class, &event.new_state);

        let display_name = config.name_aliases.get(&event.entity_id)
            .map(|r| r.value().clone())
            .unwrap_or_else(|| event.friendly_name.clone());

        let message_text = format!("{}{} {}: *{}*", icon, room_prefix, display_name, human_state);

        let data = NotificationData {
            display_name,
            human_state: message_text,
            entity_id: event.entity_id.clone(),
            recipients,
        };

        let b_clone = bot.clone();
        let c_clone = config.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::bot::notification::send_notification(b_clone, c_clone, data).await {
                error!("Error sending notification: {}", e);
            }
        });
    }
    Ok(())
}

pub async fn refresh_live_interface_for_recipients(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: i64,
) {
    if let Some(session) = config.sessions.get(&(user_id as u64)) {
        let b = bot.clone();
        let c = config.clone();
        let mid = MessageId(session.last_menu_id);
        let ctx = session.current_context.clone();

        info!("Updating live interfaces for {}", user_id);
        tokio::spawn(async move {
            let _ = crate::bot::handlers::render_current_view(
                &b, &c, user_id as u64, ChatId(user_id), mid, &ctx
            ).await;
        });
    }
}

fn is_user_watching_room(session: &UserSession, room_id: i64) -> bool {
    if let Ok(payload) = serde_json::from_str::<Payload>(&session.current_context) {
        match payload {
            Payload::Control(ControlPayload::RoomDetail { room }) => room == room_id,
            Payload::Control(ControlPayload::DeviceControl { room, .. }) => room == room_id,
            _ => false,
        }
    } else {
        false
    }
}