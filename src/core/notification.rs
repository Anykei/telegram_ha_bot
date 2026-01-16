use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use std::sync::Arc;
use log::{info, error};
use teloxide::Bot;
use teloxide::prelude::ChatId;
use teloxide::types::MessageId;
use crate::models::{AppConfig, NotificationData};
use crate::db;
use crate::db::StateMap;
use crate::ha::NotifyEvent;

pub fn spawn_notification_processor(
    mut rx: mpsc::Receiver<NotifyEvent>,
    bot: teloxide::prelude::Bot,
    config: Arc<AppConfig>,
    cancel_token: CancellationToken,
) {
    info!("Core: Notification processor started");

    tokio::spawn(async move {
        loop {
            tokio::select! {
            Some(event) = rx.recv() => {
                let cfg = config.clone();
                let b = bot.clone();

                tokio::spawn(async move {
                    if let Err(e) = process_and_dispatch(b, cfg, event).await {
                        error!("Core: Error processing event: {}", e);
                    }
                });
            }
            _ = cancel_token.cancelled() => break,
        }
        }
    });
}

async fn process_and_dispatch(bot: Bot, config: Arc<AppConfig>, event: NotifyEvent) -> anyhow::Result<()> {
    if event.new_state == event.old_state {
        // refresh_live_interfaces(&bot, &config, &event.entity_id).await;
        return Ok(());
    }
    info!("Core: New state change {}", event.entity_id, );

    db::device_event_log::EventLogger::record_event(&config.db, &event.entity_id, &event.new_state).await?;

    let recipients = db::get_subscribers(&config.db, &event.entity_id).await?;
    if !recipients.is_empty() {
        for uid in recipients.clone() {
            info!("Core: New event change {}, {}", event.entity_id, uid);
            refresh_live_interface_for_recipients(&bot, &config, uid).await;
        }

        info!("State changed {}: {}", event.entity_id, event.new_state);

        let display_name = config.name_aliases.get(&event.entity_id)
            .map(|s| s.clone()).unwrap_or_else(|| event.friendly_name.clone());

        let state_aliases = db::get_state_aliases(&config.db).await;
        let human_state = format_state_human(&event.entity_id, &event.new_state, &state_aliases);

        let data = NotificationData {
            display_name,
            human_state,
            entity_id: event.entity_id.clone(),
            recipients,
        };

        let b_clone = bot.clone();
        let c_clone = config.clone();
        let d_clone = data.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::bot::notification::send_notification(b_clone, c_clone, d_clone).await {
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

pub fn format_state_human(entity_id: &str, state: &str, custom_map: &StateMap) -> String {
    if let Some(mapped) = custom_map.get(entity_id).and_then(|m| m.get(state)) {
        return mapped.clone();
    }

    let domain = entity_id.split('.').next().unwrap_or("");

    let s = state.to_lowercase();
    let s_ref = s.as_str();

    match (domain, s_ref) {
        // Бинарные датчики (двери, окна, движение)
        ("binary_sensor", "on") => "🔓 Открыто".to_string(),
        ("binary_sensor", "off") => "🔒 Закрыто".to_string(),

        // Замки
        ("lock", "locked") => "🔐 Закрыто".to_string(),
        ("lock", "unlocked") => "🔓 Открыто".to_string(),
        ("lock", "locking") => "⏳ Закрывается...".to_string(),
        ("lock", "unlocking") => "⏳ Открывается...".to_string(),

        // Освещение и выключатели
        ("light" | "switch", "on") => "Включено".to_string(),
        ("light" | "switch", "off") => "Выключено".to_string(),

        // Общие системные состояния
        (_, "unavailable") => "🔌 Недоступно".to_string(),
        (_, "unknown") => "❓ Неизвестно".to_string(),

        (_, _) => state.to_string(),
    }
}