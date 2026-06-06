use log::{error, info, warn};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use teloxide::prelude::ChatId;
use teloxide::types::MessageId;
use teloxide::Bot;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::bot::router::{ControlPayload, Payload};
use crate::bot::utils::md;
use crate::db;
use crate::ha::NotifyEvent;
use crate::models::{AppConfig, NotificationData, UserSession};

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

async fn process_and_dispatch(
    bot: Bot,
    config: Arc<AppConfig>,
    event: NotifyEvent,
) -> anyhow::Result<()> {
    if event.new_state == event.old_state {
        return Ok(());
    }
    info!("Core: New state change {}", event.entity_id,);

    db::device_event_log::EventLogger::record_event(&event.entity_id, &event.new_state, &config.db)
        .await?;

    if let Err(error) =
        crate::core::camera_recording_matcher::process_event(config.clone(), &event).await
    {
        error!("Core: camera recording matcher failed: {}", error);
    }

    let room_id_opt = db::devices::get_room_id_by_entity(&event.entity_id, &config.db)
        .await
        .unwrap_or(None);

    let recipients = db::subscriptions::get_subscribers(&event.entity_id, &config.db)
        .await
        .unwrap_or_default();

    let recipients_set: std::collections::HashSet<u64> =
        recipients.iter().map(|&id| id as u64).collect();

    let now = Utc::now();
    let mut refresh_targets = Vec::new();

    for entry in config.sessions.iter() {
        let user_id = *entry.key();
        let session = entry.value();

        let is_watching = room_id_opt.is_some_and(|rid| is_user_watching_room(session, rid));

        let is_subscriber = recipients_set.contains(&user_id);

        if (is_watching || is_subscriber)
            && !session.is_ui_refresh_blocked(now)
            && can_refresh_after_event(session, now, config.event_refresh_min_interval_s)
        {
            refresh_targets.push((
                user_id,
                MessageId(session.last_menu_id),
                session.current_context.clone(),
            ));
        }
    }

    for (user_id, message_id, context) in refresh_targets {
        if let Some(mut session) = config.sessions.get_mut(&user_id) {
            session.last_ui_refresh_at = Some(now);
        }

        let b = bot.clone();
        let c = config.clone();

        tokio::spawn(async move {
            let _ = crate::bot::handlers::refresh_current_view(
                &b,
                &c,
                user_id,
                ChatId(user_id as i64),
                message_id,
                &context,
            )
            .await;
        });
    }

    // let recipients = db::subscriptions::get_subscribers(&config.db, &event.entity_id).await?;
    if !recipients.is_empty() {
        use crate::core::presentation::StateFormatter;

        let room_prefix = if let Some(rid) = room_id_opt {
            if let Ok(Some(room)) = db::rooms::get_room_by_id(rid, &config.db).await {
                format!(
                    "{} • ",
                    md::bold(room.alias.as_deref().unwrap_or(&room.area))
                )
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
        let inverted = db::devices::is_state_inverted(&event.entity_id, &config.db)
            .await
            .unwrap_or(false);
        let logical_state = StateFormatter::logical_state(&event.new_state, inverted);
        let state_alias =
            config.state_alias_for_display(&event.entity_id, &event.new_state, inverted);
        let icon = StateFormatter::get_icon(domain, class, &logical_state);
        let human_state = StateFormatter::format_state_value_with_alias(
            domain,
            class,
            &event.new_state,
            inverted,
            state_alias.as_deref(),
        );

        let display_name = config
            .name_aliases
            .get(&event.entity_id)
            .map(|r| r.value().clone())
            .unwrap_or_else(|| event.friendly_name.clone());

        let message_text = format!(
            "{}{} {}: {}",
            icon,
            room_prefix,
            md::plain(&display_name),
            md::bold(&human_state)
        );

        let data = NotificationData {
            human_state: message_text,
            recipients,
        };

        let b_clone = bot.clone();
        let c_clone = config.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::bot::notification::send_notification(b_clone, c_clone, data).await
            {
                error!("Error sending notification: {}", e);
            }
        });
    }
    Ok(())
}

fn is_user_watching_room(session: &UserSession, room_id: i64) -> bool {
    if let Ok(payload) = Payload::from_string(&session.current_context) {
        match payload {
            Payload::Control(ControlPayload::RoomDetail { room }) => room == room_id,
            Payload::Control(ControlPayload::DeviceControl { room, .. }) => room == room_id,
            Payload::Control(ControlPayload::QuickAction { room, .. }) => room == room_id,
            _ => false,
        }
    } else {
        false
    }
}

fn can_refresh_after_event(session: &UserSession, now: DateTime<Utc>, min_interval_s: u64) -> bool {
    session
        .last_ui_refresh_at
        .map(|last_refresh| {
            now.signed_duration_since(last_refresh).num_seconds() >= min_interval_s as i64
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn session_for(payload: Payload) -> UserSession {
        UserSession {
            last_menu_id: 1,
            current_context: payload.to_string(),
            header_entities: HashSet::new(),
            recording_rule_wizard: None,
            last_ui_refresh_at: None,
            ui_refresh_blocked_until: None,
            last_seen_at: Utc::now(),
        }
    }

    #[test]
    fn detects_room_context_from_compact_payload() {
        let session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        assert!(is_user_watching_room(&session, 42));
        assert!(!is_user_watching_room(&session, 7));
    }

    #[test]
    fn ignores_invalid_legacy_context() {
        let session = UserSession {
            last_menu_id: 1,
            current_context: r#"{"Control":{"RoomDetail":{"room":42}}}"#.to_string(),
            header_entities: HashSet::new(),
            recording_rule_wizard: None,
            last_ui_refresh_at: None,
            ui_refresh_blocked_until: None,
            last_seen_at: Utc::now(),
        };

        assert!(!is_user_watching_room(&session, 42));
    }

    #[test]
    fn event_refresh_cooldown_allows_session_without_timestamp() {
        let session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        assert!(can_refresh_after_event(&session, Utc::now(), 5));
    }

    #[test]
    fn event_refresh_cooldown_blocks_recent_refresh() {
        let mut session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));
        session.last_ui_refresh_at = Some(Utc::now());

        assert!(!can_refresh_after_event(&session, Utc::now(), 5));
    }

    #[test]
    fn ui_refresh_block_state_expires_by_time() {
        let now = Utc::now();
        let mut session = session_for(Payload::Control(ControlPayload::RoomDetail { room: 42 }));

        session.ui_refresh_blocked_until = Some(now + chrono::Duration::seconds(10));
        assert!(session.is_ui_refresh_blocked(now));

        session.ui_refresh_blocked_until = Some(now - chrono::Duration::seconds(1));
        assert!(!session.is_ui_refresh_blocked(now));
    }
}
